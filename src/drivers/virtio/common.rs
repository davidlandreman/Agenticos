//! VirtIO Common Types and Virtqueue Implementation
//!
//! Implements the VirtIO 1.0 specification for virtqueues and common device operations.
//! Supports modern VirtIO devices using MMIO through PCI capabilities.

use crate::debug_info;
use crate::debug_trace;
use crate::drivers::pci::{Bar, PciDevice};
use crate::mm::memory::phys_to_virt;
use crate::mm::paging::translate_virt_to_phys;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicU16, Ordering};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

pub const DMA_PAGE_SIZE: usize = 4096;
pub const MAX_QUEUE_SIZE: usize = 256;
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// VirtIO device status bits
pub mod status {
    pub const ACKNOWLEDGE: u8 = 1;
    pub const DRIVER: u8 = 2;
    pub const DRIVER_OK: u8 = 4;
    pub const FEATURES_OK: u8 = 8;
    #[expect(dead_code, reason = "intentional kernel API surface")]
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
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const MSIX_CONFIG: usize = 0x10;
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const NUM_QUEUES: usize = 0x12;
    pub const DEVICE_STATUS: usize = 0x14;
    pub const CONFIG_GENERATION: usize = 0x15;
    pub const QUEUE_SELECT: usize = 0x16;
    pub const QUEUE_SIZE: usize = 0x18;
    #[expect(dead_code, reason = "intentional kernel API surface")]
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
    #[expect(dead_code, reason = "intentional kernel API surface")]
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

#[derive(Debug, Clone, Copy)]
pub struct VirtqBuffer {
    pub addr: u64,
    pub len: u32,
    pub device_writable: bool,
}

impl VirtqBuffer {
    pub fn try_from_slice_segments(
        buffer: &[u8],
        device_writable: bool,
    ) -> Result<Vec<Self>, VirtqueueError> {
        Self::try_from_raw_segments(buffer.as_ptr(), buffer.len(), device_writable)
    }

    pub fn try_from_mut_slice_segments(buffer: &mut [u8]) -> Result<Vec<Self>, VirtqueueError> {
        Self::try_from_raw_segments(buffer.as_mut_ptr(), buffer.len(), true)
    }

