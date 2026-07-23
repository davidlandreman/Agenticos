//! Interrupt-driven modern VirtIO 9P transport (virtio-9p-pci).
//!
//! Carries whole 9P2000.L messages between the in-kernel client
//! (`src/fs/p9`) and QEMU's `local` fsdev. Multiple distinctly tagged client
//! lanes may have requests in flight together. The PCI INTx handler records
//! each used-ring result by descriptor and wakes the exact request waiter.
//! Descriptor ownership remains with that waiter until it consumes the
//! completion, matching the VirtIO block driver's ISR-wake pattern.

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::drivers::pci::{self, PciDevice};
use crate::drivers::virtio::common::{VirtioDevice, VirtqBuffer, Virtqueue, VIRTIO_F_VERSION_1};
use crate::{debug_info, debug_warn};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;

/// Feature bit: the device exposes its mount tag in config space. Required —
/// the tag is the device's identity the way a drive serial identifies a
/// virtio-blk disk.
const VIRTIO_9P_F_MOUNT_TAG: u64 = 1 << 0;

/// Smallest possible 9P message: size[4] + type[1] + tag[2].
const P9_HEADER_LEN: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P9TransportError {
    Quarantined,
    Queue,
    MalformedCompletion,
    ShortCompletion,
}

#[derive(Clone, Copy)]
enum Waiter {
    Bootstrap,
    Kernel(crate::process::ProcessId),
    RingThree(u32),
}

#[derive(Clone, Copy)]
enum Completion {
    Pending,
    Complete(u32),
    Malformed,
}

struct Request {
    token: u64,
    waiter: Waiter,
    completion: Completion,
}

struct Driver {
    device: VirtioDevice,
    queue: Virtqueue,
    irq: u8,
    requests: BTreeMap<u16, Request>,
    quarantined: bool,
}

lazy_static! {
    static ref DRIVERS: InterruptMutex<Vec<Driver>> = InterruptMutex::new(Vec::new());
}

// Keep 9p tokens disjoint from the block driver's low-half sequence. The
// scheduler's I/O wait reason is shared by both transports.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1 << 63);

#[cfg(feature = "test")]
pub fn request_count() -> u64 {
    NEXT_TOKEN.load(Ordering::Relaxed) - (1 << 63)
}

#[cfg(feature = "test")]
static REQUEST_TYPE_COUNTS: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

#[cfg(feature = "test")]
static REQUESTS_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "test")]
static MAX_REQUESTS_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "test")]
pub fn request_type_count(kind: u8) -> u64 {
    REQUEST_TYPE_COUNTS[kind as usize].load(Ordering::Relaxed)
}

#[cfg(feature = "test")]
pub fn max_requests_in_flight() -> u64 {
    MAX_REQUESTS_IN_FLIGHT.load(Ordering::Relaxed)
}

#[derive(Clone)]
pub struct P9Transport {
    index: usize,
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

