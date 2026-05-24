// Global Descriptor Table and Task State Segment for AgenticOS.
//
// Layout is a hard interface: the kernel CS=0x08 and SS=0x10 selectors are
// hard-coded as literal pushes in `src/arch/x86_64/preemption.rs` and
// `src/arch/x86_64/context_switch.rs`. Any reordering of the descriptors
// below WILL break those naked-asm blocks. The user_data-before-user_code
// order also keeps the door open for `syscall`/`sysret` later, which derives
// user_cs = STAR[63:48] + 16 and user_ss = STAR[63:48] + 8 by formula.
//
// Slot map:
//   0x00  null
//   0x08  kernel code (DPL 0, 64-bit)
//   0x10  kernel data (DPL 0)
//   0x18  user data   (DPL 3)
//   0x20  user code   (DPL 3, 64-bit)
//   0x28  TSS         (system descriptor, occupies two GDT slots)

use lazy_static::lazy_static;
use x86_64::VirtAddr;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

/// IST entry index used by the double-fault handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

// Phase 5 PR-C1 gives each user process its own 16 KiB kernel stack,
// so this global stack is only used between user-process activations
// (kernel main loop and one-off kernel syscalls). 16 KiB is plenty.
const KERNEL_RSP0_STACK_SIZE: usize = 16 * 1024;
const DOUBLE_FAULT_STACK_SIZE: usize = 4 * 1024;

#[repr(align(16))]
struct AlignedStack<const N: usize>([u8; N]);

static mut KERNEL_RSP0_STACK: AlignedStack<KERNEL_RSP0_STACK_SIZE> =
    AlignedStack([0; KERNEL_RSP0_STACK_SIZE]);

static mut DOUBLE_FAULT_STACK: AlignedStack<DOUBLE_FAULT_STACK_SIZE> =
    AlignedStack([0; DOUBLE_FAULT_STACK_SIZE]);

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.privilege_stack_table[0] = {
            let stack_start = VirtAddr::from_ptr(&raw const KERNEL_RSP0_STACK);
            stack_start + KERNEL_RSP0_STACK_SIZE as u64
        };
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
            stack_start + DOUBLE_FAULT_STACK_SIZE as u64
        };
        tss
    };

    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.add_entry(Descriptor::kernel_code_segment());
        let kernel_data = gdt.add_entry(Descriptor::kernel_data_segment());
        let user_data = gdt.add_entry(Descriptor::user_data_segment());
        let user_code = gdt.add_entry(Descriptor::user_code_segment());
        let tss = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (
            gdt,
            Selectors {
                kernel_code,
                kernel_data,
                user_data,
                user_code,
                tss,
            },
        )
    };
}

#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub tss: SegmentSelector,
}

/// Selector accessors for code that needs to push selectors onto an iretq frame.
pub fn selectors() -> &'static Selectors {
    &GDT.1
}

/// Ring-3 (CS, SS) pair, with RPL=3, ready to push onto an iretq frame.
/// Today's GDT layout yields `(0x23, 0x1B)`; deriving from `selectors()`
/// keeps the call site honest if the layout ever shifts. Used by U4's
/// `resume_ring3` wrapper and any other code building a ring-3 iretq
/// frame from scratch.
pub fn user_selectors() -> (u64, u64) {
    let s = selectors();
    (u64::from(s.user_code.0), u64::from(s.user_data.0))
}

/// Top of the static kernel rsp0 stack — the value the userland subsystem
/// stamps into `TSS.privilege_stack_table[0]` before entering ring 3 (D6).
///
/// Single-app-synchronous (D5) means we always switch *back* to this same
/// stack on a ring 3 → ring 0 transition; multiplexing per-process rsp0 is
/// out of scope.
pub fn kernel_rsp0_top() -> VirtAddr {
    let stack_start = VirtAddr::from_ptr(&raw const KERNEL_RSP0_STACK);
    stack_start + KERNEL_RSP0_STACK_SIZE as u64
}

/// Update `TSS.privilege_stack_table[0]` (rsp0). Call before entering ring 3.
///
/// SAFETY: the caller must ensure no ring 3 → ring 0 transition is in flight
/// while this write is observable in a partial state. Practically this means:
/// call from CPL=0 with interrupts disabled (or, equivalently, from a context
/// where no user app is currently active — i.e., from `enter_user_mode` just
/// before issuing `iretq`). Single-app-synchronous (D5) makes the policy
/// simple: rsp0 is set once per `run` and not touched again until exit.
///
/// We must mutate a `lazy_static` which exposes `&'static TaskStateSegment`
/// — we go through a raw pointer cast rather than `UnsafeCell` to keep the
/// `lazy_static` ergonomic. The cast is sound because the static lives for
/// `'static` and the field is `Copy`.
pub unsafe fn set_kernel_rsp0(rsp0: VirtAddr) {
    let tss_ptr = &*TSS as *const TaskStateSegment as *mut TaskStateSegment;
    (*tss_ptr).privilege_stack_table[0] = rsp0;
}

/// Install the GDT, reload all segment registers, and load the TSS.
///
/// Must run before any code path that depends on the new selectors — in
/// particular before the IDT is loaded with handlers that reference IST entries
/// or before any ring-0/ring-3 transition can occur.
pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    let sel = &GDT.1;

    unsafe {
        CS::set_reg(sel.kernel_code);
        SS::set_reg(sel.kernel_data);
        DS::set_reg(sel.kernel_data);
        ES::set_reg(sel.kernel_data);
        load_tss(sel.tss);
    }
}
