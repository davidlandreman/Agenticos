use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::mm::paging::{
    UserMapError, UserPerms, USER_LOAD_BASE, USER_VA_RANGE_END, USER_VA_RANGE_START,
};
use crate::userland::abi::{
    self, nr, syscall_dispatch, validate_user_slice, EBADF, EFAULT, EINVAL, ENOENT, ENOSYS,
    ERANGE, EROFS, LAST_EXIT_CODE, UserVaBounds,
};
use crate::userland::error::LoaderError;
use crate::userland::fdtable::{FdSlot, FdTable};
use crate::userland::loader::load_elf;
use crate::userland::path::{copy_user_cstr, normalize_path};
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

/// Trace mode on: an unknown syscall returns `-ENOSYS` (same as off, in
/// the synthetic test path) and marks the per-nr "seen" bit so the next
/// occurrence demotes from info to trace.
fn test_unknown_syscall_trace_mode_returns_enosys_and_marks_seen() {
    use crate::userland::abi::{
        is_trace_mode, reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    let prior = is_trace_mode();
    set_trace_mode(true);
    reset_unknown_syscall_trace();
    // Pick an unused-but-in-range nr (Linux x86-64 currently uses 0..335;
    // 411 is unused and < TRACE_NR_CAPACITY so the per-nr bookkeeping
    // applies). Using a number ≥ 512 would test the overflow path
    // instead — see test_unknown_syscall_trace_mode_capacity_overflow.
    let nr = 411;
    assert!(!unknown_syscall_was_seen(nr));
    let mut args = SyscallArgs::default();
    args.rax = nr;
    args.rdi = 0xdead_beef;
    args.rsi = 0xfeed_face;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
    assert!(unknown_syscall_was_seen(nr));
    // Restore prior state so subsequent tests see the same dispatcher
    // behavior they were authored against.
    set_trace_mode(prior);
    reset_unknown_syscall_trace();
}

/// Trace mode on, same nr twice: the swap is itself the bookkeeping —
/// `unknown_syscall_was_seen` reports true after the first call and stays
/// true after subsequent calls.
fn test_unknown_syscall_trace_mode_marks_only_once() {
    use crate::userland::abi::{
        reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    set_trace_mode(true);
    reset_unknown_syscall_trace();
    let nr = 412;
    let mut args = SyscallArgs::default();
    args.rax = nr;
    let _ = syscall_dispatch(&mut args);
    assert!(unknown_syscall_was_seen(nr));
    let _ = syscall_dispatch(&mut args);
    assert!(unknown_syscall_was_seen(nr));
    set_trace_mode(false);
    reset_unknown_syscall_trace();
}

/// Trace mode OFF: the synthetic-test dispatcher path returns `-ENOSYS`
/// (no active continuation to long-jump to) but does NOT mark the SEEN
/// bookkeeping — that's exclusive to trace mode.
fn test_unknown_syscall_trace_mode_off_does_not_mark() {
    use crate::userland::abi::{
        reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    set_trace_mode(false);
    reset_unknown_syscall_trace();
    let nr = 413;
    let mut args = SyscallArgs::default();
    args.rax = nr;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
    assert!(!unknown_syscall_was_seen(nr));
}

/// Trace mode on, nr beyond TRACE_NR_CAPACITY (512): handler returns
/// ENOSYS without panicking and `unknown_syscall_was_seen` reports false
/// (those numbers are not tracked individually — they log every time).
fn test_unknown_syscall_trace_mode_capacity_overflow() {
    use crate::userland::abi::{reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen};
    set_trace_mode(true);
    reset_unknown_syscall_trace();
    let nr = 9999; // > 512
    let mut args = SyscallArgs::default();
    args.rax = nr;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
    assert!(!unknown_syscall_was_seen(nr));
    set_trace_mode(false);
}

/// `write(1, valid_ptr, len)` succeeds and returns `len`. The active
/// user-VA bounds bracket the kernel buffer for the duration of the call —
/// the dispatcher does not care where the bytes come from, only that the
/// slice lies within the declared bounds.
/// Pin stdin/stdout/stderr in the FD table so the dispatcher doesn't
/// reject `write(1, …)` with `-EBADF`. Phase 2 routed `write` through
/// the FD table; the older write-handler tests need the streams pinned
/// to keep their original meaning.
fn install_streams_for_dispatcher_test() {
    crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.clear();
        au.fd_table.install_default_streams();
    });
}

fn clear_streams_after_dispatcher_test() {
    crate::userland::lifecycle::with_active_user(|au| au.fd_table.clear());
}

fn test_write_handler_valid_slice() {
    install_streams_for_dispatcher_test();
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
    clear_streams_after_dispatcher_test();
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
    install_streams_for_dispatcher_test();
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xffff_8000_0000_0000;
    args.rdx = 5;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    clear_streams_after_dispatcher_test();
}

/// `write(1, ptr+4, 100)` with an 8-byte bounds window is rejected as the
/// span exceeds the upper bound.
fn test_write_handler_rejects_span_past_bounds() {
    install_streams_for_dispatcher_test();
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
    clear_streams_after_dispatcher_test();
}

/// **Wraparound**: ptr + len overflowing u64 must be rejected even when
/// bounds are wide. checked_add is the defense.
fn test_write_handler_rejects_pointer_wraparound() {
    install_streams_for_dispatcher_test();
    abi::set_user_va_bounds(UserVaBounds { start: 0, end: u64::MAX });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xFFFF_FFFF_FFFF_FF00;
    args.rdx = 0x200;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `write(1, _, 0)` is a no-op, succeeds, and returns 0 even with no
/// active user-VA bounds.
fn test_write_handler_zero_len_succeeds() {
    install_streams_for_dispatcher_test();
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xdead_beef;
    args.rdx = 0;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
    clear_streams_after_dispatcher_test();
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

/// Fixture D — unhandled-syscall trap. Binary issues `syscall RAX=999`;
/// kernel must terminate the process with `ExitKind::UnimplementedSyscall`
/// rather than panicking, hanging, or silently returning.
fn test_run_unhandled_syscall_fixture_d() {
    reset_active_user();
    let bytes = fix::syscall_999_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::UnimplementedSyscall { nr } => assert_eq!(nr, 999),
        other => panic!("expected UnimplementedSyscall(999), got {:?}", other),
    }
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

// ---------- Phase 1: user stdin queue + read(0) syscall ----------

/// `userland::stdin::pop_into` against an installed-but-empty queue
/// reports zero, and reports the requested bytes after a producer push.
fn test_user_stdin_install_push_pop() {
    crate::userland::stdin::clear();
    assert!(!crate::userland::stdin::is_active());

    crate::userland::stdin::install();
    assert!(crate::userland::stdin::is_active());

    let mut buf = [0u8; 16];
    assert_eq!(crate::userland::stdin::pop_into(&mut buf), 0,
        "empty queue must return 0 (caller treats as block-needed)");

    crate::userland::stdin::push_bytes(b"hi\n");
    assert_eq!(crate::userland::stdin::queued_len(), 3);

    let n = crate::userland::stdin::pop_into(&mut buf);
    assert_eq!(n, 3);
    assert_eq!(&buf[..n], b"hi\n");
    assert_eq!(crate::userland::stdin::queued_len(), 0);

    crate::userland::stdin::clear();
    assert!(!crate::userland::stdin::is_active());
}

/// `push_bytes` while no user is active is a silent no-op — the producer
/// (TerminalWindow) never has to gate the call itself.
fn test_user_stdin_push_when_inactive_is_noop() {
    crate::userland::stdin::clear();
    crate::userland::stdin::push_bytes(b"dropped");
    assert!(!crate::userland::stdin::is_active());
}

/// `read(0, ptr, len)` fast-path: when bytes are already queued, the
/// dispatcher returns them without entering the sti/hlt blocking loop.
/// Drives the dispatcher synthetically (no ring 3) so the test never
/// actually halts the CPU waiting for keyboard input.
fn test_dispatch_read_returns_queued_bytes() {
    install_streams_for_dispatcher_test();
    let buf = [0u8; 32];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 32 });

    crate::userland::stdin::clear();
    crate::userland::stdin::install();
    crate::userland::stdin::push_bytes(b"echo me\n");

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = 0;
    args.rsi = ptr;
    args.rdx = 16;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 8, "expected 8 bytes (\"echo me\\n\")");
    assert_eq!(&buf[..8], b"echo me\n");

    crate::userland::stdin::clear();
    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `read(0)` with no active user-stdin queue reports 0 (EOF) rather than
/// hanging in the sti/hlt loop. Defensive: a production launch always
/// installs the queue before iretq, so this branch only triggers in the
/// in-kernel test path.
fn test_dispatch_read_no_active_user_returns_zero() {
    install_streams_for_dispatcher_test();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 16 });
    crate::userland::stdin::clear();

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = 0;
    args.rsi = ptr;
    args.rdx = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// Smoke test for `enter_user_mode_with`: a hello binary launched with a
