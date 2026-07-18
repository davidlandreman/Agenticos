//! Interrupt-driven modern VirtIO block driver.
//!
//! Requests own their DMA bounce pages until the device returns the used-ring
//! entry. Synchronous filesystem callers sleep on the exact request token;
//! the PCI INTx handler reclaims descriptors and wakes only that waiter.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use lazy_static::lazy_static;

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::drivers::block::BlockDevice;
use crate::drivers::pci;
use crate::drivers::virtio::common::{
    DmaPage, VirtioDevice, VirtqBuffer, Virtqueue, VIRTIO_F_VERSION_1,
};
use crate::{debug_info, debug_warn};

const VIRTIO_BLK_F_RO: u64 = 1 << 5;
const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_BLK_T_GET_ID: u32 = 8;
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const SECTOR_SIZE: usize = 512;
const MAX_SECTORS_PER_REQUEST: usize = 128;
const STATUS_OFFSET: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct RequestHeader {
    request_type: u32,
    reserved: u32,
    sector: u64,
}

#[derive(Clone, Copy)]
enum Waiter {
    Bootstrap,
    Kernel(crate::process::ProcessId),
    RingThree(u32),
}

struct Request {
    token: u64,
    waiter: Waiter,
    control: DmaPage,
    data: Vec<DmaPage>,
    data_len: usize,
    complete: bool,
}

struct Driver {
    device: VirtioDevice,
    queue: Virtqueue,
    irq: u8,
    capacity_sectors: u64,
    read_only: bool,
    flush_supported: bool,
    id: String,
    requests: BTreeMap<u16, Request>,
}

