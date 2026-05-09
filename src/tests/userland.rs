use crate::lib::test_utils::Testable;
use crate::mm::paging::{
    UserMapError, UserPerms, USER_LOAD_BASE, USER_VA_RANGE_END, USER_VA_RANGE_START,
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
    ]
}
