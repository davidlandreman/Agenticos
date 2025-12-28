//! VirtIO Common Types and Virtqueue Implementation
//!
//! Implements the VirtIO 1.0 specification for virtqueues and common device operations.
//! Supports modern VirtIO devices using MMIO through PCI capabilities.

use core::sync::atomic::{AtomicU16, Ordering};
use core::ptr::{read_volatile, write_volatile};
use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::drivers::pci::{PciDevice, Bar};
use crate::mm::memory::phys_to_virt;
use crate::mm::paging::translate_virt_to_phys;
use crate::debug_info;
use crate::debug_trace;

/// VirtIO device status bits
pub mod status {
    pub const ACKNOWLEDGE: u8 = 1;
    pub const DRIVER: u8 = 2;
    pub const DRIVER_OK: u8 = 4;
    pub const FEATURES_OK: u8 = 8;
    pub const DEVICE_NEEDS_RESET: u8 = 64;
    pub const FAILED: u8 = 128;
}

/// VirtIO PCI capability types
mod cap_type {
    pub const COMMON_CFG: u8 = 1;
    pub const NOTIFY_CFG: u8 = 2;
    pub const ISR_CFG: u8 = 3;
    pub const DEVICE_CFG: u8 = 4;
}

/// VirtIO common configuration structure offsets
mod common_cfg {
    pub const DEVICE_FEATURE_SELECT: usize = 0x00;
    pub const DEVICE_FEATURE: usize = 0x04;
    pub const DRIVER_FEATURE_SELECT: usize = 0x08;
    pub const DRIVER_FEATURE: usize = 0x0C;
    pub const MSIX_CONFIG: usize = 0x10;
    pub const NUM_QUEUES: usize = 0x12;
    pub const DEVICE_STATUS: usize = 0x14;
    pub const CONFIG_GENERATION: usize = 0x15;
    pub const QUEUE_SELECT: usize = 0x16;
    pub const QUEUE_SIZE: usize = 0x18;
    pub const QUEUE_MSIX_VECTOR: usize = 0x1A;
    pub const QUEUE_ENABLE: usize = 0x1C;
    pub const QUEUE_NOTIFY_OFF: usize = 0x1E;
    pub const QUEUE_DESC: usize = 0x20;
    pub const QUEUE_DRIVER: usize = 0x28;
    pub const QUEUE_DEVICE: usize = 0x30;
}

/// Virtqueue descriptor flags
pub mod desc_flags {
    pub const NEXT: u16 = 1;
    pub const WRITE: u16 = 2;
    pub const INDIRECT: u16 = 4;
}

/// Virtqueue descriptor
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// Virtqueue used ring element
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// Aligned available ring structure
#[repr(C, align(2))]
pub struct VirtqAvailRing {
    pub flags: u16,
    pub idx: AtomicU16,
    pub ring: [u16; 256],
}

/// Aligned used ring structure
#[repr(C, align(4))]
pub struct VirtqUsedRing {
    pub flags: u16,
    pub idx: AtomicU16,
    pub ring: [VirtqUsedElem; 256],
}

/// Information about VirtIO capability locations
#[derive(Debug, Clone, Copy, Default)]
struct VirtioCaps {
    common_cfg_bar: u8,
    common_cfg_offset: u32,
    notify_bar: u8,
    notify_offset: u32,
    notify_multiplier: u32,
    isr_bar: u8,
    isr_offset: u32,
    device_cfg_bar: u8,
    device_cfg_offset: u32,
}

/// A virtqueue for communication with the device (modern mode with MMIO)
pub struct Virtqueue {
    /// Queue size (number of descriptors)
    pub size: u16,
    /// Descriptor table
    descriptors: Box<[VirtqDesc]>,
    /// Available ring
    avail: Box<VirtqAvailRing>,
    /// Used ring
    used: Box<VirtqUsedRing>,
    /// Next free descriptor index
    free_head: u16,
    /// Number of free descriptors
    num_free: u16,
    /// Last seen used index
    last_used_idx: u16,
    /// Queue index
    queue_idx: u16,
    /// Notify address for this queue
    notify_addr: *mut u16,
}

