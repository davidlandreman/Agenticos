use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::mm::paging::{
    UserMapError, UserPerms, USER_LOAD_BASE, USER_VA_RANGE_END, USER_VA_RANGE_START,
};
use crate::userland::abi::{
    self, nr, syscall_dispatch, validate_user_slice, EBADF, EFAULT, ENOSYS, LAST_EXIT_CODE,
    UserVaBounds,
};
use crate::userland::error::LoaderError;
use crate::userland::loader::load_elf;
use crate::tests::userland_fixtures as fix;
use alloc::vec;
use x86_64::VirtAddr;

// ---------- GDT / TSS sanity ----------

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

// ---------- mm: user-region mapper ----------

fn test_map_user_region_kernel_can_read() {
    let va = VirtAddr::new(USER_LOAD_BASE);
    let frames = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(va, 1, UserPerms::ReadWrite)
    })
    .expect("mapper")
    .expect("map");
    assert_eq!(frames.len(), 1);

    let mut sum: u64 = 0;
    unsafe {
        let p = va.as_u64() as *const u8;
        for i in 0..0x1000 {
            sum = sum.wrapping_add(*p.add(i) as u64);
        }
    }
    assert_eq!(sum, 0, "freshly mapped user page should be zero-filled");

    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

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

        // In-range start whose end exceeds USER_VA_RANGE_END.
        let last_page = VirtAddr::new(USER_VA_RANGE_END - 0x1000);
        let r = m.map_user_region(last_page, 2, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);
    })
    .unwrap();
}

fn test_unmap_user_region_rejects_unmapped() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x6000);
    let err = crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap_err();
    assert_eq!(err, UserMapError::PageNotMapped);
}

// ---------- abi: numeric dispatcher + slice validation ----------

/// Out-of-range syscall numbers fall through to `-ENOSYS` without invoking
/// any handler.
fn test_dispatch_unregistered_returns_enosys() {
    let mut args = SyscallArgs::default();
    args.rax = 9999;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
}

/// `write(1, valid_ptr, len)` succeeds and returns `len`. The active
/// user-VA bounds bracket the kernel buffer for the duration of the call —
/// the dispatcher does not care where the bytes come from, only that the
/// slice lies within the declared bounds.
fn test_write_handler_valid_slice() {
    let buf: [u8; 5] = [b'h', b'e', b'l', b'l', b'o'];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1; // stdout
    args.rsi = ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, buf.len() as i64);

    abi::clear_user_va_bounds();
}

/// `write(99, ptr, len)` to an unsupported fd returns `-EBADF` without
/// touching the buffer.
fn test_write_handler_rejects_unknown_fd() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 99;
    args.rsi = 0xdead_beef;
    args.rdx = 16;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EBADF);
}

/// `write(1, kernel_ptr, 5)` is rejected by the slice validator without
/// dereferencing.
fn test_write_handler_rejects_kernel_pointer() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xffff_8000_0000_0000;
    args.rdx = 5;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
}

/// `write(1, ptr+4, 100)` with an 8-byte bounds window is rejected as the
/// span exceeds the upper bound.
fn test_write_handler_rejects_span_past_bounds() {
    let buf: [u8; 8] = [0; 8];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 8 });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = ptr + 4;
    args.rdx = 100;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
}

/// **Wraparound**: ptr + len overflowing u64 must be rejected even when
/// bounds are wide. checked_add is the defense.
fn test_write_handler_rejects_pointer_wraparound() {
    abi::set_user_va_bounds(UserVaBounds { start: 0, end: u64::MAX });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xFFFF_FFFF_FFFF_FF00;
    args.rdx = 0x200;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
}

/// `write(1, _, 0)` is a no-op, succeeds, and returns 0 even with no
/// active user-VA bounds.
fn test_write_handler_zero_len_succeeds() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xdead_beef;
    args.rdx = 0;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
}

/// `exit_group(42)` records 42 in `LAST_EXIT_CODE` (kernel-test fallback —
/// no active continuation here).
fn test_exit_group_handler_records_code() {
    *LAST_EXIT_CODE.lock() = None;
    let mut args = SyscallArgs::default();
    args.rax = nr::EXIT_GROUP;
    args.rdi = 42;
    let _ = syscall_dispatch(&mut args);
    assert_eq!(*LAST_EXIT_CODE.lock(), Some(42));
}

