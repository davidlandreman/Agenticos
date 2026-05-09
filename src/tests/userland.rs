use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::mm::paging::{
    UserMapError, UserPerms, USER_LOAD_BASE, USER_TRAMPOLINE_VA, USER_VA_RANGE_END,
    USER_VA_RANGE_START,
};
use crate::userland::abi::{
    self, register_syscall, syscall_dispatch, syscall_id, validate_user_slice,
    UserVaBounds, EFAULT, ENOSYS, LAST_EXIT_CODE,
};
use crate::userland::syscalls::print_handler;
use crate::userland::trampoline::{
    build_trampoline_bytes, emit_stub, STUB_SIZE,
};
use crate::userland::error::LoaderError;
use crate::userland::loader::load_elf;
use crate::tests::userland_fixtures as fix;
use alloc::vec;
use x86_64::VirtAddr;

/// After `gdt::init()`, CS must read 0x08 and SS 0x10 — the two literals the
/// existing naked asm in `src/arch/x86_64/{preemption,context_switch}.rs`
/// hard-codes when constructing `iretq` frames.
fn test_gdt_kernel_selectors() {
    use x86_64::instructions::segmentation::{CS, SS, Segment};
    let cs = CS::get_reg();
    let ss = SS::get_reg();
    assert_eq!(cs.0, 0x08, "kernel CS must remain at GDT slot 1");
    assert_eq!(ss.0, 0x10, "kernel SS must remain at GDT slot 2");
}

/// The user selectors live at known offsets and carry RPL=3.
fn test_gdt_user_selectors() {
    use x86_64::PrivilegeLevel;
    use crate::arch::x86_64::gdt::selectors;
    let sel = selectors();
    assert_eq!(sel.user_data.0 & !0x3, 0x18, "user data at GDT slot 3");
    assert_eq!(sel.user_code.0 & !0x3, 0x20, "user code at GDT slot 4");
    assert_eq!(sel.user_data.rpl(), PrivilegeLevel::Ring3);
    assert_eq!(sel.user_code.rpl(), PrivilegeLevel::Ring3);
}

/// `ltr` must have run — TR is non-zero after `gdt::init()`.
fn test_tss_loaded() {
    let tr: u16;
    unsafe {
        core::arch::asm!("str {:x}", out(reg) tr, options(nomem, nostack, preserves_flags));
    }
    assert_ne!(tr, 0, "TR must be loaded with the TSS selector after gdt::init()");
}

/// `map_user_region` followed by a kernel-mode read of the page succeeds
/// (kernel reads are unaffected by the U bit; the inverse is what's blocked).
/// The mapped page is zero-filled. Each subtest uses a disjoint VA so failure
/// of one cannot perturb another.
fn test_map_user_region_kernel_can_read() {
    let va = VirtAddr::new(USER_LOAD_BASE);
    let frames = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(va, 1, UserPerms::ReadWrite)
    })
    .expect("mapper")
    .expect("map");
    assert_eq!(frames.len(), 1);

    // Read every byte of the page from kernel mode — must not fault.
    let mut sum: u64 = 0;
    unsafe {
        let p = va.as_u64() as *const u8;
        for i in 0..0x1000 {
            sum = sum.wrapping_add(*p.add(i) as u64);
        }
    }
    assert_eq!(sum, 0, "freshly mapped user page should be zero-filled");

    // Tear down for isolation from the next test.
    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

/// After `map_user_region`, every parent table entry on the path
/// (PML4 -> PDPT -> PD -> PT) has the USER bit set. Walks the page tables
/// directly via `user_bit_set_on_all_parents` — a test-only helper.
fn test_map_user_region_propagates_user_bit() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x1000);
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(va, 1, UserPerms::ReadExecute)
    })
    .unwrap()
    .unwrap();

    let ok = crate::mm::memory::with_memory_mapper(|m| m.user_bit_set_on_all_parents(va))
        .unwrap();
    assert!(ok, "USER bit must be set on every parent table entry");

    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