lazy_static! {
    static ref DRIVERS: InterruptMutex<Vec<Driver>> = InterruptMutex::new(Vec::new());
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static REQUESTS_SUBMITTED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct VirtioBlockDevice {
    index: usize,
    capacity_sectors: u64,
    read_only: bool,
    flush_supported: bool,
    name: String,
}

#[derive(Clone, Copy)]
enum Operation {
    Read,
    Write,
    Flush,
    GetId,
}

impl Operation {
    fn request_type(self) -> u32 {
        match self {
            Self::Read => VIRTIO_BLK_T_IN,
            Self::Write => VIRTIO_BLK_T_OUT,
            Self::Flush => VIRTIO_BLK_T_FLUSH,
            Self::GetId => VIRTIO_BLK_T_GET_ID,
        }
    }

    fn device_writable(self) -> bool {
        matches!(self, Self::Read | Self::GetId)
    }
}

/// Discover every modern VirtIO block function and enable its INTx line only
/// after the queue and request registry are ready.
pub fn init() -> usize {
    let devices = pci::find_virtio_block_devices();
    for pci_device in devices {
        let irq = pci_device.interrupt_line;
        let Some(device) = VirtioDevice::new(pci_device) else {
            continue;
        };
        let accepted =
            VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_RO | VIRTIO_BLK_F_BLK_SIZE | VIRTIO_BLK_F_FLUSH;
        let Ok(features) = device.begin_init(VIRTIO_F_VERSION_1, accepted) else {
            debug_warn!("VirtIO block feature negotiation failed");
            continue;
        };
        let Some(queue) = device.setup_queue(0) else {
            debug_warn!("VirtIO block queue 0 is unavailable");
            continue;
        };
        let capacity_sectors = read_stable_capacity(&device);
        let read_only = features & VIRTIO_BLK_F_RO != 0;
        let flush_supported = features & VIRTIO_BLK_F_FLUSH != 0;
        device.finish_init();
        DRIVERS.lock().push(Driver {
            device,
            queue,
            irq,
            capacity_sectors,
            read_only,
            flush_supported,
            id: String::new(),
            requests: BTreeMap::new(),
        });
        if !crate::arch::x86_64::interrupts::enable_pci_irq(irq) {
            debug_warn!("VirtIO block has unusable PCI IRQ {}", irq);
        }
    }

    let count = DRIVERS.lock().len();
    for index in 0..count {
        let mut id_bytes = [0u8; 20];
        if perform(index, Operation::GetId, 0, &mut id_bytes).is_ok() {
            let end = id_bytes.iter().position(|byte| *byte == 0).unwrap_or(20);
            let id = core::str::from_utf8(&id_bytes[..end])
                .unwrap_or("virtio-blk")
                .trim()
                .to_string();
            DRIVERS.lock()[index].id = id;
        }
    }
    for (index, driver) in DRIVERS.lock().iter().enumerate() {
        debug_info!(
            "VirtIO block {}: id='{}' sectors={} readonly={} irq={}",
            index,
            driver.id,
            driver.capacity_sectors,
            driver.read_only,
            driver.irq
        );
    }
    count
}

fn read_stable_capacity(device: &VirtioDevice) -> u64 {
    loop {
        let generation = device.config_generation();
        let capacity = device.read_device_config::<u64>(0);
        if generation == device.config_generation() {
            return capacity;
        }
    }
}

impl VirtioBlockDevice {
    pub fn by_id(id: &str) -> Option<Self> {
        let drivers = DRIVERS.lock();
        let index = drivers.iter().position(|driver| driver.id == id)?;
        Some(Self::from_driver(index, &drivers[index]))
    }

    pub fn by_index(index: usize) -> Option<Self> {
        let drivers = DRIVERS.lock();
        Some(Self::from_driver(index, drivers.get(index)?))
    }

    fn from_driver(index: usize, driver: &Driver) -> Self {
        Self {
            index,
            capacity_sectors: driver.capacity_sectors,
            read_only: driver.read_only,
            flush_supported: driver.flush_supported,
            name: if driver.id.is_empty() {
                alloc::format!("virtio-blk{}", index)
            } else {
                driver.id.clone()
            },
        }
    }
}

impl BlockDevice for VirtioBlockDevice {
    fn read_blocks(&self, block: u64, count: u32, buffer: &mut [u8]) -> Result<(), &'static str> {
        let bytes = (count as usize)
            .checked_mul(SECTOR_SIZE)
            .ok_or("block read length overflow")?;
        if buffer.len() < bytes
            || block
                .checked_add(count as u64)
                .is_none_or(|end| end > self.capacity_sectors)
        {
            return Err("invalid VirtIO block read");
        }
        let mut sector = block;
        let mut offset = 0;
        while offset < bytes {
            let len = (bytes - offset).min(MAX_SECTORS_PER_REQUEST * SECTOR_SIZE);
            perform(
                self.index,
                Operation::Read,
                sector,
                &mut buffer[offset..offset + len],
            )?;
            sector += (len / SECTOR_SIZE) as u64;
            offset += len;
        }
        Ok(())
    }

    fn write_blocks(&self, block: u64, count: u32, buffer: &[u8]) -> Result<(), &'static str> {
        if self.read_only {
            return Err("VirtIO block device is read-only");
        }
        let bytes = (count as usize)
            .checked_mul(SECTOR_SIZE)
            .ok_or("block write length overflow")?;
        if buffer.len() < bytes
            || block
                .checked_add(count as u64)
                .is_none_or(|end| end > self.capacity_sectors)
        {
            return Err("invalid VirtIO block write");
        }
        let mut sector = block;
        let mut offset = 0;
        while offset < bytes {
            let len = (bytes - offset).min(MAX_SECTORS_PER_REQUEST * SECTOR_SIZE);
            let mut bounce = buffer[offset..offset + len].to_vec();
            perform(self.index, Operation::Write, sector, &mut bounce)?;
            sector += (len / SECTOR_SIZE) as u64;
            offset += len;
        }
        Ok(())
    }

    fn block_size(&self) -> u32 {
        SECTOR_SIZE as u32
    }
    fn total_blocks(&self) -> u64 {
        self.capacity_sectors
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn name(&self) -> &str {
        &self.name
    }

    fn flush(&self) -> Result<(), &'static str> {
        if !self.flush_supported || self.read_only {
            return Ok(());
        }
        perform(self.index, Operation::Flush, 0, &mut [])
    }
}

