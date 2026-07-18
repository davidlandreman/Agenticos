use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpCidr};

use crate::drivers::virtio::net::NetDriverCounters;
use crate::drivers::virtio::net::VirtioNet;
use crate::net::socket::SocketRegistry;
use crate::net::NetworkConfig;
use crate::{debug_info, debug_warn};

pub(super) struct NetworkStack {
    pub(super) device: VirtioNet,
    pub(super) interface: Interface,
    pub(super) sockets: SocketSet<'static>,
    pub(super) registry: SocketRegistry,
    dhcp: SocketHandle,
    config: NetworkConfig,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PollOutcome {
    pub(super) changed: bool,
    pub(super) config_changed: bool,
    pub(super) config: NetworkConfig,
}

impl NetworkStack {
    pub(super) fn new() -> Option<Self> {
        let mut device = VirtioNet::discover()?;
        let mac = device.mac_address();
        let mut random = [0u8; 16];
        crate::random::fill_bytes(&mut random).ok()?;
        let mut iface_config = Config::new(EthernetAddress(mac).into());
        iface_config.random_seed = u64::from_ne_bytes(random[..8].try_into().ok()?);
        let port_seed = u16::from_ne_bytes(random[8..10].try_into().ok()?);
        let first_ephemeral = 49152 + (port_seed & 0x3fff);
        let now = now();
        let interface = Interface::new(iface_config, &mut device, now);
        let mut sockets = SocketSet::new(Vec::new());
        let dhcp = sockets.add(dhcpv4::Socket::new());
        Some(Self {
            device,
            interface,
            sockets,
            registry: SocketRegistry::new(first_ephemeral),
            dhcp,
            config: NetworkConfig::default(),
        })
    }

    pub(super) fn poll_once(&mut self) -> PollOutcome {
        let timestamp = now();
        let mut changed = matches!(
            self.interface
                .poll(timestamp, &mut self.device, &mut self.sockets),
            PollResult::SocketStateChanged
        );
        let mut config_changed = false;
        let event = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll();
        match event {
            Some(dhcpv4::Event::Configured(config)) => {
                self.interface.update_ip_addrs(|addresses| {
                    addresses.clear();
                    let _ = addresses.push(IpCidr::Ipv4(config.address));
                });
                if let Some(router) = config.router {
                    if self
                        .interface
                        .routes_mut()
                        .add_default_ipv4_route(router)
                        .is_err()
                    {
                        debug_warn!("DHCP default route table is full");
                    }
                } else {
                    self.interface.routes_mut().remove_default_ipv4_route();
                }

                let mut snapshot = NetworkConfig {
                    configured: true,
                    address: config.address.address().octets(),
                    prefix_len: config.address.prefix_len(),
                    router: config.router.map(|address| address.octets()),
                    ..NetworkConfig::default()
                };
                for (index, address) in config.dns_servers.iter().take(3).enumerate() {
                    snapshot.dns_servers[index] = address.octets();
                    snapshot.dns_server_count += 1;
                }
                if snapshot != self.config {
                    debug_info!(
                        "DHCP configured {}.{}.{}.{}/{} router={:?}",
                        snapshot.address[0],
                        snapshot.address[1],
                        snapshot.address[2],
                        snapshot.address[3],
                        snapshot.prefix_len,
                        snapshot.router
                    );
                    self.config = snapshot;
                    changed = true;
                    config_changed = true;
                }
            }
            Some(dhcpv4::Event::Deconfigured) => {
                self.interface
                    .update_ip_addrs(|addresses| addresses.clear());
                self.interface.routes_mut().remove_default_ipv4_route();
                if self.config.configured {
                    debug_info!("DHCP lease lost");
                    self.config = NetworkConfig::default();
                    changed = true;
                    config_changed = true;
                }
            }
            None => {}
        }
        PollOutcome {
            changed,
            config_changed,
            config: self.config,
        }
    }

    pub(super) fn config(&self) -> NetworkConfig {
        self.config
    }

    pub(super) fn counters(&self) -> NetDriverCounters {
        self.device.counters()
    }

    /// Owned per-socket snapshot for `/proc/agenticos/sockets`. Reads
    /// live TCP state from the smoltcp socket set; no smoltcp type
    /// escapes.
    pub(super) fn socket_snapshot(&mut self) -> alloc::vec::Vec<super::socket::SocketSnapshot> {
        super::socket::snapshot_registry(&self.registry, &mut self.sockets)
    }

    pub(super) fn next_poll_ticks(&mut self) -> u64 {
        let milliseconds = self
            .interface
            .poll_delay(now(), &self.sockets)
            .map(|delay| delay.total_millis().max(0) as u64)
            .unwrap_or(100);
        let deadline_ticks = milliseconds.div_ceil(10).max(1);
        let cadence_cap = if self.registry.is_empty() { 10 } else { 1 };
        deadline_ticks.min(cadence_cap)
    }
}

fn now() -> Instant {
    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
    Instant::from_millis(ticks.saturating_mul(10) as i64)
}