// SAFETY: The notify_addr pointer points to MMIO space which is globally accessible.
// Access is controlled by the Mutex wrapping VirtioTablet.
unsafe impl Send for Virtqueue {}
unsafe impl Sync for Virtqueue {}

impl Virtqueue {
    /// Create a new virtqueue with the given size
    pub fn new(size: u16, queue_idx: u16, notify_addr: *mut u16) -> Self {
        let mut descriptors = Vec::with_capacity(size as usize);
        for i in 0..size {
            descriptors.push(VirtqDesc {
                addr: 0,
                len: 0,
                flags: 0,
                next: if i + 1 < size { i + 1 } else { 0 },
            });
        }

        let avail = Box::new(VirtqAvailRing {
            flags: 0,
            idx: AtomicU16::new(0),
            ring: [0; 256],
        });

        let used = Box::new(VirtqUsedRing {
            flags: 0,
            idx: AtomicU16::new(0),
            ring: [VirtqUsedElem::default(); 256],
        });

        Self {
            size,
            descriptors: descriptors.into_boxed_slice(),
            avail,
            used,
            free_head: 0,
            num_free: size,
            last_used_idx: 0,
            queue_idx,
            notify_addr,
        }
    }

    /// Get descriptor table physical address
    pub fn desc_phys_addr(&self) -> u64 {
        let virt = self.descriptors.as_ptr() as u64;
        translate_virt_to_phys(virt).unwrap_or_else(|| {
            debug_info!("Warning: Could not translate desc addr 0x{:x}", virt);
            virt
        })
    }

    /// Get available ring physical address
    pub fn avail_phys_addr(&self) -> u64 {
        let virt = &*self.avail as *const _ as u64;
        translate_virt_to_phys(virt).unwrap_or_else(|| {
            debug_info!("Warning: Could not translate avail addr 0x{:x}", virt);
            virt
        })
    }

    /// Get used ring physical address
    pub fn used_phys_addr(&self) -> u64 {
        let virt = &*self.used as *const _ as u64;
        translate_virt_to_phys(virt).unwrap_or_else(|| {
            debug_info!("Warning: Could not translate used addr 0x{:x}", virt);
            virt
        })
    }

    /// Add a buffer to the available ring
    pub fn add_buffer(&mut self, buffer: &[u8], device_writable: bool) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }

        let desc_idx = self.free_head;
        self.free_head = self.descriptors[desc_idx as usize].next;
        self.num_free -= 1;

        // Convert buffer virtual address to physical address for DMA
        let virt_addr = buffer.as_ptr() as u64;
        let phys_addr = translate_virt_to_phys(virt_addr).unwrap_or(virt_addr);

        let desc = &mut self.descriptors[desc_idx as usize];
        desc.addr = phys_addr;
        desc.len = buffer.len() as u32;
        desc.flags = if device_writable { desc_flags::WRITE } else { 0 };
        desc.next = 0;

        // Add to available ring
        let avail_idx = self.avail.idx.load(Ordering::Relaxed);
        self.avail.ring[(avail_idx % self.size) as usize] = desc_idx;

        // Memory barrier
        core::sync::atomic::fence(Ordering::SeqCst);

        self.avail.idx.store(avail_idx.wrapping_add(1), Ordering::Release);

        Some(desc_idx)
    }

    /// Notify the device that there are new buffers available
    pub fn notify(&self) {
        unsafe {
            write_volatile(self.notify_addr, self.queue_idx);
        }
    }

    /// Check if there are used buffers to process
    pub fn has_used_buffers(&self) -> bool {
        let used_idx = self.used.idx.load(Ordering::Acquire);
        used_idx != self.last_used_idx
    }

    /// Get the next used buffer
    pub fn pop_used(&mut self) -> Option<(u16, u32)> {
        let used_idx = self.used.idx.load(Ordering::Acquire);
        if used_idx == self.last_used_idx {
            return None;
        }

        let elem = &self.used.ring[(self.last_used_idx % self.size) as usize];
        let desc_idx = elem.id as u16;
        let len = elem.len;

        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        // Return descriptor to free list
        self.descriptors[desc_idx as usize].next = self.free_head;
        self.free_head = desc_idx;
        self.num_free += 1;

        Some((desc_idx, len))
    }
}

