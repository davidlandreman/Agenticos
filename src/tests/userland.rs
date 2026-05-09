use crate::lib::test_utils::Testable;

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

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_gdt_kernel_selectors,
        &test_gdt_user_selectors,
        &test_tss_loaded,
    ]
}