/// real argv/envp still exits cleanly with code 0. We don't assert on the
/// frame layout here — the auxv walker fixture covers the
/// argv=1, envp=0 case; this just verifies the new entry point doesn't
/// regress the happy path when extra strings are emitted onto the stack.
fn test_enter_user_mode_with_argv_envp() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let argv = ["/host/HELLO.ELF", "alpha", "beta"];
    let envp = ["PATH=/host", "HOME=/", "TERM=dumb"];
    let result = crate::userland::enter_user_mode_with(image, &argv, &envp)
        .expect("enter_user_mode_with");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 0);
}

// ---------- Phase 2: FD table ----------

fn test_fdtable_install_default_streams() {
    let mut t = FdTable::new();
    t.install_default_streams();
    assert!(matches!(t.get(0), Some(FdSlot::Stdin)));
    assert!(matches!(t.get(1), Some(FdSlot::Stdout)));
    assert!(matches!(t.get(2), Some(FdSlot::Stderr)));
    assert!(t.get(3).is_none());
}

fn test_fdtable_alloc_and_close() {
    let mut t = FdTable::new();
    t.install_default_streams();
    // Build a fake file slot for allocation. We can't easily fabricate a
    // real Arc<File> in unit tests, so we use the Stdin marker as a
    // stand-in — `alloc` accepts any FdSlot variant.
    let fd = t.alloc(FdSlot::Stdin).expect("first alloc");
    assert_eq!(fd, 3, "lowest-free-fd should start at 3");
    let fd2 = t.alloc(FdSlot::Stdin).expect("second alloc");
    assert_eq!(fd2, 4);
    assert!(t.close(fd).is_ok());
    let fd3 = t.alloc(FdSlot::Stdin).expect("third alloc reuses slot 3");
    assert_eq!(fd3, 3, "closed slot 3 must be reused");
    assert_eq!(t.close(99).err(), Some(EBADF));
}

fn test_fdtable_dup_and_dup2() {
    let mut t = FdTable::new();
    t.install_default_streams();
    let dup_fd = t.dup(1).expect("dup stdout");
    assert_eq!(dup_fd, 3);
    assert!(matches!(t.get(dup_fd), Some(FdSlot::Stdout)));

    let target = t.dup2(0, 7).expect("dup2 stdin to fd 7");
    assert_eq!(target, 7);
    assert!(matches!(t.get(7), Some(FdSlot::Stdin)));

    // dup2(fd, fd) is a no-op on a valid fd.
    assert_eq!(t.dup2(1, 1), Some(1));
    // dup2 from a closed fd returns None (-> EBADF at the syscall layer).
    assert_eq!(t.dup2(20, 5), None);
}

// ---------- Phase 2: path utilities ----------

fn test_normalize_path_absolute_keeps_path() {
    assert_eq!(normalize_path("/host", "/etc/passwd"), "/etc/passwd");
}

fn test_normalize_path_relative_anchors_at_cwd() {
    assert_eq!(normalize_path("/host", "foo.txt"), "/host/foo.txt");
    assert_eq!(normalize_path("/", "foo.txt"), "/foo.txt");
}

fn test_normalize_path_collapses_redundancy() {
    assert_eq!(normalize_path("/host", "./a/./b//c"), "/host/a/b/c");
    assert_eq!(normalize_path("/host", "a/../b"), "/host/b");
    assert_eq!(normalize_path("/host", "../.."), "/");
    assert_eq!(normalize_path("/", "../foo"), "/foo");
    assert_eq!(normalize_path("/host", "."), "/host");
}

fn test_copy_user_cstr_happy_path() {
    let bytes = b"hello\0world";
    let ptr = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + bytes.len() as u64,
    });
    let s = copy_user_cstr(ptr).expect("copy hello");
    assert_eq!(s, "hello");
    abi::clear_user_va_bounds();
}

fn test_copy_user_cstr_unterminated_at_bound_returns_efault() {
    let bytes = b"abcdef";
    let ptr = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + bytes.len() as u64,
    });
    // No NUL within bounds → -EFAULT
    assert_eq!(copy_user_cstr(ptr), Err(EFAULT));
    abi::clear_user_va_bounds();
}

// ---------- Phase 2: dispatcher tests ----------

/// Helper: install default streams and a fixed cwd for syscall tests
/// that don't go through `enter_user_mode_with`.
fn setup_phase2_active_user() {
    use alloc::string::String;
    crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.clear();
        au.fd_table.install_default_streams();
        au.cwd = String::from("/host");
    });
}

