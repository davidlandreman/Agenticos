//! Polling modern VirtIO 9P transport (virtio-9p-pci).
//!
//! Carries whole 9P2000.L messages between the in-kernel client
//! (`src/fs/p9`) and QEMU's `local` fsdev. Exactly one request is in flight
//! at a time; the client serializes callers behind its own lock, so this
//! driver follows the rng/net polling model (INTx disabled, bounded
//! `wait_used`). A timed-out or malformed completion quarantines the channel
//! while the virtqueue DMA storage stays owned by the driver.

use crate::drivers::pci::{self, PciDevice};
use crate::drivers::virtio::common::{
    VirtioDevice, VirtqBuffer, Virtqueue, VirtqueueError, VIRTIO_F_VERSION_1,
};
use crate::{debug_info, debug_warn};
use alloc::string::String;
use alloc::vec::Vec;

/// Feature bit: the device exposes its mount tag in config space. Required —
/// the tag is the device's identity the way a drive serial identifies a
/// virtio-blk disk.
const VIRTIO_9P_F_MOUNT_TAG: u64 = 1 << 0;

/// Spin budget for one host filesystem operation. The `local` fsdev serves
/// requests with ordinary host syscalls (microseconds to low milliseconds),
/// so the budget is sized orders of magnitude above that; only a hung device
/// trips it, and tripping it quarantines the channel rather than retrying.
const COMPLETION_SPINS: usize = 500_000_000;

/// Smallest possible 9P message: size[4] + type[1] + tag[2].
const P9_HEADER_LEN: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P9TransportError {
    Quarantined,
    Queue,
    Timeout,
    MalformedCompletion,
    ShortCompletion,
}

pub struct P9Transport {
    _device: VirtioDevice,
    queue: Virtqueue,
    quarantined: bool,
    tag: String,
}

impl P9Transport {
    /// Find the virtio-9p device carrying `tag` and bring it up. Absence is
    /// normal (share disabled, or a QEMU launch without the device).
    pub fn discover_by_tag(tag: &str) -> Option<Self> {
        pci::find_virtio_9p_devices()
            .into_iter()
            .filter_map(Self::new)
            .find(|transport| transport.tag == tag)
    }

    fn new(pci_device: PciDevice) -> Option<Self> {
        let device = VirtioDevice::new(pci_device)?;
        if let Err(error) = device.begin_init(
            VIRTIO_F_VERSION_1 | VIRTIO_9P_F_MOUNT_TAG,
            VIRTIO_F_VERSION_1 | VIRTIO_9P_F_MOUNT_TAG,
        ) {
            debug_warn!("VirtIO 9p feature negotiation failed: {:?}", error);
            return None;
        }
        let queue = device.setup_queue(0)?;
        let tag = Self::read_tag_stable(&device)?;
        device.finish_init();
        device.pci.disable_intx();
        debug_info!(
            "VirtIO 9p initialized (tag={}, queue size {})",
            tag,
            queue.size
        );
        Some(Self {
            _device: device,
            queue,
            quarantined: false,
            tag,
        })
    }

    /// Config space is `{ u16 tag_len; u8 tag[tag_len] }`. Read under the
    /// config-generation loop so a torn update can't produce a garbled tag.
    fn read_tag_stable(device: &VirtioDevice) -> Option<String> {
        loop {
            let generation = device.config_generation();
            let tag_len: u16 = device.read_device_config(0);
            if tag_len == 0 || tag_len > 256 {
                debug_warn!("VirtIO 9p mount tag length {} out of range", tag_len);
                return None;
            }
            let mut bytes = Vec::with_capacity(tag_len as usize);
            for offset in 0..tag_len as u32 {
                bytes.push(device.read_device_config::<u8>(2 + offset));
            }
            if device.config_generation() == generation {
                return String::from_utf8(bytes).ok();
            }
        }
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn mount_tag(&self) -> &str {
        &self.tag
    }

    /// Send one T-message and receive its R-message. Returns the number of
    /// bytes the device wrote into `response`. The caller owns 9P framing;
    /// this layer only guarantees whole-message delivery or quarantine.
    pub fn rpc(&mut self, request: &[u8], response: &mut [u8]) -> Result<usize, P9TransportError> {
        if self.quarantined {
            return Err(P9TransportError::Quarantined);
        }
        if request.len() < P9_HEADER_LEN || response.len() < P9_HEADER_LEN {
            return Err(P9TransportError::Queue);
        }
        let mut buffers = VirtqBuffer::try_from_slice_segments(request, false)
            .map_err(|_| P9TransportError::Queue)?;
        buffers.extend(
            VirtqBuffer::try_from_mut_slice_segments(response)
                .map_err(|_| P9TransportError::Queue)?,
        );
        let head = self
            .queue
            .add_chain(&buffers)
            .map_err(|_| P9TransportError::Queue)?;
        self.queue.notify();
        let used = match self.queue.wait_used(head, COMPLETION_SPINS) {
            Ok(used) => used as usize,
            Err(VirtqueueError::Timeout) => {
                self.quarantined = true;
                debug_warn!("VirtIO 9p completion timed out; quarantining channel");
                return Err(P9TransportError::Timeout);
            }
            Err(error) => {
                self.quarantined = true;
                debug_warn!("VirtIO 9p malformed completion: {:?}", error);
                return Err(P9TransportError::MalformedCompletion);
            }
        };
        // The used length counts only device-written bytes: the R-message.
        if used < P9_HEADER_LEN || used > response.len() {
            self.quarantined = true;
            return Err(P9TransportError::ShortCompletion);
        }
        Ok(used)
    }
}
