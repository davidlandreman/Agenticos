// Per-CPU Global Descriptor Tables and Task State Segments.
//
// Selector layout is a hard interface with the assembly stubs:
// 0x08 kernel code, 0x10 kernel data, 0x18 user data, 0x20 user code,
// 0x28 TSS.

use spin::Once;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

use super::acpi::MAX_CPUS;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PANIC_NMI_IST_INDEX: u16 = 1;

const KERNEL_RSP0_STACK_SIZE: usize = 16 * 1024;
const DOUBLE_FAULT_STACK_SIZE: usize = 4 * 1024;
const PANIC_NMI_STACK_SIZE: usize = 8 * 1024;

#[repr(align(16))]
struct AlignedStack<const N: usize>([u8; N]);

// A CPU initializes and subsequently owns only its indexed stack/TSS slot;
// GDTS publishes the matching immutable descriptor table exactly once.
static mut KERNEL_RSP0_STACKS: [AlignedStack<KERNEL_RSP0_STACK_SIZE>; MAX_CPUS] =
    [const { AlignedStack([0; KERNEL_RSP0_STACK_SIZE]) }; MAX_CPUS];
static mut DOUBLE_FAULT_STACKS: [AlignedStack<DOUBLE_FAULT_STACK_SIZE>; MAX_CPUS] =
    [const { AlignedStack([0; DOUBLE_FAULT_STACK_SIZE]) }; MAX_CPUS];
static mut PANIC_NMI_STACKS: [AlignedStack<PANIC_NMI_STACK_SIZE>; MAX_CPUS] =
    [const { AlignedStack([0; PANIC_NMI_STACK_SIZE]) }; MAX_CPUS];
static mut TSS: [TaskStateSegment; MAX_CPUS] = [const { TaskStateSegment::new() }; MAX_CPUS];

struct CpuGdt {
    table: GlobalDescriptorTable,
    selectors: Selectors,
}

static GDTS: [Once<CpuGdt>; MAX_CPUS] = [const { Once::new() }; MAX_CPUS];

#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub tss: SegmentSelector,
}

pub fn selectors() -> &'static Selectors {
    &GDTS[0].get().expect("BSP GDT not initialized").selectors
}

pub fn user_selectors() -> (u64, u64) {
    let selectors = selectors();
    (
        u64::from(selectors.user_code.0),
        u64::from(selectors.user_data.0),
    )
}

pub fn kernel_rsp0_top() -> VirtAddr {
    kernel_rsp0_top_for(0)
}

pub fn kernel_rsp0_top_for(cpu: usize) -> VirtAddr {
    assert!(cpu < MAX_CPUS);
    let start = unsafe { core::ptr::addr_of!(KERNEL_RSP0_STACKS[cpu]) as u64 };
    VirtAddr::new(start + KERNEL_RSP0_STACK_SIZE as u64)
}

pub unsafe fn set_kernel_rsp0(rsp0: VirtAddr) {
    let cpu = super::percpu::cpu_id();
    TSS[cpu].privilege_stack_table[0] = rsp0;
}

/// Construct and load the calling CPU's private GDT/TSS.
///
/// # Safety
/// `cpu` must be the calling CPU's unique logical slot and may only be
/// initialized once.
pub unsafe fn init_cpu(cpu: usize) {
    assert!(cpu < MAX_CPUS);
    TSS[cpu].privilege_stack_table[0] = kernel_rsp0_top_for(cpu);
    let df_start = core::ptr::addr_of!(DOUBLE_FAULT_STACKS[cpu]) as u64;
    TSS[cpu].interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
        VirtAddr::new(df_start + DOUBLE_FAULT_STACK_SIZE as u64);
    let panic_start = core::ptr::addr_of!(PANIC_NMI_STACKS[cpu]) as u64;
    TSS[cpu].interrupt_stack_table[PANIC_NMI_IST_INDEX as usize] =
        VirtAddr::new(panic_start + PANIC_NMI_STACK_SIZE as u64);

    let tss: &'static TaskStateSegment = &*core::ptr::addr_of!(TSS[cpu]);
    let cpu_gdt = GDTS[cpu].call_once(|| {
        let mut table = GlobalDescriptorTable::new();
        let kernel_code = table.add_entry(Descriptor::kernel_code_segment());
        let kernel_data = table.add_entry(Descriptor::kernel_data_segment());
        let user_data = table.add_entry(Descriptor::user_data_segment());
        let user_code = table.add_entry(Descriptor::user_code_segment());
        let tss = table.add_entry(Descriptor::tss_segment(tss));
        CpuGdt {
            table,
            selectors: Selectors {
                kernel_code,
                kernel_data,
                user_data,
                user_code,
                tss,
            },
        }
    });

    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
    use x86_64::instructions::tables::load_tss;

    cpu_gdt.table.load();
    let selectors = &cpu_gdt.selectors;
    CS::set_reg(selectors.kernel_code);
    SS::set_reg(selectors.kernel_data);
    DS::set_reg(selectors.kernel_data);
    ES::set_reg(selectors.kernel_data);
    load_tss(selectors.tss);
}

pub fn init() {
    unsafe { init_cpu(0) }
}