/// Modern VirtIO device with MMIO
pub struct VirtioDevice {
    pub pci: PciDevice,
    /// Base addresses for each BAR
    bar_addrs: [u64; 6],
    /// Capability locations
    caps: VirtioCaps,
}

// SAFETY: VirtioDevice only contains Copy types and accesses MMIO through bar_addrs.
// Access is controlled by the Mutex wrapping VirtioTablet.
unsafe impl Send for VirtioDevice {}
unsafe impl Sync for VirtioDevice {}

impl VirtioDevice {
    /// Initialize a VirtIO device from a PCI device
    pub fn new(pci: PciDevice) -> Option<Self> {
        debug_info!("Initializing VirtIO device {:04x}:{:04x}",
            pci.vendor_id, pci.device_id);

        // Read all BARs and convert physical addresses to virtual addresses
        let mut bar_addrs = [0u64; 6];
        for i in 0..6 {
            if let Some(bar) = pci.read_bar(i) {
                match bar {
                    Bar::Memory { address, size, .. } => {
                        // Convert physical BAR address to virtual address
                        let virt_addr = phys_to_virt(address).unwrap_or(address);
                        bar_addrs[i as usize] = virt_addr;
                        debug_info!("  BAR{}: MMIO phys=0x{:x} virt=0x{:x} size={}",
                            i, address, virt_addr, size);
                    }
                    Bar::Io { port } => {
                        bar_addrs[i as usize] = port as u64;
                        debug_info!("  BAR{}: I/O at 0x{:04x}", i, port);
                    }
                }
            }
        }

        // Enable memory space access and bus mastering
        pci.enable_memory_space();
        pci.enable_bus_master();

        // Parse VirtIO capabilities
        let caps = Self::parse_capabilities(&pci)?;
        debug_info!("  Common config: BAR{} offset 0x{:x}",
            caps.common_cfg_bar, caps.common_cfg_offset);
        debug_info!("  Notify: BAR{} offset 0x{:x} multiplier {}",
            caps.notify_bar, caps.notify_offset, caps.notify_multiplier);
        debug_info!("  ISR: BAR{} offset 0x{:x}", caps.isr_bar, caps.isr_offset);
        debug_info!("  Device config: BAR{} offset 0x{:x}",
            caps.device_cfg_bar, caps.device_cfg_offset);

        Some(Self { pci, bar_addrs, caps })
    }

