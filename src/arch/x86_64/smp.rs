//! Application-processor startup and the shared SMP idle loop.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use x86_64::registers::control::Cr3;
use x86_64::PhysAddr;

use super::acpi::MAX_CPUS;
use crate::{debug_info, debug_warn};

const TRAMPOLINE_PHYS: u64 = 0x8000;
const TRAMPOLINE_VECTOR: u8 = (TRAMPOLINE_PHYS >> 12) as u8;

static CHECKED_IN: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static ONLINE_CPUS: AtomicUsize = AtomicUsize::new(1);
static RELEASE_APS: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test")]
static TEST_AP_DISPATCH_ENABLED: AtomicBool = AtomicBool::new(false);

core::arch::global_asm!(
    r#"
    .section .text.ap_trampoline, "ax"
    .balign 16
    .code16
    .global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7000
    lgdt cs:[0x120]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    .byte 0x66, 0xea
    .long ap_trampoline_protected - ap_trampoline_start
    .word 0x08

    .code32
ap_trampoline_protected:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, cr4
    or eax, 0x20
    mov cr4, eax
    mov eax, dword ptr [0x128]
    mov cr3, eax
    mov ecx, 0xc0000080
    rdmsr
    or eax, 0x00000900
    wrmsr
    mov eax, cr0
    or eax, 0x80000000
    mov cr0, eax
    mov ebp, 0x8000
    .byte 0xea
    .long 0x8000 + (ap_trampoline_long - ap_trampoline_start)
    .word 0x18

    .code64
ap_trampoline_long:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov rsp, qword ptr [rbp + 0x130]
    xor rbp, rbp
    mov edi, dword ptr [0x8140]
    mov rax, qword ptr [0x8138]
    jmp rax

    .org 0x100
ap_trampoline_gdt:
    .quad 0x0000000000000000
    .quad 0x00cf9a008000ffff
    .quad 0x00cf92008000ffff
    .quad 0x00af9a000000ffff
ap_trampoline_gdt_end:
ap_trampoline_gdt_ptr:
    .word 31
    .long 0x8100

    .balign 8
    .global ap_trampoline_cr3
ap_trampoline_cr3:
    .quad 0
    .global ap_trampoline_stack
ap_trampoline_stack:
    .quad 0
    .global ap_trampoline_entry
ap_trampoline_entry:
    .quad 0
    .global ap_trampoline_cpu_id
ap_trampoline_cpu_id:
    .long 0
    .global ap_trampoline_end
ap_trampoline_end:
    .code64
"#
);

unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_trampoline_cr3: u8;
    static ap_trampoline_stack: u8;
    static ap_trampoline_entry: u8;
    static ap_trampoline_cpu_id: u8;
}

pub fn init() {
    CHECKED_IN[0].store(true, Ordering::Release);
    let topology = super::acpi::topology();
    if topology.cpu_count <= 1 || !super::lapic::available() {
        debug_info!("SMP: one CPU active");
        return;
    }
    if !prepare_trampoline() {
        debug_warn!("SMP: trampoline unavailable; AP startup disabled");
        return;
    }

    for logical_id in 1..topology.cpu_count {
        let Some(cpu) = topology.cpu(logical_id) else {
            continue;
        };
        if !patch_trampoline(logical_id) {
            break;
        }
        debug_info!("SMP: starting CPU {} (LAPIC {})", logical_id, cpu.lapic_id);
        super::lapic::send_init(cpu.lapic_id);
        wait_pit_ticks(1);
        super::lapic::send_startup(cpu.lapic_id, TRAMPOLINE_VECTOR);
        if !wait_for_checkin(logical_id, 2) {
            super::lapic::send_startup(cpu.lapic_id, TRAMPOLINE_VECTOR);
        }
        if !wait_for_checkin(logical_id, 100) {
            debug_warn!("SMP: CPU {} failed to check in", logical_id);
        }
    }
    debug_info!(
        "SMP: {}/{} CPU(s) online",
        ONLINE_CPUS.load(Ordering::Acquire),
        topology.cpu_count
    );
}

