//! I/O APIC routing with all external interrupts pinned to the BSP.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use x86_64::{PhysAddr, VirtAddr};

use super::acpi::CpuTopology;
use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::{debug_info, debug_warn};

pub const IOAPIC_VIRT_BASE: u64 = super::lapic::LAPIC_VIRT_BASE + 0x1000;

const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;
const REG_VERSION: u8 = 0x01;
const REDIRECTION_BASE: u8 = 0x10;
const REDIRECTION_MASKED: u32 = 1 << 16;

static BASE: AtomicU64 = AtomicU64::new(0);
static GSI_BASE: AtomicU32 = AtomicU32::new(0);
static MAX_REDIRECTIONS: AtomicU32 = AtomicU32::new(0);
static ACCESS: InterruptMutex<()> = InterruptMutex::new(());

pub fn init(topology: &CpuTopology) -> bool {
    let Some(info) = topology.ioapic else {
        return false;
    };
    let mapped = crate::mm::memory::with_memory_mapper(|mapper| {
        mapper.map_mmio_page(
            VirtAddr::new(IOAPIC_VIRT_BASE),
            PhysAddr::new(u64::from(info.address)),
        )
    });
    if !matches!(mapped, Some(Ok(()))) {
        debug_warn!("failed to map IOAPIC MMIO page at {:#x}", info.address);
        return false;
    }
    BASE.store(IOAPIC_VIRT_BASE, Ordering::Release);
    GSI_BASE.store(info.gsi_base, Ordering::Release);
    let maximum = ((read(REG_VERSION) >> 16) & 0xff) + 1;
    MAX_REDIRECTIONS.store(maximum, Ordering::Release);

    for index in 0..maximum {
        write_redirection(index, 0, REDIRECTION_MASKED);
    }
    for irq in [0u8, 1, 12] {
        route_isa_irq(topology, irq, 32 + irq, topology.bsp_lapic_id, false);
    }
    debug_info!(
        "IOAPIC {} enabled with {} redirection entries at GSI {}",
        info.id,
        maximum,
        info.gsi_base
    );
    true
}

pub fn route_pci_irq(irq: u8, bsp_lapic_id: u8) -> bool {
    // PCI INTx is active-low and level-triggered. This is load-bearing for
    // drivers whose ISR uses try_lock: if an interrupt observes brief lock
    // contention and returns without acknowledging the device, the asserted
    // line must retrigger after LAPIC EOI instead of being lost as an edge.
    route_gsi(u32::from(irq), 32 + irq, bsp_lapic_id, true, true, false)
}

fn route_isa_irq(
    topology: &CpuTopology,
    irq: u8,
    vector: u8,
    destination: u8,
    masked: bool,
) -> bool {
    let override_entry = topology.override_for_irq(irq);
    let gsi = override_entry
        .map(|entry| entry.gsi)
        .unwrap_or(u32::from(irq));
    let flags = override_entry.map(|entry| entry.flags).unwrap_or(0);
    let active_low = flags & 0b11 == 0b11;
    let level_triggered = (flags >> 2) & 0b11 == 0b11;
    route_gsi(
        gsi,
        vector,
        destination,
        active_low,
        level_triggered,
        masked,
    )
}

fn route_gsi(
    gsi: u32,
    vector: u8,
    destination: u8,
    active_low: bool,
    level_triggered: bool,
    masked: bool,
) -> bool {
    let base = GSI_BASE.load(Ordering::Acquire);
    let Some(index) = gsi.checked_sub(base) else {
        return false;
    };
    if index >= MAX_REDIRECTIONS.load(Ordering::Acquire) {
        return false;
    }
    let mut low = u32::from(vector);
    if active_low {
        low |= 1 << 13;
    }
    if level_triggered {
        low |= 1 << 15;
    }
    if masked {
        low |= REDIRECTION_MASKED;
    }
    write_redirection(index, u32::from(destination) << 24, low);
    true
}

fn write_redirection(index: u32, high: u32, low: u32) {
    let register = REDIRECTION_BASE as u32 + index * 2;
    write((register + 1) as u8, high);
    write(register as u8, low);
}

fn read(register: u8) -> u32 {
    let _guard = ACCESS.lock();
    unsafe {
        let base = BASE.load(Ordering::Acquire);
        core::ptr::write_volatile((base + IOREGSEL) as *mut u32, u32::from(register));
        core::ptr::read_volatile((base + IOWIN) as *const u32)
    }
}

fn write(register: u8, value: u32) {
    let _guard = ACCESS.lock();
    unsafe {
        let base = BASE.load(Ordering::Acquire);
        core::ptr::write_volatile((base + IOREGSEL) as *mut u32, u32::from(register));
        core::ptr::write_volatile((base + IOWIN) as *mut u32, value);
    }
}