    /// Parse VirtIO PCI capabilities
    fn parse_capabilities(pci: &PciDevice) -> Option<VirtioCaps> {
        let mut caps = VirtioCaps::default();
        let mut found_common = false;
        let mut found_notify = false;

        // Check if device has capabilities (status bit 4)
        let status = (pci.read_config(0x04) >> 16) as u16;
        if status & 0x10 == 0 {
            debug_info!("  Device has no PCI capabilities");
            return None;
        }

        // Get capabilities pointer (offset 0x34)
        let mut cap_ptr = (pci.read_config(0x34) & 0xFF) as u8;

        while cap_ptr != 0 {
            // Read capability header
            let cap_header = pci.read_config(cap_ptr);
            let cap_id = (cap_header & 0xFF) as u8;
            let next_ptr = ((cap_header >> 8) & 0xFF) as u8;

            // VirtIO vendor-specific capability (ID 0x09)
            if cap_id == 0x09 {
                let cfg_type = ((cap_header >> 24) & 0xFF) as u8;
                let bar = pci.read_config(cap_ptr + 4);
                let bar_num = (bar & 0xFF) as u8;
                let offset = pci.read_config(cap_ptr + 8);

                debug_trace!("  VirtIO cap type {} at BAR{} offset 0x{:x}",
                    cfg_type, bar_num, offset);

                match cfg_type {
                    cap_type::COMMON_CFG => {
                        caps.common_cfg_bar = bar_num;
                        caps.common_cfg_offset = offset;
                        found_common = true;
                    }
                    cap_type::NOTIFY_CFG => {
                        caps.notify_bar = bar_num;
                        caps.notify_offset = offset;
                        // Notify multiplier is at offset +16 in the capability
                        caps.notify_multiplier = pci.read_config(cap_ptr + 16);
                        found_notify = true;
                    }
                    cap_type::ISR_CFG => {
                        caps.isr_bar = bar_num;
                        caps.isr_offset = offset;
                    }
                    cap_type::DEVICE_CFG => {
                        caps.device_cfg_bar = bar_num;
                        caps.device_cfg_offset = offset;
                    }
                    _ => {}
                }
            }

            cap_ptr = next_ptr;
        }

        if found_common && found_notify {
            Some(caps)
        } else {
            debug_info!("  Missing required VirtIO capabilities");
            None
        }
    }

    /// Get MMIO address for a BAR + offset
    fn mmio_addr(&self, bar: u8, offset: u32) -> *mut u8 {
        (self.bar_addrs[bar as usize] + offset as u64) as *mut u8
    }

    /// Read from common config
    fn read_common<T: Copy>(&self, offset: usize) -> T {
        unsafe {
            let addr = self.mmio_addr(self.caps.common_cfg_bar,
                self.caps.common_cfg_offset + offset as u32);
            read_volatile(addr as *const T)
        }
    }

    /// Write to common config
    fn write_common<T: Copy>(&self, offset: usize, value: T) {
        unsafe {
            let addr = self.mmio_addr(self.caps.common_cfg_bar,
                self.caps.common_cfg_offset + offset as u32);
            write_volatile(addr as *mut T, value);
        }
    }

    /// Read device status
    pub fn read_status(&self) -> u8 {
        self.read_common(common_cfg::DEVICE_STATUS)
    }

    /// Write device status
    pub fn write_status(&self, status: u8) {
        self.write_common(common_cfg::DEVICE_STATUS, status);
    }

    /// Reset the device
    pub fn reset(&self) {
        self.write_status(0);
        // Wait for reset to complete
        while self.read_status() != 0 {
            core::hint::spin_loop();
        }
    }

    /// Read device features (32 bits at a time)
    pub fn read_device_features(&self, select: u32) -> u32 {
        self.write_common(common_cfg::DEVICE_FEATURE_SELECT, select);
        self.read_common(common_cfg::DEVICE_FEATURE)
    }

    /// Write driver features (32 bits at a time)
    pub fn write_driver_features(&self, select: u32, features: u32) {
        self.write_common(common_cfg::DRIVER_FEATURE_SELECT, select);
        self.write_common(common_cfg::DRIVER_FEATURE, features);
    }

    /// Get number of queues
    pub fn num_queues(&self) -> u16 {
        self.read_common(common_cfg::NUM_QUEUES)
    }

    /// Select a virtqueue
    pub fn select_queue(&self, queue: u16) {
        self.write_common(common_cfg::QUEUE_SELECT, queue);
    }

    /// Get the size of the selected queue
    pub fn get_queue_size(&self) -> u16 {
        self.read_common(common_cfg::QUEUE_SIZE)
    }

    /// Set the size of the selected queue
    pub fn set_queue_size(&self, size: u16) {
        self.write_common(common_cfg::QUEUE_SIZE, size);
    }

    /// Enable the selected queue
    pub fn enable_queue(&self) {
        self.write_common::<u16>(common_cfg::QUEUE_ENABLE, 1);
    }