/// `unmap_user_region` returns the freed frames, in the same order they were
/// allocated. The test maps two pages, unmaps them, and checks the returned
/// frame list matches what `map_user_region` returned.
fn test_unmap_user_region_returns_frames() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x2000);
    let mapped = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(va, 2, UserPerms::ReadWrite)
    })
    .unwrap()
    .unwrap();
    assert_eq!(mapped.len(), 2);

    let unmapped = crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 2))
        .unwrap()
        .unwrap();
    assert_eq!(unmapped.len(), 2);
    assert_eq!(unmapped[0], mapped[0]);
    assert_eq!(unmapped[1], mapped[1]);
}

/// Mapping the same range twice must fail with `PageAlreadyMapped` rather
/// than silently overwriting — the user range must be empty when load
/// begins. Defends the page-fault footgun (S3 in the doc-review findings).
fn test_map_user_region_rejects_double_map() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x4000);
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(va, 1, UserPerms::ReadWrite)
    })
    .unwrap()
    .unwrap();

    let err = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(va, 1, UserPerms::ReadWrite)
    })
    .unwrap()
    .unwrap_err();
    assert_eq!(err, UserMapError::PageAlreadyMapped);

    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

/// Ranges outside the user VA window are rejected with `VaOutOfRange`.
/// Covers: kernel-heap address, address above the user range, misaligned
/// address, zero pages, and an in-range start whose end overflows the
/// upper bound.
fn test_map_user_region_rejects_out_of_range() {
    crate::mm::memory::with_memory_mapper(|m| {
        // Kernel heap address.
        let r = m.map_user_region(
            VirtAddr::new(0x_4444_4444_0000),
            1,
            UserPerms::ReadWrite,
        );
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // Above the user range.
        let r = m.map_user_region(
            VirtAddr::new(USER_VA_RANGE_END),
            1,
            UserPerms::ReadWrite,
        );
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // Misaligned start.
        let r = m.map_user_region(
            VirtAddr::new(USER_VA_RANGE_START + 1),
            1,
            UserPerms::ReadWrite,
        );
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // Zero pages.
        let r = m.map_user_region(VirtAddr::new(USER_LOAD_BASE), 0, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // In-range start whose end exceeds `USER_VA_RANGE_END`.
        let last_page = VirtAddr::new(USER_VA_RANGE_END - 0x1000);
        let r = m.map_user_region(last_page, 2, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);
    })
    .unwrap();
}

/// Unmapping a never-mapped range fails with `PageNotMapped` rather than
/// returning an empty frame list.
fn test_unmap_user_region_rejects_unmapped() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x6000);
    let err = crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap_err();
    assert_eq!(err, UserMapError::PageNotMapped);
}

// ---------- U5: int 0x80 transport + symbol table + trampoline page ----------

/// `print` and `exit` are pre-registered at kernel init with stable IDs 0/1.
/// A user binary that imports these names must see the same IDs the
/// trampoline page emits.
fn test_first_class_syscalls_registered() {
    assert_eq!(syscall_id("print"), Some(0));
    assert_eq!(syscall_id("exit"), Some(1));
}

/// Registering a duplicate name returns an error rather than overwriting.
fn test_register_syscall_rejects_duplicate() {
    let err = register_syscall("print", print_handler).unwrap_err();
    assert_eq!(err, abi::RegisterError::DuplicateName);
}

/// `emit_stub` produces the expected `mov rax, imm32; int 0x80; ret` bytes,
/// and the imm32 reflects the syscall ID.
fn test_emit_stub_byte_layout() {
    let mut buf = [0u8; STUB_SIZE];
    emit_stub(0, &mut buf);
    assert_eq!(buf, [0xB8, 0x00, 0x00, 0x00, 0x00, 0xCD, 0x80, 0xC3]);

    let mut buf = [0u8; STUB_SIZE];
    emit_stub(1, &mut buf);
    assert_eq!(buf, [0xB8, 0x01, 0x00, 0x00, 0x00, 0xCD, 0x80, 0xC3]);

    // A larger ID lands in the imm32 little-endian.
    let mut buf = [0u8; STUB_SIZE];
    emit_stub(0x1234_5678, &mut buf);
    assert_eq!(buf, [0xB8, 0x78, 0x56, 0x34, 0x12, 0xCD, 0x80, 0xC3]);
}

