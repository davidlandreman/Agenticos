//! Polling modern VirtIO network device.

use alloc::vec::Vec;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::drivers::pci::{self, PciDevice};
use crate::drivers::virtio::common::{
    DmaPage, QueueError, UsedBuffer, VirtioDevice, Virtqueue, VIRTIO_F_VERSION_1,
};
use crate::{debug_info, debug_warn};

const VIRTIO_NET_F_MAC: u64 = 1 << 5;
// Modern VirtIO always carries the 12-byte v1 header, including
// `num_buffers`; the two-byte-shorter shape is legacy-only when merged RX
// buffers are not negotiated.
const VIRTIO_NET_HEADER_LEN: usize = 12;
pub const ETHERNET_FRAME_MAX: usize = 1514;
const DMA_BUFFER_LEN: usize = VIRTIO_NET_HEADER_LEN + ETHERNET_FRAME_MAX;
const RX_POOL_SIZE: usize = 32;
const TX_POOL_SIZE: usize = 32;
const FALLBACK_MAC: [u8; 6] = [0x02, 0x41, 0x47, 0x4e, 0x54, 0x01];

#[derive(Debug, Clone, Copy, Default)]
pub struct NetDriverCounters {
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_drops: u64,
    pub tx_drops: u64,
    pub malformed_completions: u64,
    pub pool_exhaustion: u64,
}

pub struct VirtioNet {
    device: VirtioDevice,
    rxq: Virtqueue,
    txq: Virtqueue,
    rx_pages: Vec<DmaPage>,
    tx_pages: Vec<DmaPage>,
    tx_free: Vec<u16>,
    mac: [u8; 6],
    counters: NetDriverCounters,
}

unsafe impl Send for VirtioNet {}

impl VirtioNet {
    pub fn discover() -> Option<Self> {
        pci::find_virtio_net_devices()
            .into_iter()
            .find_map(Self::new)
    }

    pub fn new(pci_device: PciDevice) -> Option<Self> {
        let device = VirtioDevice::new(pci_device)?;
        let negotiated =
            match device.begin_init(VIRTIO_F_VERSION_1, VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC) {
                Ok(features) => features,
                Err(error) => {
                    debug_warn!("VirtIO net feature negotiation failed: {:?}", error);
                    return None;
                }
            };

        let mac = if negotiated & VIRTIO_NET_F_MAC != 0 {
            Self::read_mac_stable(&device)
        } else {
            FALLBACK_MAC
        };
        let rxq = device.setup_queue(0)?;
        let txq = device.setup_queue(1)?;

        let rx_count = RX_POOL_SIZE.min(rxq.size as usize);
        let tx_count = TX_POOL_SIZE.min(txq.size as usize);
        let mut rx_pages = Vec::with_capacity(rx_count);
        let mut tx_pages = Vec::with_capacity(tx_count);
        for _ in 0..rx_count {
            rx_pages.push(DmaPage::new_zeroed()?);
        }
        for _ in 0..tx_count {
            tx_pages.push(DmaPage::new_zeroed()?);
        }
        let mut tx_free = Vec::with_capacity(tx_count);
        for index in (0..tx_count).rev() {
            tx_free.push(index as u16);
        }

        let mut net = Self {
            device,
            rxq,
            txq,
            rx_pages,
            tx_pages,
            tx_free,
            mac,
            counters: NetDriverCounters::default(),
        };
        for index in 0..net.rx_pages.len() {
            if net.requeue_rx(index as u16).is_err() {
                debug_warn!("VirtIO net could not fill RX queue");
                return None;
            }
        }
        net.rxq.notify();
        net.device.finish_init();
        net.device.pci.disable_intx();
        debug_info!(
            "VirtIO net ready: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rx={} tx={}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
            rx_count,
            tx_count
        );
        Some(net)
    }