/// `validate_user_slice(_, 0)` is OK regardless of bounds.
fn test_validate_user_slice_zero_len_ok() {
    abi::clear_user_va_bounds();
    assert!(validate_user_slice(0xdead_beef, 0).is_ok());
}

// ---------- loader ----------

fn test_loader_happy_path() {
    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("load_elf happy");

    assert_eq!(image.entry.as_u64(), 0x40_0000);
    assert_eq!(image.stack_top.as_u64(), crate::mm::paging::USER_STACK_TOP);

    assert_eq!(image.mapping_count(), 2);
    assert_eq!(image.total_pages(), 1 + 8);

    unsafe {
        let p = 0x40_0000u64 as *const u8;
        for i in 16..0x100 {
            assert_eq!(*p.add(i), 0, "bss tail not zeroed at +{}", i);
        }
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
    bytes[4] = 1;
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongArch);
}

fn test_loader_wrong_type() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u16(&mut bytes, 16, fix::ET_REL);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongType);
}

fn test_loader_truncated_phdrs() {
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
    fix::write_u16(&mut bytes, 56, 4);
    bytes.truncate((fix::EHDR_SIZE + fix::PHDR_SIZE) as usize);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::Truncated);
}

fn test_loader_va_out_of_range() {
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
    let mut bytes = fix::happy_path_elf();
    fix::write_u64(&mut bytes, 24, 0x40_5000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::EntryNotMapped);
}

fn test_loader_alignment_bad() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u64(&mut bytes, 64 + 48, 0x2000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::AlignmentBad);
}

/// PT_TLS is now supported. The image loads, the TCB self-pointer is
/// initialized to USER_TCB_VA, and the FS_BASE accessor on UserImage
/// reflects the TCB address.
fn test_loader_pt_tls_loads() {
    use crate::mm::paging::{USER_TCB_VA, USER_TLS_IMAGE_VA};
    let bytes = fix::tls_smoke_elf();
    let image = load_elf(&bytes).expect("load_elf with PT_TLS");

    assert_eq!(image.tls_fs_base, Some(VirtAddr::new(USER_TCB_VA)));

    // tdata bytes (the four 0x55 bytes the fixture put at p_offset) landed
    // at the TLS image VA.
    unsafe {
        let p = USER_TLS_IMAGE_VA as *const u8;
        for i in 0..4 {
            assert_eq!(*p.add(i), 0x55, "tdata[{}] not copied", i);
        }
        // tbss is zero-filled by the fresh mapping.
        for i in 4..0x100 {
            assert_eq!(*p.add(i), 0, "tbss[{}] not zero", i);
        }
        // TCB self-pointer at offset 0.
        let tcb = USER_TCB_VA as *const u64;
        assert_eq!(core::ptr::read_unaligned(tcb), USER_TCB_VA);
        // dtv slot at offset 8 is zero.
        assert_eq!(core::ptr::read_unaligned(tcb.add(1)), 0);
    }

    drop(image);
}

/// Oversized PT_TLS (>4 KiB image) is rejected with `TlsUnsupported` so
/// the milestone's single-page TLS limit is honored.
fn test_loader_pt_tls_oversized_rejected() {
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
                p_vaddr: 0,
                p_filesz: 4,
                // 5 KiB image — over the milestone cap.
                p_memsz: 0x1400,
                p_align: 0x10,
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
    let mut bytes = fix::happy_path_elf();
    fix::write_u64(&mut bytes, 64 + 8, u64::MAX - 4);
    fix::write_u64(&mut bytes, 64 + 32, 100);
    let err = load_elf(&bytes).unwrap_err();
    assert!(
        matches!(err, LoaderError::SegmentOverflow | LoaderError::AlignmentBad),
        "got {:?}", err
    );
}

fn test_loader_unsupported_reloc() {
    let bytes = fix::elf_with_one_reloc("anything", fix::R_X86_64_TPOFF64, 0x40_1000);
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::UnsupportedReloc(fix::R_X86_64_TPOFF64)
    );
}

/// After the SYSCALL transition, any GLOB_DAT / JUMP_SLOT against an
/// undefined extern is rejected as `UnresolvedImport` — the kernel-side
/// name registry is gone, so the loader has no resolver to consult.
fn test_loader_glob_dat_unresolved() {
    let bytes = fix::elf_with_one_reloc(
        "anything",
        fix::R_X86_64_GLOB_DAT,
        0x40_1000,
    );
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::UnresolvedImport);
}