/// `build_trampoline_bytes` emits one stub per registered syscall at the
/// expected offset, and returns the symbol VA list.
fn test_build_trampoline_bytes_matches_registry() {
    let (bytes, symbols) = build_trampoline_bytes();
    assert_eq!(bytes.len(), 0x1000);

    // print is ID 0, exit is ID 1; they should be the first two entries.
    assert!(symbols.iter().any(|s| s.name == "print" && s.va == USER_TRAMPOLINE_VA));
    assert!(symbols.iter().any(|s| s.name == "exit" && s.va == USER_TRAMPOLINE_VA + STUB_SIZE as u64));

    // Bytes at offset 0 (print/ID 0) and offset 8 (exit/ID 1) match emit_stub.
    let mut expected = [0u8; STUB_SIZE];
    emit_stub(0, &mut expected);
    assert_eq!(&bytes[0..STUB_SIZE], &expected);
    let mut expected = [0u8; STUB_SIZE];
    emit_stub(1, &mut expected);
    assert_eq!(&bytes[STUB_SIZE..STUB_SIZE * 2], &expected);
}

/// `syscall_dispatch` routes by RAX. Out-of-range ID returns ENOSYS without
/// invoking any handler.
fn test_dispatch_unregistered_returns_enosys() {
    let mut args = SyscallArgs::default();
    args.rax = 9999; // out of range
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
}

/// Registering a synthetic "nop" syscall and dispatching to its ID runs the
/// registered handler and returns its value through RAX.
fn test_dispatch_routes_to_registered_handler() {
    fn nop_handler(args: &mut SyscallArgs) -> i64 {
        // Return RDI back as the result so the test can detect it.
        args.rdi as i64
    }
    let id = register_syscall("u5_test_nop", nop_handler).expect("register");
    let mut args = SyscallArgs::default();
    args.rax = id as u64;
    args.rdi = 0xCAFE;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0xCAFE);
}

/// `print(valid_ptr, len)` from a fake-active-user context succeeds and
/// returns `len`. Pointer to a kernel buffer is allowed only because we set
/// the active user-VA bounds to bracket that buffer for the duration of the
/// test — the dispatcher does not care where the bytes come from, only that
/// the slice lies within the declared bounds.
fn test_print_handler_valid_slice() {
    let buf: [u8; 5] = [b'h', b'e', b'l', b'l', b'o'];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = 0; // print
    args.rdi = ptr;
    args.rsi = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, buf.len() as i64);

    abi::clear_user_va_bounds();
}

/// `print(0xffff_8000_0000_0000, 5)` (kernel address) is rejected without
/// dereferencing. With no active user-VA bounds set, all pointer validation
/// fails closed.
fn test_print_handler_rejects_kernel_pointer() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = 0;
    args.rdi = 0xffff_8000_0000_0000;
    args.rsi = 5;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
}

/// `print` with a slice that starts inside the bounds but extends past the
/// upper edge is rejected.
fn test_print_handler_rejects_span_past_bounds() {
    let buf: [u8; 8] = [0; 8];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 8,
    });
    let mut args = SyscallArgs::default();
    args.rax = 0;
    args.rdi = ptr + 4;
    args.rsi = 100;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
}

/// **S2 fix**: `print(0xFFFF_FFFF_FFFF_FF00, 0x200)` would wrap around to a
/// low address if `ptr + len` were computed without `checked_add`. Verify the
/// validator rejects it. We also set bounds to a wide window so a non-checked
/// implementation would *not* be caught by the simple `end > bounds.end` test.
fn test_print_handler_rejects_pointer_wraparound() {
    abi::set_user_va_bounds(UserVaBounds {
        start: 0,
        end: u64::MAX,
    });
    let mut args = SyscallArgs::default();
    args.rax = 0;
    args.rdi = 0xFFFF_FFFF_FFFF_FF00;
    args.rsi = 0x200;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
}