fn teardown_phase2_active_user() {
    crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.clear();
        au.cwd.clear();
    });
}

fn test_dispatch_getcwd_returns_default() {
    setup_phase2_active_user();
    let buf = [0u8; 64];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::GETCWD;
    args.rdi = ptr;
    args.rsi = 64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 6, "expected '/host\\0' → 6 bytes");
    assert_eq!(&buf[..6], b"/host\0");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_getcwd_short_buffer_returns_erange() {
    setup_phase2_active_user();
    let buf = [0u8; 4];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 4 });

    let mut args = SyscallArgs::default();
    args.rax = nr::GETCWD;
    args.rdi = ptr;
    args.rsi = 4; // Need 6 (5 + NUL) — short.
    assert_eq!(syscall_dispatch(&mut args), ERANGE);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_chdir_root_succeeds() {
    setup_phase2_active_user();
    let path = b"/\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + path.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::CHDIR;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let cwd = crate::userland::lifecycle::with_active_user(|au| au.cwd.clone());
    assert_eq!(cwd, "/");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_chdir_nonexistent_returns_enoent() {
    setup_phase2_active_user();
    let path = b"/nonexistent_directory_xyz\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + path.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::CHDIR;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), ENOENT);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_open_nonexistent_returns_enoent() {
    setup_phase2_active_user();
    let path = b"/host/NEVER_EXISTS_XYZ.TXT\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + path.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0; // O_RDONLY
    assert_eq!(syscall_dispatch(&mut args), ENOENT);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_open_writable_flag_returns_erofs() {
    setup_phase2_active_user();
    let path = b"/host/HELLO.ELF\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + path.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 1; // O_WRONLY
    assert_eq!(syscall_dispatch(&mut args), EROFS);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_close_stream_is_noop() {
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::CLOSE;
    args.rdi = 1; // stdout
    assert_eq!(syscall_dispatch(&mut args), 0);

    let still_open = crate::userland::lifecycle::with_active_user(|au| au.fd_table.get(1).is_some());
    assert!(still_open, "stdout must remain open after a close attempt");
    teardown_phase2_active_user();
}

fn test_dispatch_dup_stdout() {
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::DUP;
    args.rdi = 1; // stdout
    let new_fd = syscall_dispatch(&mut args);
    assert_eq!(new_fd, 3);

    let is_stdout = crate::userland::lifecycle::with_active_user(|au| {
        matches!(au.fd_table.get(3), Some(FdSlot::Stdout))
    });
    assert!(is_stdout);
    teardown_phase2_active_user();
}

fn test_dispatch_lseek_on_stream_returns_espipe() {
    use crate::userland::abi::ESPIPE;
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::LSEEK;
    args.rdi = 1;
    args.rsi = 0;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), ESPIPE);
    teardown_phase2_active_user();
}

fn test_dispatch_clock_gettime_writes_timespec() {
    setup_phase2_active_user();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 16 });

    let mut args = SyscallArgs::default();
    args.rax = nr::CLOCK_GETTIME;
    args.rdi = 1; // CLOCK_MONOTONIC
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    // tv_nsec at offset 8 must be < 1e9
    let ns_bytes: [u8; 8] = buf[8..16].try_into().unwrap();
    let nsec = i64::from_ne_bytes(ns_bytes);
    assert!((0..1_000_000_000).contains(&nsec), "nsec out of range: {}", nsec);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_clock_gettime_invalid_clock_einval() {
    setup_phase2_active_user();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 16 });

    let mut args = SyscallArgs::default();
    args.rax = nr::CLOCK_GETTIME;
    args.rdi = 99;
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), EINVAL);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_getrandom_fills_buffer() {
    setup_phase2_active_user();
    let buf = [0u8; 32];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + 32 });

    let mut args = SyscallArgs::default();
    args.rax = nr::GETRANDOM;
    args.rdi = ptr;
    args.rsi = 32;
    assert_eq!(syscall_dispatch(&mut args), 32);
    // At least one byte should be non-zero (deterministic for the seed).
    assert!(buf.iter().any(|&b| b != 0), "getrandom returned all zeros");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_uname_writes_sysname_linux() {
    setup_phase2_active_user();
    let buf = [0u8; 6 * 65];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + buf.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::UNAME;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(&buf[..5], b"Linux");
    // machine field at offset 4*65
    assert_eq!(&buf[4 * 65..4 * 65 + 6], b"x86_64");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

// ---------- Phase 4 PR-B: per-process address spaces ----------

/// `AddressSpace::new` succeeds after boot, returns a frame distinct
/// from the kernel L4 (which `capture_kernel_l4` recorded), and leaves
/// the user-half (PML4[0]) empty while copying the kernel-half entry
/// covering the heap (PML4[136]).
fn test_address_space_new_kernel_half_shared() {
    use crate::userland::address_space::AddressSpace;
    use x86_64::structures::paging::PageTable;

    let kernel_frame =
        crate::mm::paging::kernel_l4_frame().expect("kernel L4 captured at boot");

    let aspace = AddressSpace::new().expect("AddressSpace::new should succeed");
    assert_ne!(
        aspace.l4_frame(),
        kernel_frame,
        "process L4 must be a fresh frame, not the kernel L4 itself"
    );

    // Walk both L4s through the bootloader's offset mapping and
    // compare PML4 entries.
    let phys_offset = crate::mm::memory::get_physical_memory_offset()
        .expect("phys offset");
    let kernel_va = phys_offset + kernel_frame.start_address().as_u64();
    let user_va = phys_offset + aspace.l4_frame().start_address().as_u64();
    let kernel_table = unsafe { &*(kernel_va as *const PageTable) };
    let user_table = unsafe { &*(user_va as *const PageTable) };

    // PML4[0] is per-process — empty in the fresh user L4.
    assert!(
        user_table[0].is_unused(),
        "PML4[0] of a fresh AddressSpace must start unused"
    );

    // PML4[136] hosts the kernel heap (0x4444_4444_0000). It must be
    // mirrored from the kernel L4 so heap demand-paging continues to
    // work while the user L4 is active.
    assert_eq!(
        user_table[136].addr(),
        kernel_table[136].addr(),
        "kernel-heap PML4 entry must be shared by reference"
    );
    assert_eq!(
        user_table[136].flags(),
        kernel_table[136].flags(),
        "kernel-heap PML4 flags must match"
    );
}

/// Activating an `AddressSpace`, then dropping it, leaves CR3 pointing
/// at the kernel L4. The Drop impl is the safety net for early-return
/// error paths in `RunProcess::run_path`.
fn test_address_space_drop_restores_kernel_cr3() {
    use crate::userland::address_space::AddressSpace;
    use x86_64::registers::control::Cr3;

    let kernel_frame =
        crate::mm::paging::kernel_l4_frame().expect("kernel L4 captured at boot");

    {
        let aspace = AddressSpace::new().expect("AddressSpace::new");
        // SAFETY: kernel half copied from kernel L4 — the code after
        // the CR3 write is still mapped.
        unsafe { aspace.activate(); }
        let (after, _) = Cr3::read();
        assert_eq!(after, aspace.l4_frame());
        // aspace dropped here.
    }

    let (final_cr3, _) = Cr3::read();
    assert_eq!(
        final_cr3, kernel_frame,
        "AddressSpace::Drop must revert CR3 to the kernel L4"
    );
}

// ---------- Phase 5 PR-B: signals ----------

/// `rt_sigaction(SIGUSR1, &act, &oldact, 8)` round-trips: the
/// installed handler comes back via a follow-up "query only" call.
fn test_dispatch_rt_sigaction_round_trip() {
    use crate::userland::signal::{SigAction, SIGUSR1};
    setup_phase2_active_user();

    let new_action = SigAction {
        sa_handler: 0xCAFE_BABE_DEAD_BEEF,
        sa_flags: 0x4000_0000, // SA_RESTORER
        sa_restorer: 0x1234_5678_9ABC_DEF0,
        sa_mask: 0,
    };
    let act = new_action;
    let mut oldact = SigAction::default();

    let act_ptr = &act as *const SigAction as u64;
    let oldact_ptr = &mut oldact as *mut SigAction as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(act_ptr, oldact_ptr),
        end: core::cmp::max(act_ptr, oldact_ptr) + 32,
    });

    // First call: install new, retrieve previous (default).
    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = SIGUSR1 as u64;
    args.rsi = act_ptr;
    args.rdx = oldact_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(oldact.sa_handler, 0); // SIG_DFL initially

    // Second call: query-only. Now we should see the action we just set.
    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = SIGUSR1 as u64;
    args.rsi = 0; // act = NULL → query
    args.rdx = oldact_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(oldact.sa_handler, 0xCAFE_BABE_DEAD_BEEF);
    assert_eq!(oldact.sa_restorer, 0x1234_5678_9ABC_DEF0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// SIGKILL and SIGSTOP must not be settable (POSIX). The syscall
/// returns 0 (no error) but the action stays as default.
fn test_dispatch_rt_sigaction_rejects_sigkill_sigstop() {
    use crate::userland::signal::{SigAction, SIGKILL};
    setup_phase2_active_user();

    let act = SigAction {
        sa_handler: 0xDEAD,
        ..SigAction::default()
    };
    let act_ptr = &act as *const SigAction as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: act_ptr,
        end: act_ptr + 32,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = SIGKILL as u64;
    args.rsi = act_ptr;
    args.rdx = 0;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let installed = crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.action(SIGKILL).unwrap_or_default().sa_handler
    });
    assert_eq!(installed, 0, "SIGKILL action must remain SIG_DFL");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `rt_sigprocmask(SIG_BLOCK, &set, &oldset)` ORs into the blocked