fn perform(
    index: usize,
    operation: Operation,
    sector: u64,
    buffer: &mut [u8],
) -> Result<(), &'static str> {
    if !matches!(operation, Operation::Flush | Operation::GetId) && buffer.len() % SECTOR_SIZE != 0
    {
        return Err("unaligned VirtIO block transfer");
    }
    let mut control = DmaPage::new_zeroed().ok_or("out of DMA memory")?;
    let header = RequestHeader {
        request_type: operation.request_type(),
        reserved: 0,
        sector,
    };
    unsafe { core::ptr::write_volatile(control.as_mut_ptr::<RequestHeader>(), header) };
    control.bytes_mut(STATUS_OFFSET, 1).unwrap()[0] = 0xff;

    let mut pages = Vec::new();
    let mut copied = 0usize;
    while copied < buffer.len() {
        let len = (buffer.len() - copied).min(4096);
        let mut page = DmaPage::new_zeroed().ok_or("out of DMA memory")?;
        if matches!(operation, Operation::Write) {
            page.bytes_mut(0, len)
                .unwrap()
                .copy_from_slice(&buffer[copied..copied + len]);
        }
        pages.push(page);
        copied += len;
    }

    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let waiter = if let Some(pid) = crate::userland::lifecycle::current_user_pid() {
        Waiter::RingThree(pid)
    } else if let Some(pid) = crate::process::current_io_waiter() {
        Waiter::Kernel(pid)
    } else {
        Waiter::Bootstrap
    };

    x86_64::instructions::interrupts::without_interrupts(|| {
        let head = {
            let mut drivers = DRIVERS.lock();
            let driver = drivers
                .get_mut(index)
                .ok_or("missing VirtIO block device")?;
            let mut descriptors = Vec::with_capacity(pages.len() + 2);
            descriptors.push(VirtqBuffer {
                addr: control.phys_addr(),
                len: core::mem::size_of::<RequestHeader>() as u32,
                device_writable: false,
            });
            for (page_index, page) in pages.iter().enumerate() {
                let len = (buffer.len() - page_index * 4096).min(4096);
                descriptors.push(VirtqBuffer {
                    addr: page.phys_addr(),
                    len: len as u32,
                    device_writable: operation.device_writable(),
                });
            }
            descriptors.push(VirtqBuffer {
                addr: control.phys_addr() + STATUS_OFFSET as u64,
                len: 1,
                device_writable: true,
            });
            let head = driver
                .queue
                .add_chain(&descriptors)
                .map_err(|_| "VirtIO block queue full")?;
            REQUESTS_SUBMITTED.fetch_add(1, Ordering::Relaxed);
            driver.requests.insert(
                head,
                Request {
                    token,
                    waiter,
                    control,
                    data: pages,
                    data_len: buffer.len(),
                    complete: false,
                },
            );
            driver.queue.notify();
            head
        };

        match waiter {
            Waiter::RingThree(_) => crate::userland::switch::block_current_ring3_on_io(token),
            Waiter::Kernel(_) => crate::process::block_current_kernel_thread_on_io(token),
            Waiter::Bootstrap => loop {
                if request_complete(index, head) {
                    break;
                }
                x86_64::instructions::interrupts::enable_and_hlt();
                x86_64::instructions::interrupts::disable();
            },
        }

        let request = DRIVERS
            .lock()
            .get_mut(index)
            .and_then(|driver| driver.requests.remove(&head))
            .ok_or("lost VirtIO block completion")?;
        let status = request.control.bytes(STATUS_OFFSET, 1).unwrap()[0];
        if status != VIRTIO_BLK_S_OK {
            return Err(if status == VIRTIO_BLK_S_IOERR {
                "VirtIO block I/O error"
            } else {
                "VirtIO block unsupported request"
            });
        }
        if operation.device_writable() {
            let mut offset = 0usize;
            for page in request.data {
                let len = (request.data_len - offset).min(4096);
                buffer[offset..offset + len].copy_from_slice(page.bytes(0, len).unwrap());
                offset += len;
            }
        }
        Ok(())
    })
}

#[cfg(feature = "test")]
pub fn request_count() -> u64 {
    REQUESTS_SUBMITTED.load(Ordering::Relaxed)
}

#[cfg(feature = "test")]
pub fn request_diagnostics() -> (usize, usize) {
    let drivers = DRIVERS.lock();
    let pending = drivers.iter().map(|driver| driver.requests.len()).sum();
    let complete = drivers
        .iter()
        .flat_map(|driver| driver.requests.values())
        .filter(|request| request.complete)
        .count();
    (pending, complete)
}

fn request_complete(index: usize, head: u16) -> bool {
    DRIVERS
        .lock()
        .get(index)
        .and_then(|driver| driver.requests.get(&head))
        .is_some_and(|request| request.complete)
}

/// Shared PCI INTx dispatch. Reading each matching device ISR both identifies
/// and acknowledges the source before used-ring reclamation.
pub fn handle_interrupt(irq: u8) {
    // No allocation in interrupt context: an IRQ may have interrupted the
    // heap allocator itself. 512 covers two full 256-entry queues sharing a
    // legacy PCI line (QEMU's root and host disks share IRQ11).
    let mut waking = [None; 512];
    let mut wake_count = 0usize;
    {
        let Some(mut drivers) = DRIVERS.try_lock() else {
            return;
        };
        for driver in drivers.iter_mut().filter(|driver| driver.irq == irq) {
            if driver.device.read_isr() & 1 == 0 {
                continue;
            }
            loop {
                match driver.queue.pop_used() {
                    Ok(Some(used)) => {
                        if let Some(request) = driver.requests.get_mut(&used.descriptor) {
                            request.complete = true;
                            if wake_count < waking.len() {
                                waking[wake_count] = Some((request.token, request.waiter));
                                wake_count += 1;
                            } else {
                                debug_warn!("VirtIO block IRQ wake batch overflow");
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        debug_warn!("VirtIO block malformed completion: {:?}", error);
                        break;
                    }
                }
            }
        }
    }
    for (token, waiter) in waking[..wake_count].iter().flatten().copied() {
        match waiter {
            Waiter::Bootstrap => {}
            Waiter::Kernel(pid) => crate::process::queue_kernel_io_wake(pid),
            Waiter::RingThree(pid) => crate::userland::lifecycle::queue_ring3_io_wake(pid, token),
        }
    }
}