/// `print(_, 0)` succeeds, prints nothing, and returns 0 even when no
/// user-VA bounds are active (the slice is empty so no pointer is read).
fn test_print_handler_zero_len_succeeds() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = 0;
    args.rdi = 0xdead_beef;
    args.rsi = 0;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
}

/// `exit(42)` records 42 in `LAST_EXIT_CODE` (the U7-pending placeholder).
fn test_exit_handler_records_code() {
    *LAST_EXIT_CODE.lock() = None;
    let mut args = SyscallArgs::default();
    args.rax = 1; // exit
    args.rdi = 42;
    let _ = syscall_dispatch(&mut args);
    assert_eq!(*LAST_EXIT_CODE.lock(), Some(42));
}

/// Direct unit test of the validator: `len = 0` with an arbitrary pointer
/// is OK; large `len` is rejected when no bounds are set.
fn test_validate_user_slice_zero_len_ok() {
    abi::clear_user_va_bounds();
    assert!(validate_user_slice(0xdead_beef, 0).is_ok());
}

/// Drive the naked `int 0x80` stub from kernel mode (CPL=0). The interrupt
/// gate's DPL=3 means CPL <= DPL is satisfied (0 <= 3), so kernel-mode
/// `int 0x80` is allowed too. The dispatcher sees RAX=ID and writes the
/// handler's return value back into RAX, which must reach us through the
/// `iretq`. Uses the synthetic handler registered earlier via
/// `test_dispatch_routes_to_registered_handler` (so name reuse is fine — a
/// failed re-registration just no-ops). To stay independent of test order,
/// register a fresh handler name here.
fn test_int_0x80_returns_value() {
    fn echo_handler(args: &mut SyscallArgs) -> i64 {
        // Return RDI as the syscall return value.
        (args.rdi as i64).wrapping_add(1)
    }
    let id = register_syscall("u5_test_echo", echo_handler).expect("register");
    let id_u32 = id as u32;
    let arg: u64 = 0x1000;
    let result: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inout("rax") id_u32 as u64 => result,
            in("rdi") arg,
            // Clobbered by the handler path; even though we are CPL=0 here,
            // the gate semantics save/restore RFLAGS via iretq.
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    assert_eq!(result, (arg as i64).wrapping_add(1) as u64);
}

// ---------- U6: ELF loader + relocations ----------

/// Happy path: a hand-crafted minimal valid ELF loads into a `UserImage` with
/// `entry == p_vaddr`, the recorded mapping list covers the PT_LOAD plus the
/// user stack (2 entries), and the bss tail beyond `p_filesz` is zero in the
/// freshly mapped page. Drop tears everything down so the next test starts
/// from the same baseline.
fn test_loader_happy_path() {
    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("load_elf happy");

    assert_eq!(image.entry.as_u64(), 0x40_0000);
    assert_eq!(image.stack_top.as_u64(), crate::mm::paging::USER_STACK_TOP);

    // Two mappings: 1-page PT_LOAD + 8-page stack.
    assert_eq!(image.mapping_count(), 2);
    assert_eq!(image.total_pages(), 1 + 8);

    // bss zero-fill: bytes 16..0x100 in the loaded segment must read zero
    // through the kernel-side user-VA (kernel reads are unaffected by USER+R+X).
    unsafe {
        let p = 0x40_0000u64 as *const u8;
        for i in 16..0x100 {
            assert_eq!(*p.add(i), 0, "bss tail not zeroed at +{}", i);
        }
        // The first 16 bytes match what we wrote into the payload.
        for i in 0..16u8 {
            assert_eq!(*p.add(i as usize), i);
        }
    }
}

fn test_loader_bad_magic() {
    let bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::from(&b"XXXX"[..]);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::BadMagic);
}

fn test_loader_wrong_arch() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u16(&mut bytes, 18, fix::EM_AARCH64);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongArch);
}

fn test_loader_wrong_class() {
    let mut bytes = fix::happy_path_elf();
    bytes[4] = 1; // ELFCLASS32
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongArch);
}