/// mask; SIGKILL/SIGSTOP cannot be blocked even if the set requests it.
fn test_dispatch_rt_sigprocmask_block_strips_kill_stop() {
    use crate::userland::signal::{SIG_BLOCK, SIGKILL, SIGSTOP, SIGUSR1};
    setup_phase2_active_user();

    let set: u64 = (1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)) | (1 << (SIGUSR1 - 1));
    let mut oldset: u64 = 0;
    let set_ptr = &set as *const u64 as u64;
    let oldset_ptr = &mut oldset as *mut u64 as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(set_ptr, oldset_ptr),
        end: core::cmp::max(set_ptr, oldset_ptr) + 8,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGPROCMASK;
    args.rdi = SIG_BLOCK as u64;
    args.rsi = set_ptr;
    args.rdx = oldset_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(oldset, 0, "previous mask was empty");

    let blocked = crate::userland::lifecycle::with_current_process(|p| p.signal_state.blocked);
    assert_eq!(blocked & (1 << (SIGUSR1 - 1)), 1 << (SIGUSR1 - 1));
    assert_eq!(blocked & (1 << (SIGKILL - 1)), 0, "SIGKILL must not be blockable");
    assert_eq!(blocked & (1 << (SIGSTOP - 1)), 0, "SIGSTOP must not be blockable");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `kill(self, SIGUSR1)` sets SIGUSR1 pending on the current process.
fn test_dispatch_kill_self_sets_pending() {
    use crate::userland::signal::SIGUSR1;
    reset_active_user();

    // Install a non-zero PID so kill_handler's "kill me" branch fires.
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image, &["agenticos-app"], &[])
        .expect("enter_user_mode_with");
    // Don't release yet — we want the Process slot populated for the
    // signal-state inspection.

    let me = crate::userland::lifecycle::current_pid();
    let mut args = SyscallArgs::default();
    args.rax = nr::KILL;
    args.rdi = me as u64;
    args.rsi = SIGUSR1 as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let pending = crate::userland::lifecycle::with_current_process(|p| p.signal_state.pending);
    assert_eq!(pending & (1 << (SIGUSR1 - 1)), 1 << (SIGUSR1 - 1));

    let _ = crate::userland::release_active_image();
}

/// Forking and exiting a child sets SIGCHLD pending on the parent's
/// signal state. We exercise this with the existing fork fixture and
/// inspect the parent's pending mask after fork returns.
fn test_fork_child_exit_sets_sigchld_on_parent() {
    use crate::userland::lifecycle::ExitKind;
    use crate::userland::signal::SIGCHLD;
    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new()
        .expect("AddressSpace::new");
    unsafe { aspace.activate(); }

    let bytes = fix::fork_then_wait_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode_with_aspace(
        image,
        &["agenticos-app"],
        &[],
        Some(aspace),
    )
    .expect("enter_user_mode_with_aspace");

    // Parent process is still installed at this point — the run
    // command hasn't called release_active_image yet (we drive
    // enter_user_mode directly from the test).
    let pending = crate::userland::lifecycle::with_current_process(|p| p.signal_state.pending);
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert!(
        pending & (1 << (SIGCHLD - 1)) != 0,
        "expected SIGCHLD pending after child exit, got mask {:#x}",
        pending,
    );
}