/// Static-no-pie ET_EXEC binaries from musl-cross-make typically emit no
/// relocations at all — the walker must accept that as a no-op.
fn test_loader_no_relocations_is_ok() {
    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("no-relocations load");
    drop(image);
}

/// On a relocation-phase failure, the partial `UserImage` is dropped and
/// every recorded mapping is unmapped. Verify by re-mapping the same VAs
/// after an `UnsupportedReloc` failure.
fn test_loader_rollback_unmaps_on_reloc_failure() {
    let bytes = fix::elf_with_one_reloc("anything", fix::R_X86_64_TPOFF64, 0x40_1000);
    assert!(load_elf(&bytes).is_err());

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

    crate::mm::memory::with_memory_mapper(|m| {
        m.unmap_user_region(VirtAddr::new(0x40_0000), 1).unwrap();
        m.unmap_user_region(VirtAddr::new(0x40_1000), 1).unwrap();
        m.unmap_user_region(VirtAddr::new(stack_bottom), 8).unwrap();
    });
}

// ---------- enter_user_mode + lifecycle ----------

fn reset_active_user() {
    let _img = crate::userland::release_active_image();
    drop(_img);
    crate::userland::force_clear_active_for_test();
}

fn test_enter_user_mode_single_user_invariant() {
    reset_active_user();

    let dummy = crate::userland::image::UserImage::new(
        x86_64::VirtAddr::new(0x40_0000),
        x86_64::VirtAddr::new(0x80_0000),
        0x40_0000,
        0x80_0000,
    );
    crate::userland::lifecycle::with_active_user(|au| {
        au.image = Some(dummy);
    });

    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let r = crate::userland::enter_user_mode(image);
    assert!(matches!(r, Err(crate::userland::EnterError::AlreadyActive)));

    reset_active_user();
}

/// Fixture B — Linux initial-stack contract.
///
/// The binary walks the kernel-built argc/argv/envp/auxv frame and exits
/// with code 0 if every check passes, or 1..6 indicating which assertion
/// failed (argc / argv[0] / argv[1] NULL / envp[0] NULL / AT_RANDOM
/// missing / AT_RANDOM ptr null).
fn test_run_initial_stack_fixture_b() {
    reset_active_user();
    let bytes = fix::auxv_walker_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 0,
        "auxv walker exited with code {} — see fixture comments for the meaning",
        result.1
    );
}

/// Fixture A — SYSCALL fast-path smoke test. The smallest possible end-to-end
/// proof of the SYSCALL transition: a `syscall` with `RAX=NR_EXIT_GROUP,
/// RDI=42` records exit code 42 via cooperative_exit.
fn test_run_syscall_exit42_fixture_a() {
    reset_active_user();
    let bytes = fix::syscall_exit42_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 42);
}

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

/// `write` with a kernel-range pointer returns EFAULT; the app then does
/// `exit_group(EFAULT)`. EFAULT = -14 (sign-extended through the i32 cast
/// in `exit_group_handler`).
fn test_run_bad_pointer_syscall() {
    reset_active_user();
    let bytes = fix::print_kernel_ptr_then_exit_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, -14);
}

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
        // ABI / dispatcher
        &test_dispatch_unregistered_returns_enosys,
        &test_write_handler_valid_slice,
        &test_write_handler_rejects_unknown_fd,
        &test_write_handler_rejects_kernel_pointer,
        &test_write_handler_rejects_span_past_bounds,
        &test_write_handler_rejects_pointer_wraparound,
        &test_write_handler_zero_len_succeeds,
        &test_exit_group_handler_records_code,
        &test_validate_user_slice_zero_len_ok,
        // Loader
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
        &test_loader_pt_tls_loads,
        &test_loader_pt_tls_oversized_rejected,
        &test_loader_pt_interp_rejected,
        &test_loader_segment_overflow,
        &test_loader_unsupported_reloc,
        &test_loader_glob_dat_unresolved,
        &test_loader_no_relocations_is_ok,
        &test_loader_rollback_unmaps_on_reloc_failure,
        // enter_user_mode lifecycle
        &test_enter_user_mode_single_user_invariant,
        &test_run_initial_stack_fixture_b,
        &test_run_syscall_exit42_fixture_a,
        &test_run_happy_path_hello,
        &test_run_fault_ud,
        &test_run_fault_pf,
        &test_run_fault_gp,
        &test_run_bad_pointer_syscall,
        &test_run_leak_loop_happy,
        &test_run_leak_loop_fault,
    ]
}