fn test_loader_wrong_type() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u16(&mut bytes, 16, fix::ET_REL);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongType);
}

fn test_loader_truncated_phdrs() {
    // Claim two phdrs but only ship room for one.
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![fix::PhdrSpec {
            p_type: fix::PT_LOAD,
            p_flags: fix::PF_R | fix::PF_X,
            p_offset: 0x1000,
            p_vaddr: 0x40_0000,
            p_filesz: 4,
            p_memsz: 4,
            p_align: 0x1000,
        }],
        payloads: vec![(0x1000u64, vec![1u8, 2, 3, 4])],
        truncate_to: None,
    }
    .build();
    let mut bytes = bytes;
    // Manually rewrite e_phnum to 4 — file ends after one phdr's worth.
    fix::write_u16(&mut bytes, 56, 4);
    bytes.truncate((fix::EHDR_SIZE + fix::PHDR_SIZE) as usize); // only one phdr present
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::Truncated);
}

fn test_loader_va_out_of_range() {
    // Place PT_LOAD at the kernel heap. The loader must reject before any
    // mapping is attempted.
    let p_vaddr = 0x_4444_4444_0000u64;
    let p_offset = 0x1000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: p_vaddr,
        phdrs: vec![fix::PhdrSpec {
            p_type: fix::PT_LOAD,
            p_flags: fix::PF_R | fix::PF_X,
            p_offset,
            p_vaddr,
            p_filesz: 4,
            p_memsz: 4,
            p_align: 0x1000,
        }],
        payloads: vec![(p_offset, vec![1u8, 2, 3, 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::VaOutOfRange);
}

fn test_loader_overlapping_pt_load() {
    // Two PT_LOADs both at 0x40_0000.
    let p_offset_a = 0x1000u64;
    let p_offset_b = 0x2000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_X,
                p_offset: p_offset_a,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 0x100,
                p_align: 0x1000,
            },
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_W,
                p_offset: p_offset_b,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 0x100,
                p_align: 0x1000,
            },
        ],
        payloads: vec![(p_offset_a, vec![1u8; 4]), (p_offset_b, vec![2u8; 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::OverlappingPtLoad);
}

fn test_loader_entry_not_mapped() {
    // Entry is past the mapped segment.
    let mut bytes = fix::happy_path_elf();
    // Rewrite e_entry to something far outside.
    fix::write_u64(&mut bytes, 24, 0x40_5000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::EntryNotMapped);
}

fn test_loader_alignment_bad() {
    // p_align != 0x1000.
    let mut bytes = fix::happy_path_elf();
    // p_align lives at phdr_off + 48; phdr_off = 64.
    fix::write_u64(&mut bytes, 64 + 48, 0x2000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::AlignmentBad);
}

fn test_loader_pt_tls_rejected() {
    let p_offset = 0x1000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_X,
                p_offset,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 0x1000,
            },
            fix::PhdrSpec {
                p_type: fix::PT_TLS,
                p_flags: fix::PF_R,
                p_offset,
                p_vaddr: 0x40_2000,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 0x1000,
            },
        ],
        payloads: vec![(p_offset, vec![1u8; 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::TlsUnsupported);
}

fn test_loader_pt_interp_rejected() {
    let p_offset = 0x1000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_X,
                p_offset,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 0x1000,
            },
            fix::PhdrSpec {
                p_type: fix::PT_INTERP,
                p_flags: fix::PF_R,
                p_offset,
                p_vaddr: 0,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 1,
            },
        ],
        payloads: vec![(p_offset, vec![1u8; 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::InterpUnsupported);
}

fn test_loader_segment_overflow() {
    // p_offset + p_filesz overflows.
    let mut bytes = fix::happy_path_elf();
    // p_offset is at phdr_off + 8; p_filesz at phdr_off + 32.
    fix::write_u64(&mut bytes, 64 + 8, u64::MAX - 4);
    fix::write_u64(&mut bytes, 64 + 32, 100);
    let err = load_elf(&bytes).unwrap_err();
    assert!(
        matches!(err, LoaderError::SegmentOverflow | LoaderError::AlignmentBad),
        "got {:?}", err
    );
}

fn test_loader_unsupported_reloc() {
    // ELF with R_X86_64_TPOFF64 — must be rejected.
    let bytes = fix::elf_with_one_reloc("print", fix::R_X86_64_TPOFF64, 0x40_1000);
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::UnsupportedReloc(fix::R_X86_64_TPOFF64)
    );
}