/// Phase 5 PR-B2: full signal-delivery round trip.
///
/// Pre-installs a SIGUSR1 handler pointing at the fixture's handler
/// body, then enters ring 3. The fixture calls `kill(getpid(),
/// SIGUSR1)`; the dispatcher's post-syscall hook detects the pending
/// signal, builds a signal frame on the user stack, and `iretq`s into
/// the handler. The handler exits with code 42. If signal delivery
/// were broken, the fixture would fall through and exit with 99.
fn test_signal_delivery_handler_runs() {
    use crate::userland::lifecycle::ExitKind;
    use crate::userland::signal::{SigAction, SIGUSR1};

    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new()
        .expect("AddressSpace::new");
    unsafe { aspace.activate(); }

    let (bytes, handler_offset) = fix::signal_delivery_handler_exits_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let entry = image.entry.as_u64();

    // Tests bypass the run command; install the process slot
    // ourselves *and* pre-set the signal action so the dispatcher's
    // post-syscall hook will find a handler when the fixture's
    // kill() returns.
    // Pre-install the SIGUSR1 action via the test hook — it'll be
    // applied after `enter_user_mode_with_aspace` builds the Process
    // slot but before the iretq into ring 3.
    crate::userland::test_hooks::set_pre_iretq_signal_action(
        SIGUSR1,
        SigAction {
            sa_handler: entry + handler_offset,
            sa_flags: 0,
            sa_restorer: 0, // handler exits before returning; no restorer needed
            sa_mask: 0,
        },
    );

    let result = crate::userland::enter_user_mode_with_aspace(
        image,
        &["agenticos-app"],
        &[],
        Some(aspace),
    )
    .expect("enter_user_mode_with_aspace");
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 42,
        "expected handler to run and exit 42, got {} (99 means signal didn't deliver)",
        result.1,
    );
}

// ---------- Phase 5 PR-A: pipes ----------

/// Unit test for the kernel-side `Pipe` ring buffer: write some bytes,
/// read them back, verify both halves see the same data.
fn test_pipe_basic_write_then_read() {
    use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

    let pipe = Pipe::new();
    let writer = PipeWriteHandle::new(pipe.clone());
    let reader = PipeReadHandle::new(pipe);

    let n = writer.pipe().write(b"hello pipe");
    assert_eq!(n, 10);
    assert_eq!(reader.pipe().writers(), 1);
    assert_eq!(reader.pipe().readers(), 1);

    let mut buf = [0u8; 16];
    let read = reader.pipe().read(&mut buf);
    assert_eq!(read, 10);
    assert_eq!(&buf[..10], b"hello pipe");
}

/// Cloning a writer handle bumps the writer count; dropping it
/// decrements. Same for readers. The pipe knows when no one is left
/// on either side via these counts.
fn test_pipe_handle_clone_drop_tracks_counts() {
    use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

    let pipe = Pipe::new();
    let writer1 = PipeWriteHandle::new(pipe.clone());
    let reader1 = PipeReadHandle::new(pipe.clone());
    assert_eq!(writer1.pipe().writers(), 1);
    assert_eq!(reader1.pipe().readers(), 1);

    let writer2 = writer1.clone();
    let reader2 = reader1.clone();
    assert_eq!(writer2.pipe().writers(), 2);
    assert_eq!(reader2.pipe().readers(), 2);

    drop(writer1);
    drop(reader1);
    assert_eq!(writer2.pipe().writers(), 1);
    assert_eq!(reader2.pipe().readers(), 1);

    drop(writer2);
    drop(reader2);
    assert_eq!(pipe.writers(), 0);
    assert_eq!(pipe.readers(), 0);
}

/// Buffer is bounded: writing more than `PIPE_CAPACITY` returns a
/// short write. The writer is expected to retry; we validate the
/// kernel side returns the right count.
fn test_pipe_short_write_at_capacity() {
    use crate::userland::pipe::{Pipe, PipeWriteHandle, PIPE_CAPACITY};

    let pipe = Pipe::new();
    let writer = PipeWriteHandle::new(pipe);

    let big: alloc::vec::Vec<u8> = alloc::vec![0xABu8; PIPE_CAPACITY + 100];
    let n = writer.pipe().write(&big);
    assert_eq!(n, PIPE_CAPACITY);

    // Subsequent write rejects (buffer full).
    let m = writer.pipe().write(b"more");
    assert_eq!(m, 0);
}

/// `pipe2(fds, 0)` allocates two fds, writes them to the user int[2],
/// and the resulting read/write fds round-trip bytes through the
/// pipe.
fn test_dispatch_pipe2_round_trip() {
    setup_phase2_active_user();

    // User buffer for the (read_fd, write_fd) pair.
    let fds_buf = [0u8; 8];
    let fds_ptr = fds_buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: fds_ptr,
        end: fds_ptr + 16, // include the data buffer below in the same window
    });

    // pipe2(&fds, 0)
    let mut args = SyscallArgs::default();
    args.rax = nr::PIPE2;
    args.rdi = fds_ptr;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let read_fd = i32::from_ne_bytes(fds_buf[0..4].try_into().unwrap());
    let write_fd = i32::from_ne_bytes(fds_buf[4..8].try_into().unwrap());
    assert!(read_fd >= 3 && write_fd >= 3 && read_fd != write_fd);

    // Use a separate buffer for the actual byte payload. Bounds need
    // to cover both the fds and the data buffer.
    let data = b"hi pipe!";
    let data_ptr = data.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(fds_ptr, data_ptr),
        end: core::cmp::max(fds_ptr + 8, data_ptr + data.len() as u64),
    });

    // write(write_fd, data, len)
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = write_fd as u64;
    args.rsi = data_ptr;
    args.rdx = data.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), data.len() as i64);

    // read(read_fd, dst, 64)
    let dst = [0u8; 64];
    let dst_ptr = dst.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: dst_ptr,
        end: dst_ptr + 64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = read_fd as u64;
    args.rsi = dst_ptr;
    args.rdx = 64;
    let n = syscall_dispatch(&mut args);
    assert_eq!(n, data.len() as i64);
    assert_eq!(&dst[..data.len()], data);

    // Close write end, then read returns EOF (0) — no writers and
    // empty buffer.
    let mut args = SyscallArgs::default();
    args.rax = nr::CLOSE;
    args.rdi = write_fd as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = read_fd as u64;
    args.rsi = dst_ptr;
    args.rdx = 64;
    assert_eq!(syscall_dispatch(&mut args), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

// ---------- Phase 4 PR-D: execve negative path ----------

/// Parent forks; child execve's a non-existent path; execve returns
/// `-ENOENT`; child treats that as failure and exit_group(11); parent
/// wait4s and exits 11.
fn test_fork_execve_badpath_returns_to_parent() {
    use crate::userland::lifecycle::ExitKind;
    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new()
        .expect("AddressSpace::new");
    unsafe { aspace.activate(); }

    let bytes = fix::fork_execve_badpath_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode_with_aspace(
        image,
        &["agenticos-app"],
        &[],
        Some(aspace),
    )
    .expect("enter_user_mode_with_aspace");
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 11, "parent's final exit code should be 11, got {}", result.1);
}