/// Allow checked-in APs to begin selecting work after BSP boot is complete.
pub fn release_aps() {
    RELEASE_APS.store(true, Ordering::Release);
    notify_work();
}

pub fn online_cpu_count() -> usize {
    ONLINE_CPUS.load(Ordering::Acquire)
}

pub fn notify_work() {
    if !super::lapic::available() {
        return;
    }
    let me = super::percpu::cpu_id();
    let topology = super::acpi::topology();
    for cpu_id in 0..topology.cpu_count {
        if cpu_id == me || !CHECKED_IN[cpu_id].load(Ordering::Acquire) {
            continue;
        }
        if super::percpu::idle_interruptible(cpu_id) {
            super::lapic::send_fixed(
                topology.cpus[cpu_id].lapic_id,
                super::lapic::RESCHEDULE_VECTOR,
            );
            super::percpu::record_reschedule_ipi(cpu_id);
        }
    }
}

/// Wake one logical CPU when runnable work is affinity-pinned to it.
pub fn notify_cpu(cpu_id: usize) {
    if !super::lapic::available()
        || cpu_id == super::percpu::cpu_id()
        || cpu_id >= super::acpi::topology().cpu_count
        || !CHECKED_IN[cpu_id].load(Ordering::Acquire)
    {
        return;
    }
    let topology = super::acpi::topology();
    super::lapic::send_fixed(
        topology.cpus[cpu_id].lapic_id,
        super::lapic::RESCHEDULE_VECTOR,
    );
    super::percpu::record_reschedule_ipi(cpu_id);
}

pub fn freeze_other_cpus() {
    super::lapic::broadcast_halt();
}

fn prepare_trampoline() -> bool {
    let mapped = crate::mm::memory::with_memory_mapper(|mapper| {
        mapper.prepare_trampoline_page(PhysAddr::new(TRAMPOLINE_PHYS))
    });
    if !matches!(mapped, Some(Ok(()))) {
        return false;
    }
    let source = core::ptr::addr_of!(ap_trampoline_start);
    let end = core::ptr::addr_of!(ap_trampoline_end);
    let size = end as usize - source as usize;
    if size > 4096 {
        debug_warn!("SMP trampoline is {} bytes (limit 4096)", size);
        return false;
    }
    let Some(destination) = crate::mm::memory::phys_to_virt(TRAMPOLINE_PHYS) else {
        return false;
    };
    unsafe {
        core::ptr::copy_nonoverlapping(source, destination as *mut u8, size);
    }
    true
}

fn patch_trampoline(logical_id: usize) -> bool {
    let (cr3, _) = Cr3::read();
    let cr3 = cr3.start_address().as_u64();
    if cr3 > u64::from(u32::MAX) {
        debug_warn!("SMP: kernel CR3 is above 4 GiB ({:#x})", cr3);
        return false;
    }
    let stack = super::gdt::kernel_rsp0_top_for(logical_id).as_u64();
    unsafe {
        patch_u64(core::ptr::addr_of!(ap_trampoline_cr3), cr3);
        patch_u64(core::ptr::addr_of!(ap_trampoline_stack), stack);
        patch_u64(
            core::ptr::addr_of!(ap_trampoline_entry),
            ap_main as *const () as u64,
        );
        patch_u32(core::ptr::addr_of!(ap_trampoline_cpu_id), logical_id as u32);
    }
    true
}

unsafe fn patch_u64(symbol: *const u8, value: u64) {
    let offset = symbol as usize - core::ptr::addr_of!(ap_trampoline_start) as usize;
    let target = crate::mm::memory::phys_to_virt(TRAMPOLINE_PHYS).unwrap() + offset as u64;
    core::ptr::write_unaligned(target as *mut u64, value);
}

unsafe fn patch_u32(symbol: *const u8, value: u32) {
    let offset = symbol as usize - core::ptr::addr_of!(ap_trampoline_start) as usize;
    let target = crate::mm::memory::phys_to_virt(TRAMPOLINE_PHYS).unwrap() + offset as u64;
    core::ptr::write_unaligned(target as *mut u32, value);
}