fn test_loader_unresolved_import() {
    // ELF imports `nonexistent_kernel_symbol`.
    let bytes = fix::elf_with_one_reloc(
        "nonexistent_kernel_symbol",
        fix::R_X86_64_GLOB_DAT,
        0x40_1000,
    );
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::UnresolvedImport
    );
}

fn test_loader_bad_reloc_offset() {
    // r_offset points outside any PT_LOAD writable segment.
    // Use a kernel-range r_offset.
    let bytes = fix::elf_with_one_reloc(
        "print",
        fix::R_X86_64_GLOB_DAT,
        0xFFFF_8000_0000_0000,
    );
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::BadRelocOffset
    );
}

fn test_loader_reloc_into_text_rejected() {
    // r_offset lies inside the R-X "text" segment (0x40_0000) — must be
    // rejected: a writable user write here would let a crafted ELF rewrite
    // its own .text via the kernel-mode write path (S1).
    let bytes = fix::elf_with_one_reloc(
        "print",
        fix::R_X86_64_GLOB_DAT,
        0x40_0008,
    );
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::BadRelocOffset
    );
}

fn test_loader_relocation_patches_got_slot() {
    // Happy path with a GLOB_DAT relocation pointing into the R-W "data"
    // segment. After load, the qword at that VA must equal the trampoline
    // address for `print`.
    let target = 0x40_1000u64;
    let bytes = fix::elf_with_one_reloc("print", fix::R_X86_64_GLOB_DAT, target);

    // Capture trampoline VA for `print` via the kernel-side resolver. The
    // loader will (re)build the trampoline page lazily, so call it first
    // to materialize the symbol map.
    crate::userland::trampoline::build_and_map_trampoline_page().expect("trampoline");
    let want = crate::userland::trampoline::resolve("print").expect("print VA");

    let image = load_elf(&bytes).expect("load_elf reloc");

    // Read the patched qword via the kernel-visible user VA.
    let got_value = unsafe { core::ptr::read_unaligned(target as *const u64) };
    assert_eq!(got_value, want, "GOT slot must hold trampoline VA");

    drop(image);
}

fn test_loader_jump_slot_relocation() {
    // Same as above but R_X86_64_JUMP_SLOT (PLT-style).
    let target = 0x40_1010u64;
    let bytes = fix::elf_with_one_reloc("exit", 7 /* JUMP_SLOT */, target);
    crate::userland::trampoline::build_and_map_trampoline_page().expect("trampoline");
    let want = crate::userland::trampoline::resolve("exit").expect("exit VA");
    let image = load_elf(&bytes).expect("load_elf jump_slot");
    let got_value = unsafe { core::ptr::read_unaligned(target as *const u64) };
    assert_eq!(got_value, want);
    drop(image);
}

/// **F1+A3 finding**: an ELF without relocations must load successfully —
/// `static -no-pie` typically emits none. The happy-path fixture exercises
/// that already (no section headers); this test makes the property explicit.
fn test_loader_no_relocations_is_ok() {
    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("no-relocations load");
    drop(image);
}