    /// Initialize one transport and publish it to the interrupt handler before
    /// unmasking its PCI line.
    fn new(pci_device: PciDevice) -> Option<Self> {
        let irq = pci_device.interrupt_line;
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
        let queue_size = queue.size;
        device.finish_init();

        let index = {
            let mut drivers = DRIVERS.lock();
            let index = drivers.len();
            drivers.push(Driver {
                device,
                queue,
                irq,
                requests: BTreeMap::new(),
                quarantined: false,
            });
            index
        };
        if !crate::arch::x86_64::interrupts::enable_pci_irq(irq) {
            debug_warn!("VirtIO 9p has unusable PCI IRQ {}", irq);
        }
        debug_info!(
            "VirtIO 9p initialized (tag={}, queue size {}, irq={})",
            tag,
            queue_size,
            irq
        );
        Some(Self { index, tag })
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
    pub fn rpc(&self, request: &[u8], response: &mut [u8]) -> Result<usize, P9TransportError> {
        if request.len() < P9_HEADER_LEN || response.len() < P9_HEADER_LEN {
            return Err(P9TransportError::Queue);
        }
        #[cfg(feature = "test")]
        REQUEST_TYPE_COUNTS[request[4] as usize].fetch_add(1, Ordering::Relaxed);
        let mut buffers = VirtqBuffer::try_from_slice_segments(request, false)
            .map_err(|_| P9TransportError::Queue)?;
        buffers.extend(
            VirtqBuffer::try_from_mut_slice_segments(response)
                .map_err(|_| P9TransportError::Queue)?,
        );

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
                let driver = drivers.get_mut(self.index).ok_or(P9TransportError::Queue)?;
                if driver.quarantined {
                    return Err(P9TransportError::Quarantined);
                }
                let head = driver
                    .queue
                    .add_chain(&buffers)
                    .map_err(|_| P9TransportError::Queue)?;
                driver.requests.insert(
                    head,
                    Request {
                        token,
                        waiter,
                        completion: Completion::Pending,
                    },
                );
                #[cfg(feature = "test")]
                {
                    let in_flight = REQUESTS_IN_FLIGHT.fetch_add(1, Ordering::Relaxed) + 1;
                    MAX_REQUESTS_IN_FLIGHT.fetch_max(in_flight, Ordering::Relaxed);
                }
                if let Waiter::RingThree(pid) = waiter {
                    crate::diagnostics::shadow::io::submitted(
                        token,
                        crate::diagnostics::shadow::pager::current_generation(),
                        pid,
                        usize::from(u16::MAX) - self.index,
                        head,
                        request.len(),
                    );
                }
                driver.queue.notify();
                head
            };

            match waiter {
                Waiter::RingThree(_) => crate::userland::switch::block_current_ring3_on_io(token),
                Waiter::Kernel(_) => crate::process::block_current_kernel_thread_on_io(token),
                Waiter::Bootstrap => loop {
                    if request_complete(self.index, head) {
                        break;
                    }
                    x86_64::instructions::interrupts::enable_and_hlt();
                    x86_64::instructions::interrupts::disable();
                },
            }
            let mut drivers = DRIVERS.lock();
            let driver = drivers.get_mut(self.index).ok_or(P9TransportError::Queue)?;
            let completed = driver.requests.remove(&head).ok_or_else(|| {
                driver.quarantined = true;
                P9TransportError::MalformedCompletion
            })?;
            #[cfg(feature = "test")]
            REQUESTS_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
            if matches!(completed.waiter, Waiter::RingThree(_)) {
                crate::diagnostics::shadow::io::consumed(completed.token);
            }
            match completed.completion {
                Completion::Pending | Completion::Malformed => {
                    driver.quarantined = true;
                    Err(P9TransportError::MalformedCompletion)
                }
                Completion::Complete(used) => {
                    if driver.queue.release_used(head).is_err() {
                        driver.quarantined = true;
                        return Err(P9TransportError::MalformedCompletion);
                    }
                    let used = used as usize;
                    // The used length counts only device-written bytes: the
                    // R-message.
                    if used < P9_HEADER_LEN || used > response.len() {
                        driver.quarantined = true;
                        return Err(P9TransportError::ShortCompletion);
                    }
                    Ok(used)
                }
            }
        })
    }
}

fn request_complete(index: usize, head: u16) -> bool {
    DRIVERS
        .lock()
        .get(index)
        .and_then(|driver| driver.requests.get(&head))
        .is_some_and(|request| !matches!(request.completion, Completion::Pending))
}

/// Shared PCI INTx dispatch. The ISR records the completion but leaves the
/// descriptor chain pinned until the exact waiter resumes and consumes it.
pub fn handle_interrupt(irq: u8) {
    // No allocation in interrupt context. A 256-entry queue cannot return
    // more than 256 completions in one interrupt drain.
    let mut waking = [None; 256];
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
                match driver.queue.pop_used_deferred() {
                    Ok(Some(used)) => {
                        let Some(request) = driver.requests.get_mut(&used.descriptor) else {
                            driver.quarantined = true;
                            debug_warn!(
                                "VirtIO 9p completion for unknown descriptor {}",
                                used.descriptor
                            );
                            break;
                        };
                        request.completion = Completion::Complete(used.len);
                        if matches!(request.waiter, Waiter::RingThree(_)) {
                            crate::diagnostics::shadow::io::completed(request.token, 0, used.len);
                        }
                        if wake_count < waking.len() {
                            waking[wake_count] = Some((request.token, request.waiter));
                            wake_count += 1;
                        } else {
                            driver.quarantined = true;
                            debug_warn!("VirtIO 9p IRQ wake batch overflow");
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        driver.quarantined = true;
                        debug_warn!("VirtIO 9p malformed completion: {:?}", error);
                        for request in driver.requests.values_mut() {
                            if !matches!(request.completion, Completion::Pending) {
                                continue;
                            }
                            request.completion = Completion::Malformed;
                            if matches!(request.waiter, Waiter::RingThree(_)) {
                                crate::diagnostics::shadow::io::completed(request.token, 0xff, 0);
                            }
                            if wake_count < waking.len() {
                                waking[wake_count] = Some((request.token, request.waiter));
                                wake_count += 1;
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    for (token, waiter) in waking[..wake_count].iter().flatten().copied() {
        match waiter {
            Waiter::Bootstrap => {}
            Waiter::Kernel(pid) => crate::process::queue_kernel_io_wake(pid, token),
            Waiter::RingThree(pid) => crate::userland::lifecycle::queue_ring3_io_wake(pid, token),
        }
    }
}