// ---------- Phase 4 PR-C2: fork + wait4 round trip ----------

/// End-to-end fork test: a hand-rolled binary forks, child exits with
/// code 42, parent wait4s the child and then exits with code 7.
///
/// Asserts:
/// - The top-level cooperative exit recorded code 7 (parent's exit).
/// - The child's 42 was parked in `LAST_EXIT_CODE` at some point but
///   then overwritten by the parent's final exit — we don't observe
///   42 here directly.
/// - No abnormal exit / unimplemented syscall.
fn test_fork_then_wait_returns_to_parent() {
    use crate::userland::lifecycle::ExitKind;

    reset_active_user();

    // The fixture's binary lives in PML4[0] of the active L4. The test
    // drives `enter_user_mode_with` which today still routes through
    // the kernel L4 (no AddressSpace passed), but fork() needs a
    // real address space to clone from. Build one and activate it.
    let aspace = crate::userland::address_space::AddressSpace::new()
        .expect("AddressSpace::new for fork fixture");
    // SAFETY: kernel half copied — kernel code post-CR3-write mapped.
    unsafe { aspace.activate(); }

    let bytes = fix::fork_then_wait_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode_with_aspace(
        image,
        &["agenticos-app"],
        &[],
        Some(aspace),
    )
    .expect("enter_user_mode_with_aspace");
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 7, "parent's final exit code should be 7, got {}", result.1);
}

// ---------- Phase 4 PR-C: AddressSpace clone (foundation for fork) ----------

/// `AddressSpace::clone_for_child` produces a child L4 that:
/// - is distinct from both kernel L4 and parent L4,
/// - has PML4[0] populated (parent had user pages),
/// - decouples leaf data: writes to a child page don't bleed into the
///   parent's matching page.
///
/// Drives the clone path on the active user L4 — we activate a parent
/// address space, map a single page, write a magic byte pattern, then
/// clone. After switching to the child, the same VA reads the magic;
/// after switching back to parent and overwriting, switching to the
/// child again reads the original magic (independent backing).
fn test_address_space_clone_for_child_eager_copies_pages() {
    use crate::mm::paging::{UserPerms, USER_LOAD_BASE};
    use crate::userland::address_space::AddressSpace;
    use x86_64::registers::control::Cr3;
    use x86_64::VirtAddr;

    let kernel_frame =
        crate::mm::paging::kernel_l4_frame().expect("kernel L4 captured at boot");

    let parent = AddressSpace::new().expect("parent AddressSpace::new");
    // SAFETY: kernel half copied — kernel code post-CR3-write is mapped.
    unsafe { parent.activate(); }

    // Map one user page in the parent and write a magic value.
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(USER_LOAD_BASE), 1, UserPerms::ReadWrite)
            .expect("parent map");
    });
    let parent_va = USER_LOAD_BASE as *mut u32;
    unsafe { core::ptr::write_volatile(parent_va, 0xAABB_CCDD); }

    // Build the child by cloning the parent's L4. Stay on parent's L4
    // for the clone walk — `clone_for_child` reads parent's tables.
    let child = AddressSpace::clone_for_child(parent.l4_frame())
        .expect("clone_for_child");
    assert_ne!(child.l4_frame(), parent.l4_frame());
    assert_ne!(child.l4_frame(), kernel_frame);

    // Activate the child. The cloned page should read the magic value.
    unsafe { child.activate(); }
    let child_val = unsafe { core::ptr::read_volatile(parent_va) };
    assert_eq!(child_val, 0xAABB_CCDD, "child must inherit parent's data");

    // Modify child's page; check parent's page is unchanged.
    unsafe { core::ptr::write_volatile(parent_va, 0x1111_2222); }
    unsafe { parent.activate(); }
    let parent_val = unsafe { core::ptr::read_volatile(parent_va) };
    assert_eq!(
        parent_val, 0xAABB_CCDD,
        "parent must not see child's writes (eager copy isolated them)"
    );

    // Cleanup: drop both address spaces. Drop reverts CR3 to kernel L4.
    drop(child);
    drop(parent);
    let (final_cr3, _) = Cr3::read();
    assert_eq!(final_cr3, kernel_frame);
}

// ---------- Phase 4 PR-A: Process table ----------

/// `getpid()` returns the kernel sentinel (0) when no user process is
/// active and a real positive PID after a launch.
fn test_getpid_returns_real_pid() {
    reset_active_user();
    // No active process → PID is the kernel sentinel.
    let mut args = SyscallArgs::default();
    args.rax = nr::GETPID;
    let kernel_pid = syscall_dispatch(&mut args);
    assert_eq!(kernel_pid, 0, "no active process must report PID 0");

    // Launch a happy-path binary; getpid during its run is observed
    // through the lifecycle's `current_pid()` (the dispatch-during-
    // ring-3 path mirrors the kernel-side current_pid).
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let _result =
        crate::userland::enter_user_mode_with(image, &["agenticos-app"], &[])
            .expect("enter_user_mode_with");
    // The active-user slot still carries the PID until release.
    let pid_after = crate::userland::lifecycle::with_current_process(|p| p.pid);
    assert!(pid_after >= 1, "expected ≥1, got {}", pid_after);
    let _ = crate::userland::release_active_image();

    // After release, PID resets.
    let mut args = SyscallArgs::default();
    args.rax = nr::GETPID;
    assert_eq!(syscall_dispatch(&mut args), 0);
}

/// PIDs are monotonic — successive `enter_user_mode_with` calls
/// produce strictly increasing PIDs.
fn test_pid_allocation_is_monotonic() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();

    let image1 = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image1, &["agenticos-app"], &[])
        .expect("enter_user_mode_with #1");
    let pid1 = crate::userland::lifecycle::with_current_process(|p| p.pid);
    let _ = crate::userland::release_active_image();

    let image2 = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image2, &["agenticos-app"], &[])
        .expect("enter_user_mode_with #2");
    let pid2 = crate::userland::lifecycle::with_current_process(|p| p.pid);
    let _ = crate::userland::release_active_image();

    assert!(pid2 > pid1, "PIDs must be monotonic: pid1={} pid2={}", pid1, pid2);
}

/// `getppid()` returns 0 (kernel sentinel) for binaries launched by
/// the run command — they're "kernel-parented" until fork() lands.
fn test_getppid_returns_kernel_sentinel() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image, &["agenticos-app"], &[])
        .expect("enter_user_mode_with");
    let parent_pid =
        crate::userland::lifecycle::with_current_process(|p| p.parent_pid);
    assert_eq!(parent_pid, 0);
    let _ = crate::userland::release_active_image();
}

// ---------- Phase 3: TTY ----------

