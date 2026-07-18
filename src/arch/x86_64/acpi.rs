//! Minimal ACPI discovery used by the SMP and interrupt-controller bring-up.
//!
//! We intentionally parse only the tables the kernel consumes: RSDP,
//! RSDT/XSDT, and the MADT's processor, I/O APIC, and interrupt-source
//! override records.  The parser is allocation-free so discovery can run
//! before the heap is available.

use spin::Once;

use crate::{debug_info, debug_warn};

pub const MAX_CPUS: usize = 8;
pub const MAX_INTERRUPT_OVERRIDES: usize = 16;

const SDT_HEADER_LEN: usize = 36;
const MADT_FIXED_LEN: usize = SDT_HEADER_LEN + 8;
const MAX_ACPI_TABLE_LEN: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuInfo {
    pub acpi_id: u8,
    pub lapic_id: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoApicInfo {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterruptSourceOverride {
    pub source_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct CpuTopology {
    pub bsp_lapic_id: u8,
    pub cpus: [CpuInfo; MAX_CPUS],
    pub cpu_count: usize,
    pub ioapic: Option<IoApicInfo>,
    pub interrupt_overrides: [InterruptSourceOverride; MAX_INTERRUPT_OVERRIDES],
    pub interrupt_override_count: usize,
    pub lapic_mmio_base: u64,
}

impl CpuTopology {
    pub const fn fallback(bsp_lapic_id: u8) -> Self {
        let mut cpus = [CpuInfo {
            acpi_id: 0,
            lapic_id: 0,
        }; MAX_CPUS];
        cpus[0] = CpuInfo {
            acpi_id: 0,
            lapic_id: bsp_lapic_id,
        };
        Self {
            bsp_lapic_id,
            cpus,
            cpu_count: 1,
            ioapic: None,
            interrupt_overrides: [InterruptSourceOverride {
                source_irq: 0,
                gsi: 0,
                flags: 0,
            }; MAX_INTERRUPT_OVERRIDES],
            interrupt_override_count: 0,
            lapic_mmio_base: 0xfee0_0000,
        }
    }

    pub fn cpu(&self, index: usize) -> Option<CpuInfo> {
        (index < self.cpu_count).then_some(self.cpus[index])
    }

    pub fn override_for_irq(&self, irq: u8) -> Option<InterruptSourceOverride> {
        self.interrupt_overrides[..self.interrupt_override_count]
            .iter()
            .copied()
            .find(|entry| entry.source_irq == irq)
    }
}

static TOPOLOGY: Once<CpuTopology> = Once::new();

pub fn topology() -> &'static CpuTopology {
    TOPOLOGY
        .get()
        .expect("ACPI topology accessed before acpi::init")
}

pub fn init(rsdp_addr: Option<u64>) -> &'static CpuTopology {
    TOPOLOGY.call_once(|| {
        let bsp = bsp_lapic_id();
        let Some(rsdp_addr) = rsdp_addr else {
            debug_warn!("ACPI RSDP unavailable; using one-CPU PIC fallback");
            return CpuTopology::fallback(bsp);
        };
        let parsed = unsafe { discover(rsdp_addr, bsp) };
        match parsed {
            Some(topology) => {
                debug_info!(
                    "ACPI MADT: {} CPU(s), BSP LAPIC {}, LAPIC {:#x}, IOAPIC {:?}",
                    topology.cpu_count,
                    topology.bsp_lapic_id,
                    topology.lapic_mmio_base,
                    topology.ioapic
                );
                topology
            }
            None => {
                debug_warn!("ACPI MADT invalid or absent; using one-CPU PIC fallback");
                CpuTopology::fallback(bsp)
            }
        }
    })
}

fn bsp_lapic_id() -> u8 {
    // CPUID.1:EBX[31:24] is the initial xAPIC ID.
    unsafe { (core::arch::x86_64::__cpuid(1).ebx >> 24) as u8 }
}

unsafe fn discover(rsdp_phys: u64, bsp_lapic_id: u8) -> Option<CpuTopology> {
    if read_bytes(rsdp_phys, 8)? != b"RSD PTR " || !checksum_phys(rsdp_phys, 20)? {
        return None;
    }
    let revision = read_u8(rsdp_phys + 15)?;
    let (root_phys, entry_width) = if revision >= 2 {
        let length = read_u32(rsdp_phys + 20)? as usize;
        if !(36..=4096).contains(&length) || !checksum_phys(rsdp_phys, length)? {
            return None;
        }
        (read_u64(rsdp_phys + 24)?, 8usize)
    } else {
        (read_u32(rsdp_phys + 16)? as u64, 4usize)
    };

    let expected = if entry_width == 8 { b"XSDT" } else { b"RSDT" };
    if read_bytes(root_phys, 4)? != expected {
        return None;
    }
    let root_len = table_length(root_phys)?;
    if !checksum_phys(root_phys, root_len)? || root_len < SDT_HEADER_LEN {
        return None;
    }

    let entries = (root_len - SDT_HEADER_LEN) / entry_width;
    for index in 0..entries {
        let addr = root_phys + SDT_HEADER_LEN as u64 + (index * entry_width) as u64;
        let table_phys = if entry_width == 8 {
            read_u64(addr)?
        } else {
            read_u32(addr)? as u64
        };
        if read_bytes(table_phys, 4)? == b"APIC" {
            let len = table_length(table_phys)?;
            if checksum_phys(table_phys, len)? {
                return parse_madt_phys(table_phys, len, bsp_lapic_id);
            }
        }
    }
    None
}

unsafe fn parse_madt_phys(table_phys: u64, len: usize, bsp_lapic_id: u8) -> Option<CpuTopology> {
    if len < MADT_FIXED_LEN || len > MAX_ACPI_TABLE_LEN {
        return None;
    }
    let ptr = phys_ptr(table_phys)?;
    let bytes = core::slice::from_raw_parts(ptr, len);
    parse_madt(bytes, bsp_lapic_id)
}

/// Parse a complete, checksum-validatable MADT byte image.
pub(crate) fn parse_madt(bytes: &[u8], bsp_lapic_id: u8) -> Option<CpuTopology> {
    if bytes.len() < MADT_FIXED_LEN
        || bytes.get(0..4)? != b"APIC"
        || read_le_u32(bytes, 4)? as usize != bytes.len()
        || checksum(bytes) != 0
    {
        return None;
    }

    let mut result = CpuTopology::fallback(bsp_lapic_id);
    result.cpu_count = 0;
    result.ioapic = None;
    result.lapic_mmio_base = read_le_u32(bytes, SDT_HEADER_LEN)? as u64;

    let mut offset = MADT_FIXED_LEN;
    while offset < bytes.len() {
        let kind = *bytes.get(offset)?;
        let entry_len = *bytes.get(offset + 1)? as usize;
        if entry_len < 2 || offset.checked_add(entry_len)? > bytes.len() {
            return None;
        }
        let entry = &bytes[offset..offset + entry_len];
        match kind {
            0 if entry_len >= 8 => {
                let flags = read_le_u32(entry, 4)?;
                if flags & 0x3 != 0 && result.cpu_count < MAX_CPUS {
                    result.cpus[result.cpu_count] = CpuInfo {
                        acpi_id: entry[2],
                        lapic_id: entry[3],
                    };
                    result.cpu_count += 1;
                }
            }
            1 if entry_len >= 12 && result.ioapic.is_none() => {
                result.ioapic = Some(IoApicInfo {
                    id: entry[2],
                    address: read_le_u32(entry, 4)?,
                    gsi_base: read_le_u32(entry, 8)?,
                });
            }
            2 if entry_len >= 10 && entry[2] == 0 => {
                if result.interrupt_override_count < MAX_INTERRUPT_OVERRIDES {
                    result.interrupt_overrides[result.interrupt_override_count] =
                        InterruptSourceOverride {
                            source_irq: entry[3],
                            gsi: read_le_u32(entry, 4)?,
                            flags: read_le_u16(entry, 8)?,
                        };
                    result.interrupt_override_count += 1;
                }
            }
            5 if entry_len >= 12 => {
                result.lapic_mmio_base = read_le_u64(entry, 4)?;
            }
            _ => {}
        }
        offset += entry_len;
    }

    if result.cpu_count == 0 {
        result.cpus[0] = CpuInfo {
            acpi_id: 0,
            lapic_id: bsp_lapic_id,
        };
        result.cpu_count = 1;
    } else if let Some(index) = result.cpus[..result.cpu_count]
        .iter()
        .position(|cpu| cpu.lapic_id == bsp_lapic_id)
    {
        result.cpus.swap(0, index);
    }
    Some(result)
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

unsafe fn checksum_phys(phys: u64, len: usize) -> Option<bool> {
    if len > MAX_ACPI_TABLE_LEN {
        return None;
    }
    let ptr = phys_ptr(phys)?;
    Some(checksum(core::slice::from_raw_parts(ptr, len)) == 0)
}

unsafe fn table_length(phys: u64) -> Option<usize> {
    let len = read_u32(phys + 4)? as usize;
    (SDT_HEADER_LEN..=MAX_ACPI_TABLE_LEN)
        .contains(&len)
        .then_some(len)
}

unsafe fn phys_ptr(phys: u64) -> Option<*const u8> {
    crate::mm::memory::phys_to_virt(phys).map(|virt| virt as *const u8)
}

unsafe fn read_bytes(phys: u64, len: usize) -> Option<&'static [u8]> {
    Some(core::slice::from_raw_parts(phys_ptr(phys)?, len))
}

unsafe fn read_u8(phys: u64) -> Option<u8> {
    Some(core::ptr::read_unaligned(phys_ptr(phys)?))
}

unsafe fn read_u32(phys: u64) -> Option<u32> {
    Some(core::ptr::read_unaligned(phys_ptr(phys)? as *const u32))
}

unsafe fn read_u64(phys: u64) -> Option<u64> {
    Some(core::ptr::read_unaligned(phys_ptr(phys)? as *const u64))
}

fn read_le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_le_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}