    fn try_from_raw_segments(
        pointer: *const u8,
        len: usize,
        device_writable: bool,
    ) -> Result<Vec<Self>, VirtqueueError> {
        const PAGE_SIZE: usize = 4096;
        let mut segments = Vec::new();
        let mut offset = 0usize;
        while offset < len {
            let virtual_address = pointer as usize + offset;
            let bytes_in_page = PAGE_SIZE - (virtual_address & (PAGE_SIZE - 1));
            let segment_len = bytes_in_page.min(len - offset);
            let virtual_address = virtual_address as u64;
            let addr = translate_virt_to_phys(virtual_address)
                .ok_or(VirtqueueError::AddressTranslation)?;
            segments.push(Self {
                addr,
                len: segment_len as u32,
                device_writable,
            });
            offset += segment_len;
        }
        Ok(segments)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtqueueError {
    EmptyChain,
    NoDescriptors,
    AddressTranslation,
    LengthOverflow,
    Timeout,
    UnexpectedDescriptor,
    MalformedCompletion,
}

/// Aligned available ring structure
#[repr(C, align(2))]
pub struct VirtqAvailRing {
    pub flags: u16,
    pub idx: AtomicU16,
    pub ring: [u16; MAX_QUEUE_SIZE],
}

/// Aligned used ring structure
#[repr(C, align(4))]
pub struct VirtqUsedRing {
    pub flags: u16,
    pub idx: AtomicU16,
    pub ring: [VirtqUsedElem; MAX_QUEUE_SIZE],
}

/// One owned, physically contiguous DMA page exposed through the bootloader's
/// permanent physical-memory mapping. Holding the frame allocation is the
/// ownership pin: the reusable allocator cannot hand the frame to anyone else
/// until this value is dropped.
pub struct DmaPage {
    frame: PhysFrame<Size4KiB>,
    virt: *mut u8,
}

unsafe impl Send for DmaPage {}
unsafe impl Sync for DmaPage {}

impl DmaPage {
    pub fn new_zeroed() -> Option<Self> {
        let frame = crate::mm::memory::with_memory_mapper(|mapper| {
            let frame = mapper.allocate_one_frame()?;
            mapper.zero_frame(frame);
            Some(frame)
        })??;
        let phys = frame.start_address().as_u64();
        let Some(virt) = phys_to_virt(phys) else {
            let _ = crate::mm::memory::with_memory_mapper(|mapper| mapper.release_frame(frame));
            return None;
        };
        Some(Self {
            frame,
            virt: virt as *mut u8,
        })
    }

    pub fn phys_addr(&self) -> u64 {
        self.frame.start_address().as_u64()
    }

    pub fn as_ptr<T>(&self) -> *const T {
        self.virt.cast()
    }

    pub fn as_mut_ptr<T>(&mut self) -> *mut T {
        self.virt.cast()
    }

    pub fn bytes(&self, offset: usize, len: usize) -> Option<&[u8]> {
        let end = offset.checked_add(len)?;
        if end > DMA_PAGE_SIZE {
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts(self.virt.add(offset), len) })
    }

    pub fn bytes_mut(&mut self, offset: usize, len: usize) -> Option<&mut [u8]> {
        let end = offset.checked_add(len)?;
        if end > DMA_PAGE_SIZE {
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts_mut(self.virt.add(offset), len) })
    }
}

impl Drop for DmaPage {
    fn drop(&mut self) {
        let released =
            crate::mm::memory::with_memory_mapper(|mapper| mapper.release_frame(self.frame));
        debug_assert_eq!(released, Some(true));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    Full,
    LengthOverflow,
    InvalidUsedId(u32),
    DuplicateCompletion(u16),
    InvalidUsedLength {
        descriptor: u16,
        token: u16,
        used: u32,
        capacity: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsedBuffer {
    pub descriptor: u16,
    pub token: u16,
    pub len: u32,
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
    descriptor_page: DmaPage,
    /// Available ring
    avail_page: DmaPage,
    /// Used ring
    used_page: DmaPage,
    /// Caller-owned identity associated with each in-flight descriptor.
    tokens: [Option<u16>; MAX_QUEUE_SIZE],
    /// Maximum device-written length accepted for each in-flight head.
    capacities: [u32; MAX_QUEUE_SIZE],
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
    pub fn new(size: u16, queue_idx: u16, notify_addr: *mut u16) -> Option<Self> {
        if size == 0 || size as usize > MAX_QUEUE_SIZE {
            return None;
        }
        let mut descriptor_page = DmaPage::new_zeroed()?;
        let avail_page = DmaPage::new_zeroed()?;
        let used_page = DmaPage::new_zeroed()?;
        let descriptors = unsafe {
            core::slice::from_raw_parts_mut(
                descriptor_page.as_mut_ptr::<VirtqDesc>(),
                size as usize,
            )
        };
        for (i, descriptor) in descriptors.iter_mut().enumerate() {
            descriptor.next = if i + 1 < size as usize {
                i as u16 + 1
            } else {
                0
            };
        }

        Some(Self {
            size,
            descriptor_page,
            avail_page,
            used_page,
            tokens: [None; MAX_QUEUE_SIZE],
            capacities: [0; MAX_QUEUE_SIZE],
            free_head: 0,
            num_free: size,
            last_used_idx: 0,
            queue_idx,
            notify_addr,
        })
    }

    fn descriptors(&self) -> &[VirtqDesc] {
        unsafe { core::slice::from_raw_parts(self.descriptor_page.as_ptr(), self.size as usize) }
    }

    fn descriptors_mut(&mut self) -> &mut [VirtqDesc] {
        unsafe {
            core::slice::from_raw_parts_mut(self.descriptor_page.as_mut_ptr(), self.size as usize)
        }
    }

    fn avail(&self) -> &VirtqAvailRing {
        unsafe { &*self.avail_page.as_ptr() }
    }

    fn avail_mut(&mut self) -> &mut VirtqAvailRing {
        unsafe { &mut *self.avail_page.as_mut_ptr() }
    }

    fn used(&self) -> &VirtqUsedRing {
        unsafe { &*self.used_page.as_ptr() }
    }

    /// Get descriptor table physical address
    pub fn desc_phys_addr(&self) -> u64 {
        self.descriptor_page.phys_addr()
    }

    /// Get available ring physical address
    pub fn avail_phys_addr(&self) -> u64 {
        self.avail_page.phys_addr()
    }

    /// Get used ring physical address
    pub fn used_phys_addr(&self) -> u64 {
        self.used_page.phys_addr()
    }

    /// Add a readable/writable descriptor chain as one request.
    pub fn add_chain(&mut self, buffers: &[VirtqBuffer]) -> Result<u16, VirtqueueError> {
        if buffers.is_empty() {
            return Err(VirtqueueError::EmptyChain);
        }
        if buffers.len() > self.num_free as usize {
            return Err(VirtqueueError::NoDescriptors);
        }

        let capacity = buffers.iter().try_fold(0u32, |total, buffer| {
            total
                .checked_add(buffer.len)
                .ok_or(VirtqueueError::LengthOverflow)
        })?;

        let head = self.free_head;
        let mut current = head;
        for (position, buffer) in buffers.iter().enumerate() {
            let next_free = self.descriptors()[current as usize].next;
            let has_next = position + 1 < buffers.len();
            self.descriptors_mut()[current as usize] = VirtqDesc {
                addr: buffer.addr,
                len: buffer.len,
                flags: (if buffer.device_writable {
                    desc_flags::WRITE
                } else {
                    0
                }) | (if has_next { desc_flags::NEXT } else { 0 }),
                next: if has_next { next_free } else { 0 },
            };
            current = next_free;
        }
        self.free_head = current;
        self.num_free -= buffers.len() as u16;
        self.tokens[head as usize] = Some(head);
        self.capacities[head as usize] = capacity;

        let avail_idx = self.avail().idx.load(Ordering::Relaxed);
        let ring_index = (avail_idx % self.size) as usize;
        self.avail_mut().ring[ring_index] = head;
        fence(Ordering::Release);
        self.avail()
            .idx
            .store(avail_idx.wrapping_add(1), Ordering::Release);
        Ok(head)
    }

    /// Submit a caller-owned physical buffer and retain its token until the
    /// device returns the descriptor.
    pub fn submit(
        &mut self,
        phys_addr: u64,
        len: usize,
        device_writable: bool,
        token: u16,
    ) -> Result<u16, QueueError> {
        if self.num_free == 0 {
            return Err(QueueError::Full);
        }
        let len = u32::try_from(len).map_err(|_| QueueError::LengthOverflow)?;

        let desc_idx = self.free_head;
        self.free_head = self.descriptors()[desc_idx as usize].next;
        self.num_free -= 1;

        let desc = &mut self.descriptors_mut()[desc_idx as usize];
        desc.addr = phys_addr;
        desc.len = len;
        desc.flags = if device_writable {
            desc_flags::WRITE
        } else {
            0
        };
        desc.next = 0;
        self.tokens[desc_idx as usize] = Some(token);
        self.capacities[desc_idx as usize] = len;

        // Add to available ring
        let avail_idx = self.avail().idx.load(Ordering::Relaxed);
        let ring_index = (avail_idx % self.size) as usize;
        self.avail_mut().ring[ring_index] = desc_idx;
        fence(Ordering::Release);
        self.avail()
            .idx
            .store(avail_idx.wrapping_add(1), Ordering::Release);

        Ok(desc_idx)
    }

    /// Notify the device that there are new buffers available
    pub fn notify(&self) {
        unsafe {
            write_volatile(self.notify_addr, self.queue_idx);
        }
    }

    /// Get the next used buffer
    pub fn pop_used(&mut self) -> Result<Option<UsedBuffer>, QueueError> {
        let used = match self.pop_used_deferred() {
            Ok(Some(used)) => used,
            Ok(None) => return Ok(None),
            Err(error @ QueueError::InvalidUsedLength { descriptor, .. }) => {
                let _ = self.release_used(descriptor);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        self.release_used(used.descriptor)?;
        Ok(Some(used))
    }

    /// Read one used-ring entry without returning its descriptor chain to the
    /// free list. Block I/O uses this so the completed request's descriptor
    /// identity and DMA ownership remain stable until its sleeping waiter
    /// consumes the result.
    pub fn pop_used_deferred(&mut self) -> Result<Option<UsedBuffer>, QueueError> {
        let used_idx = self.used().idx.load(Ordering::Acquire);
        if used_idx == self.last_used_idx {
            return Ok(None);
        }

        fence(Ordering::Acquire);
        let elem = self.used().ring[(self.last_used_idx % self.size) as usize];
        let raw_id = elem.id;

        self.last_used_idx = self.last_used_idx.wrapping_add(1);
        if raw_id >= self.size as u32 {
            return Err(QueueError::InvalidUsedId(raw_id));
        }
        let desc_idx = raw_id as u16;
        let capacity = self.capacities[desc_idx as usize];
        let Some(token) = self.tokens[desc_idx as usize] else {
            return Err(QueueError::DuplicateCompletion(desc_idx));
        };

        if elem.len > capacity {
            return Err(QueueError::InvalidUsedLength {
                descriptor: desc_idx,
                token,
                used: elem.len,
                capacity,
            });
        }
        Ok(Some(UsedBuffer {
            descriptor: desc_idx,
            token,
            len: elem.len,
        }))
    }

    /// Return a deferred used descriptor chain to the queue after its owner
    /// has consumed every device-written byte.
    pub fn release_used(&mut self, desc_idx: u16) -> Result<(), QueueError> {
        if desc_idx >= self.size || self.tokens[desc_idx as usize].take().is_none() {
            return Err(QueueError::DuplicateCompletion(desc_idx));
        }
        let mut current = desc_idx;
        let mut released = 0u16;
        loop {
            if current >= self.size || released >= self.size {
                break;
            }
            let has_next = self.descriptors()[current as usize].flags & desc_flags::NEXT != 0;
            let next = self.descriptors()[current as usize].next;
            self.descriptors_mut()[current as usize] = VirtqDesc {
                next: self.free_head,
                ..VirtqDesc::default()
            };
            self.free_head = current;
            self.num_free = self.num_free.saturating_add(1);
            released += 1;
            if !has_next {
                break;
            }
            current = next;
        }
        self.capacities[desc_idx as usize] = 0;
        Ok(())
    }

    #[cfg(feature = "test")]
    pub fn inject_used_for_test(&mut self, id: u32, len: u32) {
        let idx = self.used().idx.load(Ordering::Relaxed);
        unsafe {
            let used = &mut *self.used_page.as_mut_ptr::<VirtqUsedRing>();
            used.ring[(idx % self.size) as usize] = VirtqUsedElem { id, len };
            used.idx.store(idx.wrapping_add(1), Ordering::Release);
        }
    }

    #[cfg(feature = "test")]
    pub fn free_count_for_test(&self) -> u16 {
        self.num_free
    }

    #[cfg(feature = "test")]
    pub fn set_used_indices_for_test(&mut self, index: u16) {
        self.last_used_idx = index;
        unsafe {
            (*self.used_page.as_mut_ptr::<VirtqUsedRing>())
                .idx
                .store(index, Ordering::Relaxed);
        }
    }

    /// Bounded synchronous completion for control queues.
    pub fn wait_used(
        &mut self,
        expected_head: u16,
        max_spins: usize,
    ) -> Result<u32, VirtqueueError> {
        for _ in 0..max_spins {
            match self.pop_used() {
                Ok(Some(used)) if used.descriptor == expected_head => return Ok(used.len),
                Ok(Some(_)) => return Err(VirtqueueError::UnexpectedDescriptor),
                Ok(None) => {}
                Err(_) => return Err(VirtqueueError::MalformedCompletion),
            }
            core::hint::spin_loop();
        }
        Err(VirtqueueError::Timeout)
    }

    #[cfg(feature = "test")]
    pub fn test_num_free(&self) -> u16 {
        self.num_free
    }

    #[cfg(feature = "test")]
    pub fn test_complete(&mut self, head: u16, len: u32) {
        self.inject_used_for_test(head as u32, len);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtioInitError {
    ResetTimeout { status: u8 },
    MissingRequiredFeatures { required: u64, offered: u64 },
    FeaturesRejected,
}

// SAFETY: VirtioDevice only contains Copy types and accesses MMIO through bar_addrs.
// Access is controlled by the Mutex wrapping VirtioTablet.
unsafe impl Send for VirtioDevice {}
unsafe impl Sync for VirtioDevice {}

impl VirtioDevice {
    /// Initialize a VirtIO device from a PCI device
    pub fn new(pci: PciDevice) -> Option<Self> {
        debug_info!(
            "Initializing VirtIO device {:04x}:{:04x}",
            pci.vendor_id,
            pci.device_id
        );

        // Read all BARs and convert physical addresses to virtual addresses
        let mut bar_addrs = [0u64; 6];
        for i in 0..6 {
            if let Some(bar) = pci.read_bar(i) {
                match bar {
                    Bar::Memory { address, size, .. } => {
                        // Convert physical BAR address to virtual address
                        let virt_addr = phys_to_virt(address).unwrap_or(address);
                        bar_addrs[i as usize] = virt_addr;
                        debug_info!(
                            "  BAR{}: MMIO phys=0x{:x} virt=0x{:x} size={}",
                            i,
                            address,
                            virt_addr,
                            size
                        );
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
        debug_info!(
            "  Common config: BAR{} offset 0x{:x}",
            caps.common_cfg_bar,
            caps.common_cfg_offset
        );
        debug_info!(
            "  Notify: BAR{} offset 0x{:x} multiplier {}",
            caps.notify_bar,
            caps.notify_offset,
            caps.notify_multiplier
        );
        debug_info!("  ISR: BAR{} offset 0x{:x}", caps.isr_bar, caps.isr_offset);
        debug_info!(
            "  Device config: BAR{} offset 0x{:x}",
            caps.device_cfg_bar,
            caps.device_cfg_offset
        );

        Some(Self {
            pci,
            bar_addrs,
            caps,
        })
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

                debug_trace!(
                    "  VirtIO cap type {} at BAR{} offset 0x{:x}",
                    cfg_type,
                    bar_num,
                    offset
                );

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
            let addr = self.mmio_addr(
                self.caps.common_cfg_bar,
                self.caps.common_cfg_offset + offset as u32,
            );
            read_volatile(addr as *const T)
        }
    }

    /// Write to common config
    fn write_common<T: Copy>(&self, offset: usize, value: T) {
        unsafe {
            let addr = self.mmio_addr(
                self.caps.common_cfg_bar,
                self.caps.common_cfg_offset + offset as u32,
            );
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
    pub fn reset(&self) -> bool {
        const RESET_SPINS: usize = 20_000_000;
        self.write_status(0);
        // Wait for reset to complete
        for _ in 0..RESET_SPINS {
            if self.read_status() == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        let status = self.read_status();
        crate::debug_error!("VirtIO reset timed out with status=0x{:02x}", status);
        false
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

    /// Select a virtqueue
    pub fn select_queue(&self, queue: u16) {
        self.write_common(common_cfg::QUEUE_SELECT, queue);
    }

    /// Get the size of the selected queue
    pub fn get_queue_size(&self) -> u16 {
        self.read_common(common_cfg::QUEUE_SIZE)
    }

    /// Set the size of the selected queue

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
            let addr = self.mmio_addr(
                self.caps.device_cfg_bar,
                self.caps.device_cfg_offset + offset,
            );
            read_volatile(addr as *const T)
        }
    }

    pub fn config_generation(&self) -> u8 {
        self.read_common(common_cfg::CONFIG_GENERATION)
    }

    fn mark_failed(&self) {
        self.write_status(self.read_status() | status::FAILED);
    }

    /// Begin initialization with an explicit feature contract. Returns the
    /// negotiated subset. The device is left at FEATURES_OK so callers can
    /// configure every queue before setting DRIVER_OK.
    pub fn begin_init(&self, required: u64, accepted: u64) -> Result<u64, VirtioInitError> {
        if !self.reset() {
            return Err(VirtioInitError::ResetTimeout {
                status: self.read_status(),
            });
        }
        self.write_status(status::ACKNOWLEDGE);
        self.write_status(status::ACKNOWLEDGE | status::DRIVER);

        let offered =
            self.read_device_features(0) as u64 | ((self.read_device_features(1) as u64) << 32);
        debug_trace!("Device features: 0x{:016x}", offered);
        if offered & required != required {
            self.mark_failed();
            return Err(VirtioInitError::MissingRequiredFeatures { required, offered });
        }

        let negotiated = offered & (accepted | required);
        self.write_driver_features(0, negotiated as u32);
        self.write_driver_features(1, (negotiated >> 32) as u32);
        self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
        if self.read_status() & status::FEATURES_OK == 0 {
            self.mark_failed();
            return Err(VirtioInitError::FeaturesRejected);
        }
        Ok(negotiated)
    }

    /// Initialize the device following VirtIO 1.0 spec
    pub fn init_simple(&self) -> bool {
        self.begin_init(VIRTIO_F_VERSION_1, VIRTIO_F_VERSION_1)
            .is_ok()
    }

    /// Initialize while negotiating only explicitly understood features.
    pub fn init_with_features(
        &self,
        supported_low: u32,
        supported_high: u32,
    ) -> Option<(u32, u32)> {
        if !self.reset() {
            return None;
        }
        self.write_status(status::ACKNOWLEDGE);
        self.write_status(status::ACKNOWLEDGE | status::DRIVER);
        let device_low = self.read_device_features(0);
        let device_high = self.read_device_features(1);
        let negotiated_low = device_low & supported_low;
        // VIRTIO_F_VERSION_1 (feature bit 32) is mandatory here.
        let negotiated_high = device_high & supported_high & 0x1;
        if negotiated_high & 0x1 == 0 {
            self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FAILED);
            return None;
        }
        self.write_driver_features(0, negotiated_low);
        self.write_driver_features(1, negotiated_high);
        self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
        if self.read_status() & status::FEATURES_OK == 0 {
            self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FAILED);
            return None;
        }
        Some((negotiated_low, negotiated_high))
    }

    /// Complete initialization
    pub fn finish_init(&self) {
        self.write_status(
            status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK,
        );
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
        let queue = Virtqueue::new(size.min(MAX_QUEUE_SIZE as u16), queue_idx, notify_addr)?;

        // Tell device where the queue structures are (using physical addresses)
        let desc_phys = queue.desc_phys_addr();
        let avail_phys = queue.avail_phys_addr();
        let used_phys = queue.used_phys_addr();

        debug_info!(
            "  Queue {} desc_phys=0x{:x} avail_phys=0x{:x} used_phys=0x{:x}",
            queue_idx,
            desc_phys,
            avail_phys,
            used_phys
        );

        self.set_queue_desc(desc_phys);
        self.set_queue_driver(avail_phys);
        self.set_queue_device(used_phys);

        // Enable the queue
        self.enable_queue();

        Some(queue)
    }
}