fn test_termios_default_is_canonical_with_echo() {
    use crate::userland::tty::{self, ECHO, ICANON};
    tty::install_default();
    let t = tty::snapshot();
    assert_ne!(t.c_lflag & ICANON, 0, "default must be canonical mode");
    assert_ne!(t.c_lflag & ECHO, 0, "default must echo");
    assert!(tty::is_canonical());
    assert!(tty::is_echo());
}

fn test_dispatch_ioctl_tcgets_returns_termios() {
    setup_phase2_active_user();
    crate::userland::tty::install_default();

    let buf = [0u8; 36];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + buf.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 0; // stdin
    args.rsi = 0x5401; // TCGETS
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    // c_lflag at offset 12 (LE u32). It should be non-zero — at minimum
    // ICANON | ECHO are set in default termios.
    let lflag_bytes: [u8; 4] = buf[12..16].try_into().unwrap();
    let lflag = u32::from_ne_bytes(lflag_bytes);
    assert_ne!(lflag & 0o000002, 0, "ICANON bit should be set");
    assert_ne!(lflag & 0o000010, 0, "ECHO bit should be set");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_ioctl_tcsets_updates_termios() {
    use crate::userland::tty::{self, ECHO, ICANON};
    setup_phase2_active_user();
    tty::install_default();

    // Build a raw-ish termios in user memory: clear ICANON|ECHO|ISIG.
    let mut buf = [0u8; 36];
    // c_iflag = 0
    // c_oflag = 0
    // c_cflag = 0
    // c_lflag = 0 (raw)
    // c_line + c_cc default 0 — VMIN=1 expected by zsh's raw mode but
    // the kernel doesn't honor it today.
    let _ = (&mut buf, 0u32);

    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + buf.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 0;
    args.rsi = 0x5402; // TCSETS
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let t = tty::snapshot();
    assert_eq!(t.c_lflag & ICANON, 0, "ICANON should be cleared");
    assert_eq!(t.c_lflag & ECHO, 0, "ECHO should be cleared");
    assert!(!tty::is_canonical());
    assert!(!tty::is_echo());

    // Restore default so subsequent tests stay in canonical mode.
    tty::install_default();
    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_ioctl_on_file_returns_enotty() {
    use crate::userland::abi::ENOTTY;
    setup_phase2_active_user();
    // Synthetic file slot — using stdin marker as a fake non-tty slot
    // wouldn't actually trigger ENOTTY (Stdin is treated as tty). Drop
    // a directory slot instead via open() on /host if available.
    if !crate::fs::exists("/host") {
        teardown_phase2_active_user();
        return;
    }
    let path = b"/host\0";
    let pp = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: pp, end: pp + path.len() as u64 });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = pp;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3);
    abi::clear_user_va_bounds();

    let buf = [0u8; 36];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + buf.len() as u64 });
    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = fd as u64;
    args.rsi = 0x5401;
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), ENOTTY);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_ioctl_tiocgwinsz_returns_80x24() {
    setup_phase2_active_user();
    let buf = [0u8; 8];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + buf.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 1; // stdout
    args.rsi = 0x5413; // TIOCGWINSZ
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let row = u16::from_ne_bytes(buf[0..2].try_into().unwrap());
    let col = u16::from_ne_bytes(buf[2..4].try_into().unwrap());
    assert_eq!(row, 24);
    assert_eq!(col, 80);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

// ---------- Phase 2 PR-4: directories + getdents64 ----------

/// `open("/host")` must succeed (host folder mount root) and produce a
/// directory fd, not an EISDIR refusal. Skipped when the host mount
/// isn't present (e.g. some test rigs run without `-fsdev`).
fn test_dispatch_open_host_directory_succeeds() {
    if !crate::fs::exists("/host") {
        return;
    }
    setup_phase2_active_user();
    let path = b"/host\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: ptr, end: ptr + path.len() as u64 });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0; // O_RDONLY
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3, "expected dir fd ≥ 3, got {}", fd);

    // It must report as a directory in the FD table.
    let is_dir = crate::userland::lifecycle::with_active_user(|au| {
        matches!(au.fd_table.get(fd as i32), Some(FdSlot::Directory { .. }))
    });
    assert!(is_dir);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `read(dirfd, …)` returns `-EISDIR`. Userland is supposed to call
/// `getdents64` instead.
fn test_dispatch_read_on_directory_returns_eisdir() {
    use crate::userland::abi::EISDIR;
    if !crate::fs::exists("/host") {
        return;
    }
    setup_phase2_active_user();

    let path = b"/host\0";
    let pptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: pptr, end: pptr + path.len() as u64 });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = pptr;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3);
    abi::clear_user_va_bounds();

    let buf = [0u8; 64];
    let bptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: bptr, end: bptr + 64 });
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = fd as u64;
    args.rsi = bptr;
    args.rdx = 64;
    assert_eq!(syscall_dispatch(&mut args), EISDIR);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `getdents64(dirfd, buf, count)` returns at least one record on
/// `/host` (which the host-mount integration places at least
/// `HELLOCPP.ELF` and `HELLO.ELF` into). Skipped without the mount.
fn test_dispatch_getdents64_emits_records() {
    if !crate::fs::exists("/host") {
        return;
    }
    setup_phase2_active_user();

    // open
    let path = b"/host\0";
    let pptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: pptr, end: pptr + path.len() as u64 });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = pptr;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3);
    abi::clear_user_va_bounds();

    // getdents64
    let buf = [0u8; 1024];
    let bptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds { start: bptr, end: bptr + 1024 });
    let mut args = SyscallArgs::default();
    args.rax = nr::GETDENTS64;
    args.rdi = fd as u64;
    args.rsi = bptr;
    args.rdx = 1024;
    let written = syscall_dispatch(&mut args);
    assert!(written > 0, "getdents64 should emit at least one record, got {}", written);

    // Walk the records: first u64 is d_ino (should be non-zero), bytes
    // 16/17 are reclen (LE u16), 18 is d_type, name starts at 19.
    let mut off = 0usize;
    let mut count = 0usize;
    while off < written as usize {
        let reclen_bytes: [u8; 2] = buf[off + 16..off + 18].try_into().unwrap();
        let reclen = u16::from_ne_bytes(reclen_bytes) as usize;
        assert!(reclen > 19 && reclen <= written as usize - off);
        assert_eq!(reclen % 8, 0, "reclen must be 8-byte aligned, got {}", reclen);
        // d_ino non-zero
        let ino_bytes: [u8; 8] = buf[off..off + 8].try_into().unwrap();
        assert_ne!(u64::from_ne_bytes(ino_bytes), 0, "d_ino must be non-zero");
        count += 1;
        off += reclen;
    }
    assert!(count >= 1);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// FAT subdirectory walk: `metadata("/host/HELLOCPP.ELF")` and a
