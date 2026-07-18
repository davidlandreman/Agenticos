//! Local xAPIC MMIO driver.

use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::registers::model_specific::Msr;
use x86_64::{PhysAddr, VirtAddr};

use crate::{debug_info, debug_warn};

pub const RESCHEDULE_VECTOR: u8 = 0xf0;
pub const HALT_VECTOR: u8 = 0xf1;
pub const ERROR_VECTOR: u8 = 0xfe;
pub const SPURIOUS_VECTOR: u8 = 0xff;
pub const LAPIC_TIMER_VECTOR: u8 = 0xef;

pub const LAPIC_VIRT_BASE: u64 = 0x0000_5580_0000_0000;

const APIC_BASE_MSR: u32 = 0x1b;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const REG_ID: u32 = 0x020;
const REG_TPR: u32 = 0x080;
const REG_EOI: u32 = 0x0b0;
const REG_LDR: u32 = 0x0d0;
const REG_DFR: u32 = 0x0e0;
const REG_SVR: u32 = 0x0f0;
const REG_ESR: u32 = 0x280;
const REG_ICR_LOW: u32 = 0x300;
const REG_ICR_HIGH: u32 = 0x310;
const REG_LVT_TIMER: u32 = 0x320;
const REG_LVT_ERROR: u32 = 0x370;
const REG_TIMER_INITIAL: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3e0;

const LVT_MASKED: u32 = 1 << 16;
const TIMER_PERIODIC: u32 = 1 << 17;
const ICR_DELIVERY_PENDING: u32 = 1 << 12;

static BASE: AtomicU64 = AtomicU64::new(0);

pub fn init(physical_base: u64) -> bool {
    let physical_base = physical_base & !0xfff;
    let mapped = crate::mm::memory::with_memory_mapper(|mapper| {
        mapper.map_mmio_page(VirtAddr::new(LAPIC_VIRT_BASE), PhysAddr::new(physical_base))
    });
    if !matches!(mapped, Some(Ok(()))) {
        debug_warn!("failed to map LAPIC MMIO page at {:#x}", physical_base);
        return false;
    }
    BASE.store(LAPIC_VIRT_BASE, Ordering::Release);
    unsafe { enable_this_cpu() };
    debug_info!("local APIC enabled (id {})", id());
    true
}

/// Enable the already-mapped LAPIC on the calling CPU.
pub unsafe fn enable_this_cpu() {
    let mut apic_base = Msr::new(APIC_BASE_MSR);
    let value = apic_base.read() | APIC_BASE_ENABLE;
    apic_base.write(value);

    write(REG_TPR, 0);
    write(REG_DFR, u32::MAX);
    write(REG_LDR, 1 << 24);
    write(REG_LVT_TIMER, LVT_MASKED | u32::from(LAPIC_TIMER_VECTOR));
    write(REG_LVT_ERROR, u32::from(ERROR_VECTOR));
    write(REG_ESR, 0);
    write(REG_ESR, 0);
    write(REG_EOI, 0);
    write(REG_SVR, 0x100 | u32::from(SPURIOUS_VECTOR));
}

pub fn available() -> bool {
    BASE.load(Ordering::Acquire) != 0
}

pub fn id() -> u8 {
    unsafe { (read(REG_ID) >> 24) as u8 }
}

#[inline]
pub fn eoi() {
    if available() {
        unsafe { write(REG_EOI, 0) };
    }
}

pub fn send_fixed(apic_id: u8, vector: u8) {
    unsafe { send_ipi(apic_id, u32::from(vector)) }
}

pub fn send_init(apic_id: u8) {
    unsafe {
        // INIT, level-triggered assert followed by deassert.
        send_ipi(apic_id, 0x0000_c500);
        send_ipi(apic_id, 0x0000_8500);
    }
}

pub fn send_startup(apic_id: u8, page_vector: u8) {
    unsafe { send_ipi(apic_id, 0x0000_0600 | u32::from(page_vector)) }
}

pub fn broadcast_halt() {
    if !available() {
        return;
    }
    unsafe {
        wait_icr();
        // Fixed delivery, all excluding self destination shorthand.
        write(REG_ICR_LOW, (3 << 18) | u32::from(HALT_VECTOR));
        wait_icr();
    }
}

pub fn configure_periodic_timer(initial_count: u32) {
    unsafe {
        // Divide by 16.
        write(REG_TIMER_DIVIDE, 0x3);
        write(
            REG_LVT_TIMER,
            TIMER_PERIODIC | u32::from(LAPIC_TIMER_VECTOR),
        );
        write(REG_TIMER_INITIAL, initial_count.max(1));
    }
}

pub fn start_timer_calibration() {
    unsafe {
        write(REG_TIMER_DIVIDE, 0x3);
        write(REG_LVT_TIMER, LVT_MASKED | u32::from(LAPIC_TIMER_VECTOR));
        write(REG_TIMER_INITIAL, u32::MAX);
    }
}

pub fn timer_current_count() -> u32 {
    unsafe { read(REG_TIMER_CURRENT) }
}

unsafe fn send_ipi(apic_id: u8, low: u32) {
    wait_icr();
    write(REG_ICR_HIGH, u32::from(apic_id) << 24);
    write(REG_ICR_LOW, low);
    wait_icr();
}

unsafe fn wait_icr() {
    while read(REG_ICR_LOW) & ICR_DELIVERY_PENDING != 0 {
        core::hint::spin_loop();
    }
}

#[inline]
unsafe fn read(register: u32) -> u32 {
    let base = BASE.load(Ordering::Acquire);
    core::ptr::read_volatile((base + u64::from(register)) as *const u32)
}

#[inline]
unsafe fn write(register: u32, value: u32) {
    let base = BASE.load(Ordering::Acquire);
    core::ptr::write_volatile((base + u64::from(register)) as *mut u32, value);
}