fn wait_pit_ticks(ticks: u64) {
    let deadline = super::interrupts::get_timer_ticks().saturating_add(ticks.max(1));
    while super::interrupts::get_timer_ticks() < deadline {
        core::hint::spin_loop();
    }
}

fn wait_for_checkin(cpu: usize, timeout_ticks: u64) -> bool {
    let deadline = super::interrupts::get_timer_ticks().saturating_add(timeout_ticks);
    while !CHECKED_IN[cpu].load(Ordering::Acquire)
        && super::interrupts::get_timer_ticks() < deadline
    {
        core::hint::spin_loop();
    }
    CHECKED_IN[cpu].load(Ordering::Acquire)
}

#[no_mangle]
extern "C" fn ap_main(logical_id: usize) -> ! {
    let topology = super::acpi::topology();
    let cpu = topology.cpus[logical_id];
    unsafe {
        super::gdt::init_cpu(logical_id);
        super::percpu::init_cpu(
            logical_id,
            cpu.lapic_id,
            super::gdt::kernel_rsp0_top_for(logical_id).as_u64(),
        );
        super::lapic::enable_this_cpu();
    }
    debug_assert_eq!(super::percpu::lapic_id(), cpu.lapic_id);
    debug_assert!(super::percpu::initialized_cpu_count() > logical_id);
    super::fpu::enable_sse();
    super::syscall::init_syscall_msrs();
    super::interrupts::load_idt_on_ap();
    calibrate_lapic_timer();

    CHECKED_IN[logical_id].store(true, Ordering::Release);
    ONLINE_CPUS.fetch_add(1, Ordering::AcqRel);
    x86_64::instructions::interrupts::enable();

    while !RELEASE_APS.load(Ordering::Acquire) {
        x86_64::instructions::hlt();
    }
    idle_loop()
}

fn calibrate_lapic_timer() {
    let start = super::interrupts::get_timer_ticks();
    super::lapic::start_timer_calibration();
    while super::interrupts::get_timer_ticks().saturating_sub(start) < 10 {
        core::hint::spin_loop();
    }
    let elapsed = u32::MAX.saturating_sub(super::lapic::timer_current_count());
    super::lapic::configure_periodic_timer((elapsed / 10).max(1));
}

fn idle_loop() -> ! {
    loop {
        // Most in-kernel unit tests create synthetic scheduler entities that
        // are not valid architecture dispatch targets. Keep APs parked unless
        // the dedicated SMP stress test explicitly opts them in.
        #[cfg(feature = "test")]
        if !TEST_AP_DISPATCH_ENABLED.load(Ordering::Acquire) {
            x86_64::instructions::interrupts::enable_and_hlt();
            continue;
        }

        crate::process::try_run_scheduled_processes();

        x86_64::instructions::interrupts::disable();
        if crate::process::scheduler::SCHEDULER
            .lock()
            .ready_entity_count()
            != 0
        {
            x86_64::instructions::interrupts::enable();
            continue;
        }
        super::percpu::set_idle_interruptible(true);
        if crate::process::scheduler::SCHEDULER
            .lock()
            .ready_entity_count()
            != 0
        {
            super::percpu::set_idle_interruptible(false);
            x86_64::instructions::interrupts::enable();
            continue;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
        super::percpu::set_idle_interruptible(false);
    }
}

#[cfg(feature = "test")]
pub fn set_test_ap_dispatch_enabled(enabled: bool) {
    TEST_AP_DISPATCH_ENABLED.store(enabled, Ordering::Release);
    if enabled {
        notify_work();
    }
}

#[cfg(feature = "test")]
pub fn trampoline_layout_for_test() -> (usize, usize, usize, usize, usize) {
    let start = core::ptr::addr_of!(ap_trampoline_start) as usize;
    (
        core::ptr::addr_of!(ap_trampoline_end) as usize - start,
        core::ptr::addr_of!(ap_trampoline_cr3) as usize - start,
        core::ptr::addr_of!(ap_trampoline_stack) as usize - start,
        core::ptr::addr_of!(ap_trampoline_entry) as usize - start,
        core::ptr::addr_of!(ap_trampoline_cpu_id) as usize - start,
    )
}