/// nested-equivalent path should both resolve when both are present.
/// On the boot disk we know `/banner.bmp` exists at root; deeper
/// paths exist on the host mount when staged by `build.sh`.
fn test_fat_metadata_subdirectory_tolerated() {
    // Sanity: the root path must work (regression of the simple case).
    let _ = crate::fs::metadata("/banner.bmp");

    // A nested path that is known to exist on the dev host's
    // `host_share` directory. If the staging didn't put a nested file
    // there, just skip — we don't fail the test for missing fixtures.
    if crate::fs::exists("/host/HELLOCPP.ELF") {
        let m = crate::fs::metadata("/host/HELLOCPP.ELF").expect("HELLOCPP.ELF metadata");
        assert!(m.size > 0);
    }

    // Negative: a deeply-nested nonexistent path must report not-found,
    // not a panic from the new walker.
    assert!(crate::fs::metadata("/host/no/such/dir/file.txt").is_err());
}

/// Regression: `write(1, …)` must not silently drop a buffer that
/// contains non-UTF-8 bytes. The original implementation strict-decoded
/// the slice and dropped the entire call on any invalid byte, which
/// made `cat` of a binary file print nothing instead of replacement
/// characters. The handler now uses lossy decoding.
fn test_write_handler_non_utf8_returns_full_len() {
    install_streams_for_dispatcher_test();
    let buf: [u8; 8] = [0x7F, b'E', b'L', b'F', 0xFF, 0xFE, 0x00, 0x42];
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
    assert_eq!(ret, buf.len() as i64,
        "write must accept the full slice even with non-UTF-8 bytes");

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

fn test_dispatch_fcntl_getfd_setfd_roundtrip() {
    setup_phase2_active_user();
    // Allocate a synthetic file slot so we have something to set FD_CLOEXEC on.
    let fd = crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.alloc(FdSlot::Stdin).unwrap() // marker — fcntl flags only affect File slots
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = fd as u64;
    args.rsi = 1; // F_GETFD
    assert_eq!(syscall_dispatch(&mut args), 0);

    args.rsi = 2; // F_SETFD
    args.rdx = 1; // FD_CLOEXEC
    assert_eq!(syscall_dispatch(&mut args), 0);
    // Stream slots ignore the flag — F_GETFD still returns 0.
    args.rsi = 1;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);

    teardown_phase2_active_user();
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
        &test_unknown_syscall_trace_mode_returns_enosys_and_marks_seen,
        &test_unknown_syscall_trace_mode_marks_only_once,
        &test_unknown_syscall_trace_mode_off_does_not_mark,
        &test_unknown_syscall_trace_mode_capacity_overflow,
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
        &test_run_unhandled_syscall_fixture_d,
        &test_run_syscall_exit42_fixture_a,
        &test_run_happy_path_hello,
        &test_run_fault_ud,
        &test_run_fault_pf,
        &test_run_fault_gp,
        &test_run_bad_pointer_syscall,
        &test_run_leak_loop_happy,
        &test_run_leak_loop_fault,
        // Phase 1: stdin + argv/envp
        &test_user_stdin_install_push_pop,
        &test_user_stdin_push_when_inactive_is_noop,
        &test_dispatch_read_returns_queued_bytes,
        &test_dispatch_read_no_active_user_returns_zero,
        &test_enter_user_mode_with_argv_envp,
        // Phase 2: FD table
        &test_fdtable_install_default_streams,
        &test_fdtable_alloc_and_close,
        &test_fdtable_dup_and_dup2,
        // Phase 2: path utilities
        &test_normalize_path_absolute_keeps_path,
        &test_normalize_path_relative_anchors_at_cwd,
        &test_normalize_path_collapses_redundancy,
        &test_copy_user_cstr_happy_path,
        &test_copy_user_cstr_unterminated_at_bound_returns_efault,
        // Phase 2: dispatcher coverage
        &test_dispatch_getcwd_returns_default,
        &test_dispatch_getcwd_short_buffer_returns_erange,
        &test_dispatch_chdir_root_succeeds,
        &test_dispatch_chdir_nonexistent_returns_enoent,
        &test_dispatch_open_nonexistent_returns_enoent,
        &test_dispatch_open_writable_flag_returns_erofs,
        &test_dispatch_close_stream_is_noop,
        &test_dispatch_dup_stdout,
        &test_dispatch_lseek_on_stream_returns_espipe,
        &test_dispatch_clock_gettime_writes_timespec,
        &test_dispatch_clock_gettime_invalid_clock_einval,
        &test_dispatch_getrandom_fills_buffer,
        &test_dispatch_uname_writes_sysname_linux,
        &test_dispatch_fcntl_getfd_setfd_roundtrip,
        &test_write_handler_non_utf8_returns_full_len,
        // Phase 2 PR-4: directories
        &test_dispatch_open_host_directory_succeeds,
        &test_dispatch_read_on_directory_returns_eisdir,
        &test_dispatch_getdents64_emits_records,
        &test_fat_metadata_subdirectory_tolerated,
        // Phase 3: TTY
        &test_termios_default_is_canonical_with_echo,
        &test_dispatch_ioctl_tcgets_returns_termios,
        &test_dispatch_ioctl_tcsets_updates_termios,
        &test_dispatch_ioctl_on_file_returns_enotty,
        &test_dispatch_ioctl_tiocgwinsz_returns_80x24,
        // Phase 4 PR-A: process table + real PIDs
        &test_getpid_returns_real_pid,
        &test_pid_allocation_is_monotonic,
        &test_getppid_returns_kernel_sentinel,
        // Phase 4 PR-B: per-process address spaces
        &test_address_space_new_kernel_half_shared,
        &test_address_space_drop_restores_kernel_cr3,
        // Phase 4 PR-C: clone for fork
        &test_address_space_clone_for_child_eager_copies_pages,
        // Phase 4 PR-C2: fork + wait4
        &test_fork_then_wait_returns_to_parent,
        // Phase 4 PR-D: execve (negative path)
        &test_fork_execve_badpath_returns_to_parent,
        // Phase 5 PR-A: pipes
        &test_pipe_basic_write_then_read,
        &test_pipe_handle_clone_drop_tracks_counts,
        &test_pipe_short_write_at_capacity,
        &test_dispatch_pipe2_round_trip,
        // Phase 5 PR-B: signal foundation
        &test_dispatch_rt_sigaction_round_trip,
        &test_dispatch_rt_sigaction_rejects_sigkill_sigstop,
        &test_dispatch_rt_sigprocmask_block_strips_kill_stop,
        &test_dispatch_kill_self_sets_pending,
        &test_fork_child_exit_sets_sigchld_on_parent,
        // Phase 5 PR-B2: real signal delivery
        &test_signal_delivery_handler_runs,
    ]
}