    /// Set descriptor table address
    pub fn set_queue_desc(&self, addr: u64) {
        self.write_common(common_cfg::QUEUE_DESC, addr);
    }

    /// Set available ring address
    pub fn set_queue_driver(&self, addr: u64) {
        self.write_common(common_cfg::QUEUE_DRIVER, addr);
    }

    /// Set used ring address
    pub fn set_queue_device(&self, addr: u64) {
        self.write_common(common_cfg::QUEUE_DEVICE, addr);
    }

    /// Get queue notify offset
    pub fn get_queue_notify_off(&self) -> u16 {
        self.read_common(common_cfg::QUEUE_NOTIFY_OFF)
    }

    /// Get the notify address for a queue
    pub fn get_notify_addr(&self, queue_notify_off: u16) -> *mut u16 {
        let offset = self.caps.notify_offset as u64
            + (queue_notify_off as u64 * self.caps.notify_multiplier as u64);
        (self.bar_addrs[self.caps.notify_bar as usize] + offset) as *mut u16
    }

    /// Read the ISR status (clears interrupt)
    pub fn read_isr(&self) -> u8 {
        unsafe {
            let addr = self.mmio_addr(self.caps.isr_bar, self.caps.isr_offset);
            read_volatile(addr)
        }
    }

    /// Read from device-specific config
    pub fn read_device_config<T: Copy>(&self, offset: u32) -> T {
        unsafe {
            let addr = self.mmio_addr(self.caps.device_cfg_bar,
                self.caps.device_cfg_offset + offset);
            read_volatile(addr as *const T)
        }
    }

    /// Initialize the device following VirtIO 1.0 spec
    pub fn init_simple(&self) -> bool {
        // 1. Reset
        self.reset();

        // 2. Set ACKNOWLEDGE status bit
        self.write_status(status::ACKNOWLEDGE);

        // 3. Set DRIVER status bit
        self.write_status(status::ACKNOWLEDGE | status::DRIVER);

        // 4. Read device features and write driver features
        let features0 = self.read_device_features(0);
        let features1 = self.read_device_features(1);
        debug_trace!("Device features: 0x{:08x} 0x{:08x}", features0, features1);

        // Accept VIRTIO_F_VERSION_1 (bit 32 = features1 bit 0)
        self.write_driver_features(0, 0);
        self.write_driver_features(1, features1 & 0x01);

        // 5. Set FEATURES_OK status bit
        self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);

        // 6. Re-read status to confirm FEATURES_OK is still set
        if self.read_status() & status::FEATURES_OK == 0 {
            debug_info!("Device did not accept features");
            return false;
        }

        true
    }

    /// Complete initialization
    pub fn finish_init(&self) {
        self.write_status(status::ACKNOWLEDGE | status::DRIVER |
            status::FEATURES_OK | status::DRIVER_OK);
    }

    /// Setup a virtqueue
    pub fn setup_queue(&self, queue_idx: u16) -> Option<Virtqueue> {
        self.select_queue(queue_idx);

        let size = self.get_queue_size();
        if size == 0 {
            return None;
        }

        debug_info!("Setting up queue {} with size {}", queue_idx, size);

        // Get notify offset for this queue
        let notify_off = self.get_queue_notify_off();
        let notify_addr = self.get_notify_addr(notify_off);

        // Create the virtqueue
        let queue = Virtqueue::new(size.min(256), queue_idx, notify_addr);

        // Tell device where the queue structures are (using physical addresses)
        let desc_phys = queue.desc_phys_addr();
        let avail_phys = queue.avail_phys_addr();
        let used_phys = queue.used_phys_addr();

        debug_info!("  Queue {} desc_phys=0x{:x} avail_phys=0x{:x} used_phys=0x{:x}",
            queue_idx, desc_phys, avail_phys, used_phys);

        self.set_queue_desc(desc_phys);
        self.set_queue_driver(avail_phys);
        self.set_queue_device(used_phys);

        // Enable the queue
        self.enable_queue();

        Some(queue)
    }
}