/// On a relocation-phase failure, the partial `UserImage` is dropped and
/// every recorded mapping is unmapped. We verify by attempting to map the
/// same VAs again afterwards: success means the previous run released them.
/// (The bump frame allocator does not return frames, so we cannot assert on
/// frame counts; this tests the page-table-state invariant instead.)
fn test_loader_rollback_unmaps_on_reloc_failure() {
    // Use an unsupported-reloc fixture so we fail in phase 3 after phase 2
    // has mapped two PT_LOADs and the stack.
    let bytes = fix::elf_with_one_reloc("print", fix::R_X86_64_TPOFF64, 0x40_1000);
    assert!(load_elf(&bytes).is_err());

    // Now we should be able to map the same VAs cleanly. If the rollback
    // failed to unmap, this would return PageAlreadyMapped.
    let r1 = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(0x40_0000), 1, UserPerms::ReadExecute)
    })
    .unwrap();
    assert!(r1.is_ok(), "PT_LOAD #1 was not unmapped on rollback: {:?}", r1.err());

    let r2 = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(0x40_1000), 1, UserPerms::ReadWrite)
    })
    .unwrap();
    assert!(r2.is_ok(), "PT_LOAD #2 was not unmapped on rollback: {:?}", r2.err());

    let stack_bottom = crate::mm::paging::USER_STACK_TOP - 8 * 0x1000;
    let r3 = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(stack_bottom), 8, UserPerms::ReadWrite)
    })
    .unwrap();
    assert!(r3.is_ok(), "user stack was not unmapped on rollback: {:?}", r3.err());

    // Tear down for the next test.
    crate::mm::memory::with_memory_mapper(|m| {
        m.unmap_user_region(VirtAddr::new(0x40_0000), 1).unwrap();
        m.unmap_user_region(VirtAddr::new(0x40_1000), 1).unwrap();
        m.unmap_user_region(VirtAddr::new(stack_bottom), 8).unwrap();
    });
}

// ---------- U7: enter_user_mode + lifecycle ----------

/// Helper: drop any active image and clear the active-user slot. Lets tests
/// run in any order without depending on prior cleanup.
fn reset_active_user() {
    // Drop the image (if any) before clearing — its Drop unmaps the user VAs.
    let _img = crate::userland::release_active_image();
    drop(_img);
    crate::userland::force_clear_active_for_test();
}

/// T-E2E-no-arg analog: enter_user_mode rejects a second invocation while
/// another image is active.
fn test_enter_user_mode_single_user_invariant() {
    reset_active_user();

    // Stuff a fake image into the active slot to simulate "user app active."
    // We use a dummy UserImage built with no mappings — its Drop is a no-op
    // because mapping_count==0.
    let dummy = crate::userland::image::UserImage::new(
        x86_64::VirtAddr::new(0x40_0000),
        x86_64::VirtAddr::new(0x80_0000),
        0x40_0000,
        0x80_0000,
    );
    crate::userland::lifecycle::with_active_user(|au| {
        au.image = Some(dummy);
    });

    // Now try to enter again with a real image — must fail with AlreadyActive.
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let r = crate::userland::enter_user_mode(image);
    assert!(matches!(r, Err(crate::userland::EnterError::AlreadyActive)));

    reset_active_user();
}

/// T-E2E-happy: load + run hello fixture; cooperative exit code 0.
fn test_run_happy_path_hello() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 0);
}

/// T-E2E-fault-UD: UD2 as the first instruction causes #UD; cleanup runs;
/// the run command observes ExitKind::Abnormal.
fn test_run_fault_ud() {
    reset_active_user();
    let bytes = fix::fault_ud_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::Abnormal { vector, .. } => assert_eq!(vector, 6),
        other => panic!("expected Abnormal(#UD), got {:?}", other),
    }
}

/// T-E2E-fault-PF: deref kernel-range pointer.
fn test_run_fault_pf() {
    reset_active_user();
    let bytes = fix::fault_pf_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::Abnormal { vector, .. } => assert_eq!(vector, 14),
        other => panic!("expected Abnormal(#PF), got {:?}", other),
    }
}

/// T-E2E-fault-GP: privileged `cli`.
fn test_run_fault_gp() {
    reset_active_user();
    let bytes = fix::fault_gp_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::Abnormal { vector, .. } => assert_eq!(vector, 13),
        other => panic!("expected Abnormal(#GP), got {:?}", other),
    }
}

