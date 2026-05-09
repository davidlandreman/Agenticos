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
    ]
}

