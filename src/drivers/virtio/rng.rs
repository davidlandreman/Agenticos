//! Polling modern VirtIO entropy device.

use crate::drivers::pci::{self, PciDevice};
use crate::drivers::virtio::common::{
    DmaPage, VirtioDevice, Virtqueue, VirtqueueError, DMA_PAGE_SIZE, VIRTIO_F_VERSION_1,
};
use crate::{debug_info, debug_warn};

const COMPLETION_SPINS: usize = 5_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RngError {
    Quarantined,
    Queue,
    Timeout,
    MalformedCompletion,
    ShortCompletion,
}

pub struct VirtioRng {
    _device: VirtioDevice,
    queue: Virtqueue,
    page: DmaPage,
    quarantined: bool,
}

// The driver is serialized by the kernel random broker. Its raw MMIO pointer
// and DMA ownership never move into another concurrent execution context.
unsafe impl Send for VirtioRng {}

impl VirtioRng {
    pub fn discover() -> Option<Self> {
        pci::find_virtio_entropy_devices()
            .into_iter()
            .find_map(Self::new)
    }

    fn new(pci_device: PciDevice) -> Option<Self> {
        let device = VirtioDevice::new(pci_device)?;
        if let Err(error) = device.begin_init(VIRTIO_F_VERSION_1, VIRTIO_F_VERSION_1) {
            debug_warn!("VirtIO RNG feature negotiation failed: {:?}", error);
            return None;
        }
        let queue = device.setup_queue(0)?;
        let page = DmaPage::new_zeroed()?;
        device.finish_init();
        device.pci.disable_intx();
        debug_info!("VirtIO RNG initialized (queue size {})", queue.size);
        Some(Self {
            _device: device,
            queue,
            page,
            quarantined: false,
        })
    }

    pub fn fill_bytes(&mut self, out: &mut [u8]) -> Result<(), RngError> {
        if self.quarantined {
            return Err(RngError::Quarantined);
        }
        let mut offset = 0usize;
        while offset < out.len() {
            let count = (out.len() - offset).min(DMA_PAGE_SIZE);
            self.page
                .bytes_mut(0, count)
                .ok_or(RngError::Queue)?
                .fill(0);
            let head = match self.queue.submit(self.page.phys_addr(), count, true, 0) {
                Ok(head) => head,
                Err(_) => return Err(RngError::Queue),
            };
            self.queue.notify();
            let used = match self.queue.wait_used(head, COMPLETION_SPINS) {
                Ok(used) => used as usize,
                Err(VirtqueueError::Timeout) => {
                    self.quarantined = true;
                    return Err(RngError::Timeout);
                }
                Err(_) => {
                    self.quarantined = true;
                    return Err(RngError::MalformedCompletion);
                }
            };
            if used != count {
                self.quarantined = true;
                return Err(RngError::ShortCompletion);
            }
            let bytes = self.page.bytes(0, count).ok_or(RngError::Queue)?;
            out[offset..offset + count].copy_from_slice(bytes);
            offset += count;
        }
        Ok(())
    }
}