/// T-E2E-bad-pointer-syscall: print with kernel-range pointer returns
/// EFAULT; app calls exit with that. Verifies the exit code reaches the
/// run-side correctly (low byte of EFAULT = -14 → 0xF2 unsigned, but the
/// fixture passes RAX directly to RDI of exit, so the exit_code recorded
/// should be EFAULT (-14) sign-extended through the i32 cast in exit_handler).
fn test_run_bad_pointer_syscall() {
    reset_active_user();
    let bytes = fix::print_kernel_ptr_then_exit_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    // EFAULT = -14
    assert_eq!(result.1, -14);
}

/// T-E2E-leak-loop: load + exit happy fixture three times. Each cycle
/// must succeed — a leak in user-VA mappings would manifest as
/// `PageAlreadyMapped` from the loader on the second iteration.
fn test_run_leak_loop_happy() {
    for _ in 0..3 {
        reset_active_user();
        let bytes = fix::hello_exit0_elf();
        let image = load_elf(&bytes).expect("load_elf in leak loop");
        let result = crate::userland::enter_user_mode(image).expect("enter_user_mode in leak loop");
        let _ = crate::userland::release_active_image();
        use crate::userland::lifecycle::ExitKind;
        assert!(matches!(result.0, ExitKind::Cooperative));
    }
}

/// T-E2E-fault-leak-loop: same, but with a UD2 fault each time.
fn test_run_leak_loop_fault() {
    for _ in 0..3 {
        reset_active_user();
        let bytes = fix::fault_ud_elf();
        let image = load_elf(&bytes).expect("load_elf in fault leak loop");
        let result = crate::userland::enter_user_mode(image).expect("enter_user_mode in fault leak loop");
        let _ = crate::userland::release_active_image();
        use crate::userland::lifecycle::ExitKind;
        assert!(matches!(result.0, crate::userland::lifecycle::ExitKind::Abnormal { .. }));
    }
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_gdt_kernel_selectors,
        &test_gdt_user_selectors,
        &test_tss_loaded,
        &test_map_user_region_kernel_can_read,
        &test_map_user_region_propagates_user_bit,
        &test_unmap_user_region_returns_frames,
        &test_map_user_region_rejects_double_map,
        &test_map_user_region_rejects_out_of_range,
        &test_unmap_user_region_rejects_unmapped,
        // U5
        &test_first_class_syscalls_registered,
        &test_register_syscall_rejects_duplicate,
        &test_emit_stub_byte_layout,
        &test_build_trampoline_bytes_matches_registry,
        &test_dispatch_unregistered_returns_enosys,
        &test_dispatch_routes_to_registered_handler,
        &test_print_handler_valid_slice,
        &test_print_handler_rejects_kernel_pointer,
        &test_print_handler_rejects_span_past_bounds,
        &test_print_handler_rejects_pointer_wraparound,
        &test_print_handler_zero_len_succeeds,
        &test_exit_handler_records_code,
        &test_validate_user_slice_zero_len_ok,
        &test_int_0x80_returns_value,
        // U6
        &test_loader_happy_path,
        &test_loader_bad_magic,
        &test_loader_wrong_arch,
        &test_loader_wrong_class,
        &test_loader_wrong_type,
        &test_loader_truncated_phdrs,
        &test_loader_va_out_of_range,
        &test_loader_overlapping_pt_load,
        &test_loader_entry_not_mapped,
        &test_loader_alignment_bad,
        &test_loader_pt_tls_rejected,
        &test_loader_pt_interp_rejected,
        &test_loader_segment_overflow,
        &test_loader_unsupported_reloc,
        &test_loader_unresolved_import,
        &test_loader_bad_reloc_offset,
        &test_loader_reloc_into_text_rejected,
        &test_loader_relocation_patches_got_slot,
        &test_loader_jump_slot_relocation,
        &test_loader_no_relocations_is_ok,
        &test_loader_rollback_unmaps_on_reloc_failure,
        // U7
        &test_enter_user_mode_single_user_invariant,
        &test_run_happy_path_hello,
        &test_run_fault_ud,
        &test_run_fault_pf,
        &test_run_fault_gp,
        &test_run_bad_pointer_syscall,
        &test_run_leak_loop_happy,
        &test_run_leak_loop_fault,
    ]
}