    fn read_mac_stable(device: &VirtioDevice) -> [u8; 6] {
        loop {
            let generation = device.config_generation();
            let mut mac = [0u8; 6];
            for (offset, octet) in mac.iter_mut().enumerate() {
                *octet = device.read_device_config(offset as u32);
            }
            if device.config_generation() == generation {
                return mac;
            }
        }
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    pub fn counters(&self) -> NetDriverCounters {
        self.counters
    }

    fn requeue_rx(&mut self, token: u16) -> Result<(), QueueError> {
        let index = token as usize;
        let phys = self
            .rx_pages
            .get(index)
            .ok_or(QueueError::InvalidUsedId(token as u32))?
            .phys_addr();
        self.rxq.submit(phys, DMA_BUFFER_LEN, true, token)?;
        Ok(())
    }

    fn reclaim_tx(&mut self) {
        loop {
            match self.txq.pop_used() {
                Ok(Some(UsedBuffer { token, .. })) => {
                    if token as usize >= self.tx_pages.len() || self.tx_free.contains(&token) {
                        self.counters.malformed_completions += 1;
                    } else {
                        self.tx_free.push(token);
                    }
                }
                Ok(None) => break,
                Err(_) => self.counters.malformed_completions += 1,
            }
        }
    }

    fn receive_frame(&mut self) -> Option<NetRxToken> {
        loop {
            let used = match self.rxq.pop_used() {
                Ok(Some(used)) => used,
                Ok(None) => return None,
                Err(QueueError::InvalidUsedLength { token, .. }) => {
                    self.counters.malformed_completions += 1;
                    self.counters.rx_drops += 1;
                    let _ = self.requeue_rx(token);
                    continue;
                }
                Err(_) => {
                    self.counters.malformed_completions += 1;
                    self.counters.rx_drops += 1;
                    continue;
                }
            };
            let index = used.token as usize;
            if index >= self.rx_pages.len()
                || used.len as usize <= VIRTIO_NET_HEADER_LEN
                || used.len as usize > DMA_BUFFER_LEN
            {
                self.counters.rx_drops += 1;
                self.counters.malformed_completions += 1;
                let _ = self.requeue_rx(used.token);
                continue;
            }
            let frame_len = used.len as usize - VIRTIO_NET_HEADER_LEN;
            let mut bytes = [0u8; ETHERNET_FRAME_MAX];
            let source = self.rx_pages[index]
                .bytes(VIRTIO_NET_HEADER_LEN, frame_len)
                .expect("validated RX page range");
            bytes[..frame_len].copy_from_slice(source);
            if self.requeue_rx(used.token).is_err() {
                self.counters.rx_drops += 1;
            } else {
                self.rxq.notify();
            }
            self.counters.rx_frames += 1;
            self.counters.rx_bytes += frame_len as u64;
            return Some(NetRxToken {
                bytes,
                len: frame_len,
            });
        }
    }

    fn transmit_frame<R>(&mut self, len: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
        let Some(token) = self.tx_free.pop() else {
            self.counters.pool_exhaustion += 1;
            self.counters.tx_drops += 1;
            return f(&mut []);
        };
        let index = token as usize;
        if len > ETHERNET_FRAME_MAX {
            self.counters.tx_drops += 1;
            self.tx_free.push(token);
            return f(&mut []);
        }
        let page = &mut self.tx_pages[index];
        page.bytes_mut(0, VIRTIO_NET_HEADER_LEN)
            .expect("header fits")
            .fill(0);
        let result = f(page
            .bytes_mut(VIRTIO_NET_HEADER_LEN, len)
            .expect("validated TX page range"));
        let phys = page.phys_addr();
        match self
            .txq
            .submit(phys, VIRTIO_NET_HEADER_LEN + len, false, token)
        {
            Ok(_) => {
                self.txq.notify();
                self.counters.tx_frames += 1;
                self.counters.tx_bytes += len as u64;
            }
            Err(_) => {
                self.tx_free.push(token);
                self.counters.tx_drops += 1;
            }
        }
        result
    }
}

pub struct NetRxToken {
    bytes: [u8; ETHERNET_FRAME_MAX],
    len: usize,
}

impl RxToken for NetRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.bytes[..self.len])
    }
}

pub struct NetTxToken<'a> {
    device: &'a mut VirtioNet,
}

impl TxToken for NetTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.device.transmit_frame(len, f)
    }
}

impl Device for VirtioNet {
    type RxToken<'a>
        = NetRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = NetTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.reclaim_tx();
        let rx = self.receive_frame()?;
        Some((rx, NetTxToken { device: self }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.reclaim_tx();
        (!self.tx_free.is_empty()).then_some(NetTxToken { device: self })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = ETHERNET_FRAME_MAX;
        capabilities.max_burst_size = Some(TX_POOL_SIZE);
        capabilities
    }
}
