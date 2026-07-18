//! Bounded AgenticOS socket registry backed by smoltcp sockets.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use smoltcp::iface::SocketHandle as SmolHandle;
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};
use spin::Mutex;

use crate::lib::arc::Arc;
use crate::net::abi::SockAddrV4;
use crate::net::stack::NetworkStack;

pub type SocketId = u64;
const MAX_SOCKETS: usize = crate::userland::fdtable::FD_TABLE_SIZE - 3;
const TCP_BUFFER_SIZE: usize = 16 * 1024;
const DATAGRAM_COUNT: usize = 8;
const DATAGRAM_BYTES: usize = 8 * 1536;
const ICMP_COUNT: usize = 4;
const ICMP_BYTES: usize = 4 * 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Stream,
    Datagram,
    RawIcmp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketError {
    NetworkDown,
    NetworkUnreachable,
    TooManySockets,
    #[expect(dead_code, reason = "socket error contract")]
    NoBuffers,
    Invalid,
    AddressInUse,
    AddressNotAvailable,
    DestinationRequired,
    MessageTooLarge,
    NotConnected,
    IsConnected,
    InProgress,
    Already,
    ConnectionRefused,
    #[expect(dead_code, reason = "socket error contract")]
    TimedOut,
    WouldBlock,
    Unsupported,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readiness {
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub hangup: bool,
}

#[derive(Debug)]
pub struct SocketHandle {
    id: SocketId,
}

impl SocketHandle {
    pub fn id(&self) -> SocketId {
        self.id
    }
}

impl Drop for SocketHandle {
    fn drop(&mut self) {
        // See `net::with_stack_mut`: this queue is shared by preemptible
        // kernel threads and syscall context, so its spinlock must not be
        // held across a timer preemption either.
        let _interrupt_guard = crate::arch::x86_64::interrupt_guard::InterruptGuard::disable();
        DEFERRED_CLOSES.lock().push_back(self.id);
    }
}

lazy_static! {
    static ref DEFERRED_CLOSES: Mutex<VecDeque<SocketId>> = Mutex::new(VecDeque::new());
}

#[derive(Debug, Clone, Copy)]
enum EntryKind {
    Tcp { listening: bool, connecting: bool },
    Udp,
    Icmp,
}

struct SocketEntry {
    smol: SmolHandle,
    kind: EntryKind,
    local: SockAddrV4,
    remote: Option<SockAddrV4>,
    nonblocking: bool,
    reuse_addr: bool,
    recv_timeout_ticks: Option<u64>,
    send_timeout_ticks: Option<u64>,
    ttl: u8,
    tcp_nodelay: bool,
    shutdown_read: bool,
    shutdown_write: bool,
    pending_error: Option<SocketError>,
}

pub(super) struct SocketRegistry {
    entries: BTreeMap<SocketId, SocketEntry>,
    next_id: SocketId,
    next_ephemeral: u16,
}

impl SocketRegistry {
    pub(super) const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
            next_ephemeral: 49152,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn allocate_id(&mut self) -> Result<SocketId, SocketError> {
        if self.entries.len() >= MAX_SOCKETS {
            return Err(SocketError::TooManySockets);
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(SocketError::TooManySockets)?;
        Ok(id)
    }

    fn ephemeral_port(&mut self) -> Result<u16, SocketError> {
        for _ in 0..(u16::MAX - 49152) {
            let port = self.next_ephemeral;
            self.next_ephemeral = if port == u16::MAX { 49152 } else { port + 1 };
            if !self.entries.values().any(|entry| entry.local.port == port) {
                return Ok(port);
            }
        }
        Err(SocketError::AddressInUse)
    }

    fn close(&mut self, sockets: &mut smoltcp::iface::SocketSet<'static>, id: SocketId) {
        if let Some(entry) = self.entries.remove(&id) {
            let _ = sockets.remove(entry.smol);
        }
    }
}

fn new_tcp_socket() -> tcp::Socket<'static> {
    tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; TCP_BUFFER_SIZE]),
        tcp::SocketBuffer::new(vec![0; TCP_BUFFER_SIZE]),
    )
}

fn ip(address: [u8; 4]) -> Ipv4Address {
    Ipv4Address::from(address)
}

fn sockaddr(endpoint: IpEndpoint) -> SockAddrV4 {
    SockAddrV4 {
        address: match endpoint.addr {
            IpAddress::Ipv4(address) => address.octets(),
        },
        port: endpoint.port,
    }
}

pub fn create(
    socket_type: SocketType,
    nonblocking: bool,
) -> Result<Arc<SocketHandle>, SocketError> {
    crate::net::with_stack_mut(|stack| {
        let id = stack.registry.allocate_id()?;
        let (smol, kind) = match socket_type {
            SocketType::Stream => (
                stack.sockets.add(new_tcp_socket()),
                EntryKind::Tcp {
                    listening: false,
                    connecting: false,
                },
            ),
            SocketType::Datagram => {
                let rx = udp::PacketBuffer::new(
                    vec![udp::PacketMetadata::EMPTY; DATAGRAM_COUNT],
                    vec![0; DATAGRAM_BYTES],
                );
                let tx = udp::PacketBuffer::new(
                    vec![udp::PacketMetadata::EMPTY; DATAGRAM_COUNT],
                    vec![0; DATAGRAM_BYTES],
                );
                (stack.sockets.add(udp::Socket::new(rx, tx)), EntryKind::Udp)
            }
            SocketType::RawIcmp => {
                let rx = icmp::PacketBuffer::new(
                    vec![icmp::PacketMetadata::EMPTY; ICMP_COUNT],
                    vec![0; ICMP_BYTES],
                );
                let tx = icmp::PacketBuffer::new(
                    vec![icmp::PacketMetadata::EMPTY; ICMP_COUNT],
                    vec![0; ICMP_BYTES],
                );
                (
                    stack.sockets.add(icmp::Socket::new(rx, tx)),
                    EntryKind::Icmp,
                )
            }
        };
        stack.registry.entries.insert(
            id,
            SocketEntry {
                smol,
                kind,
                local: SockAddrV4::UNSPECIFIED,
                remote: None,
                nonblocking,
                reuse_addr: false,
                recv_timeout_ticks: None,
                send_timeout_ticks: None,
                ttl: 64,
                tcp_nodelay: false,
                shutdown_read: false,
                shutdown_write: false,
                pending_error: None,
            },
        );
        Ok(Arc::new(SocketHandle { id }))
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn drain_deferred_closes() {
    let ids: Vec<SocketId> = {
        let _interrupt_guard = crate::arch::x86_64::interrupt_guard::InterruptGuard::disable();
        let mut deferred = DEFERRED_CLOSES.lock();
        deferred.drain(..).collect()
    };
    if ids.is_empty() {
        return;
    }
    let _ = crate::net::with_stack_mut(|stack| {
        for id in ids {
            stack.registry.close(&mut stack.sockets, id);
        }
    });
}

#[cfg(feature = "test")]
pub fn live_socket_count() -> usize {
    crate::net::with_stack_mut(|stack| stack.registry.entries.len()).unwrap_or(0)
}

pub fn nonblocking(id: SocketId) -> Result<bool, SocketError> {
    crate::net::with_stack_mut(|stack| {
        stack
            .registry
            .entries
            .get(&id)
            .map(|entry| entry.nonblocking)
            .ok_or(SocketError::Closed)
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn set_nonblocking(id: SocketId, value: bool) -> Result<(), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack
            .registry
            .entries
            .get_mut(&id)
            .ok_or(SocketError::Closed)?;
        entry.nonblocking = value;
        Ok(())
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

fn validate_local(stack: &NetworkStack, address: SockAddrV4) -> Result<(), SocketError> {
    if !address.is_unspecified()
        && (!stack.config().configured || address.address != stack.config().address)
    {
        return Err(SocketError::AddressNotAvailable);
    }
    Ok(())
}

pub fn bind(id: SocketId, mut address: SockAddrV4) -> Result<(), SocketError> {
    crate::net::with_stack_mut(|stack| {
        validate_local(stack, address)?;
        if address.port == 0 {
            address.port = stack.registry.ephemeral_port()?;
        }
        if stack.registry.entries.iter().any(|(other_id, entry)| {
            *other_id != id
                && entry.local.port == address.port
                && (!entry.reuse_addr
                    || !stack
                        .registry
                        .entries
                        .get(&id)
                        .is_some_and(|e| e.reuse_addr))
        }) {
            return Err(SocketError::AddressInUse);
        }
        let entry = stack
            .registry
            .entries
            .get_mut(&id)
            .ok_or(SocketError::Closed)?;
        match entry.kind {
            EntryKind::Tcp {
                listening: false,
                connecting: false,
            } => {}
            EntryKind::Tcp { .. } => return Err(SocketError::Invalid),
            EntryKind::Udp => stack
                .sockets
                .get_mut::<udp::Socket>(entry.smol)
                .bind(IpListenEndpoint {
                    addr: (!address.is_unspecified()).then_some(ip(address.address).into()),
                    port: address.port,
                })
                .map_err(|_| SocketError::AddressInUse)?,
            EntryKind::Icmp => stack
                .sockets
                .get_mut::<icmp::Socket>(entry.smol)
                .bind(icmp::Endpoint::Ident(address.port))
                .map_err(|_| SocketError::AddressInUse)?,
        }
        entry.local = address;
        Ok(())
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn connect(id: SocketId, remote: SockAddrV4) -> Result<(), SocketError> {
    crate::net::with_stack_mut(|stack| {
        if !stack.config().configured {
            return Err(SocketError::NetworkUnreachable);
        }
        if remote.is_unspecified() || remote.port == 0 {
            return Err(SocketError::AddressNotAvailable);
        }
        let kind = stack
            .registry
            .entries
            .get(&id)
            .ok_or(SocketError::Closed)?
            .kind;
        match kind {
            EntryKind::Tcp {
                listening: true, ..
            } => Err(SocketError::Invalid),
            EntryKind::Tcp { connecting, .. } => {
                let (smol, current_local) = {
                    let entry = stack.registry.entries.get(&id).unwrap();
                    (entry.smol, entry.local)
                };
                let state = stack.sockets.get::<tcp::Socket>(smol).state();
                match state {
                    tcp::State::Established | tcp::State::CloseWait => {
                        let entry = stack.registry.entries.get_mut(&id).unwrap();
                        entry.kind = EntryKind::Tcp {
                            listening: false,
                            connecting: false,
                        };
                        entry.remote = Some(remote);
                        Ok(())
                    }
                    tcp::State::SynSent | tcp::State::SynReceived => Err(if connecting {
                        SocketError::Already
                    } else {
                        SocketError::InProgress
                    }),
                    tcp::State::Closed if connecting => {
                        let entry = stack.registry.entries.get_mut(&id).unwrap();
                        entry.kind = EntryKind::Tcp {
                            listening: false,
                            connecting: false,
                        };
                        entry.pending_error = Some(SocketError::ConnectionRefused);
                        Err(SocketError::ConnectionRefused)
                    }
                    tcp::State::Closed => {
                        let local_port = if current_local.port == 0 {
                            stack.registry.ephemeral_port()?
                        } else {
                            current_local.port
                        };
                        let local = IpListenEndpoint {
                            addr: (!current_local.is_unspecified())
                                .then_some(ip(current_local.address).into()),
                            port: local_port,
                        };
                        stack
                            .sockets
                            .get_mut::<tcp::Socket>(smol)
                            .connect(
                                stack.interface.context(),
                                IpEndpoint::new(ip(remote.address).into(), remote.port),
                                local,
                            )
                            .map_err(|_| SocketError::AddressNotAvailable)?;
                        let entry = stack.registry.entries.get_mut(&id).unwrap();
                        entry.local.port = local_port;
                        entry.remote = Some(remote);
                        entry.kind = EntryKind::Tcp {
                            listening: false,
                            connecting: true,
                        };
                        Err(SocketError::InProgress)
                    }
                    _ => Err(SocketError::IsConnected),
                }
            }
            EntryKind::Udp => {
                let needs_bind = stack.registry.entries.get(&id).unwrap().local.port == 0;
                if needs_bind {
                    let port = stack.registry.ephemeral_port()?;
                    let entry = stack.registry.entries.get_mut(&id).unwrap();
                    stack
                        .sockets
                        .get_mut::<udp::Socket>(entry.smol)
                        .bind(port)
                        .map_err(|_| SocketError::AddressInUse)?;
                    entry.local.port = port;
                }
                stack.registry.entries.get_mut(&id).unwrap().remote = Some(remote);
                Ok(())
            }
            EntryKind::Icmp => {
                stack.registry.entries.get_mut(&id).unwrap().remote = Some(remote);
                Ok(())
            }
        }
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn listen(id: SocketId) -> Result<(), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let needs_port = stack
            .registry
            .entries
            .get(&id)
            .ok_or(SocketError::Closed)?
            .local
            .port
            == 0;
        let port = if needs_port {
            stack.registry.ephemeral_port()?
        } else {
            stack.registry.entries[&id].local.port
        };
        let entry = stack.registry.entries.get_mut(&id).unwrap();
        if !matches!(
            entry.kind,
            EntryKind::Tcp {
                listening: false,
                connecting: false
            }
        ) {
            return Err(SocketError::Unsupported);
        }
        stack
            .sockets
            .get_mut::<tcp::Socket>(entry.smol)
            .listen(IpListenEndpoint {
                addr: (!entry.local.is_unspecified()).then_some(ip(entry.local.address).into()),
                port,
            })
            .map_err(|_| SocketError::AddressInUse)?;
        entry.local.port = port;
        entry.kind = EntryKind::Tcp {
            listening: true,
            connecting: false,
        };
        Ok(())
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn accept(
    id: SocketId,
    nonblocking: bool,
) -> Result<(Arc<SocketHandle>, SockAddrV4), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let listener = stack.registry.entries.get(&id).ok_or(SocketError::Closed)?;
        if !matches!(
            listener.kind,
            EntryKind::Tcp {
                listening: true,
                ..
            }
        ) {
            return Err(SocketError::Invalid);
        }
        let established =
            stack.sockets.get::<tcp::Socket>(listener.smol).state() == tcp::State::Established;
        if !established {
            return Err(SocketError::WouldBlock);
        }
        let old_smol = listener.smol;
        let local = sockaddr(
            stack
                .sockets
                .get::<tcp::Socket>(old_smol)
                .local_endpoint()
                .ok_or(SocketError::NotConnected)?,
        );
        let remote = sockaddr(
            stack
                .sockets
                .get::<tcp::Socket>(old_smol)
                .remote_endpoint()
                .ok_or(SocketError::NotConnected)?,
        );
        let mut replacement = new_tcp_socket();
        replacement
            .listen(IpListenEndpoint {
                addr: (!local.is_unspecified()).then_some(ip(local.address).into()),
                port: local.port,
            })
            .map_err(|_| SocketError::AddressInUse)?;
        let replacement_handle = stack.sockets.add(replacement);
        stack.registry.entries.get_mut(&id).unwrap().smol = replacement_handle;

        let accepted_id = stack.registry.allocate_id()?;
        stack.registry.entries.insert(
            accepted_id,
            SocketEntry {
                smol: old_smol,
                kind: EntryKind::Tcp {
                    listening: false,
                    connecting: false,
                },
                local,
                remote: Some(remote),
                nonblocking,
                reuse_addr: false,
                recv_timeout_ticks: None,
                send_timeout_ticks: None,
                ttl: 64,
                tcp_nodelay: false,
                shutdown_read: false,
                shutdown_write: false,
                pending_error: None,
            },
        );
        Ok((Arc::new(SocketHandle { id: accepted_id }), remote))
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn send(
    id: SocketId,
    data: &[u8],
    destination: Option<SockAddrV4>,
) -> Result<usize, SocketError> {
    crate::net::with_stack_mut(|stack| {
        let kind = stack
            .registry
            .entries
            .get(&id)
            .ok_or(SocketError::Closed)?
            .kind;
        let entry = stack.registry.entries.get_mut(&id).unwrap();
        if entry.shutdown_write {
            return Err(SocketError::Closed);
        }
        match kind {
            EntryKind::Tcp { .. } => {
                let socket = stack.sockets.get_mut::<tcp::Socket>(entry.smol);
                if socket.can_send() {
                    socket.send_slice(data).map_err(|_| SocketError::WouldBlock)
                } else if socket.may_send() {
                    Err(SocketError::WouldBlock)
                } else {
                    Err(SocketError::NotConnected)
                }
            }
            EntryKind::Udp => {
                if data.len() > 1500 - 20 - 8 {
                    return Err(SocketError::MessageTooLarge);
                }
                let destination = destination
                    .or(entry.remote)
                    .ok_or(SocketError::DestinationRequired)?;
                stack
                    .sockets
                    .get_mut::<udp::Socket>(entry.smol)
                    .send_slice(
                        data,
                        IpEndpoint::new(ip(destination.address).into(), destination.port),
                    )
                    .map(|_| data.len())
                    .map_err(|error| match error {
                        udp::SendError::BufferFull => SocketError::WouldBlock,
                        udp::SendError::Unaddressable => SocketError::DestinationRequired,
                    })
            }
            EntryKind::Icmp => {
                if data.len() > ICMP_BYTES / ICMP_COUNT {
                    return Err(SocketError::MessageTooLarge);
                }
                let destination = destination
                    .or(entry.remote)
                    .ok_or(SocketError::DestinationRequired)?;
                let socket = stack.sockets.get_mut::<icmp::Socket>(entry.smol);
                if !socket.is_open() {
                    if data.len() < 8 {
                        return Err(SocketError::Invalid);
                    }
                    let identifier = u16::from_be_bytes([data[4], data[5]]);
                    socket
                        .bind(icmp::Endpoint::Ident(identifier))
                        .map_err(|_| SocketError::AddressInUse)?;
                    entry.local.port = identifier;
                }
                socket
                    .send_slice(data, ip(destination.address).into())
                    .map(|_| data.len())
                    .map_err(|_| SocketError::WouldBlock)
            }
        }
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn recv(id: SocketId, data: &mut [u8]) -> Result<(usize, Option<SockAddrV4>), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack
            .registry
            .entries
            .get_mut(&id)
            .ok_or(SocketError::Closed)?;
        if entry.shutdown_read {
            return Ok((0, None));
        }
        match entry.kind {
            EntryKind::Tcp { .. } => {
                let socket = stack.sockets.get_mut::<tcp::Socket>(entry.smol);
                if socket.can_recv() {
                    socket
                        .recv_slice(data)
                        .map(|len| (len, entry.remote))
                        .map_err(|_| SocketError::WouldBlock)
                } else if socket.may_recv() {
                    Err(SocketError::WouldBlock)
                } else {
                    Ok((0, entry.remote))
                }
            }
            EntryKind::Udp => stack
                .sockets
                .get_mut::<udp::Socket>(entry.smol)
                .recv_slice(data)
                .map(|(len, metadata)| (len, Some(sockaddr(metadata.endpoint))))
                .map_err(|error| match error {
                    udp::RecvError::Exhausted => SocketError::WouldBlock,
                    udp::RecvError::Truncated => SocketError::MessageTooLarge,
                }),
            EntryKind::Icmp => stack
                .sockets
                .get_mut::<icmp::Socket>(entry.smol)
                .recv_slice(data)
                .map(|(len, address)| {
                    (
                        len,
                        Some(SockAddrV4 {
                            address: match address {
                                IpAddress::Ipv4(v4) => v4.octets(),
                            },
                            port: 0,
                        }),
                    )
                })
                .map_err(|_| SocketError::WouldBlock),
        }
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn shutdown(id: SocketId, how: i32) -> Result<(), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack
            .registry
            .entries
            .get_mut(&id)
            .ok_or(SocketError::Closed)?;
        match how {
            0 => entry.shutdown_read = true,
            1 => entry.shutdown_write = true,
            2 => {
                entry.shutdown_read = true;
                entry.shutdown_write = true;
            }
            _ => return Err(SocketError::Invalid),
        }
        if matches!(how, 1 | 2) {
            if let EntryKind::Tcp { .. } = entry.kind {
                stack.sockets.get_mut::<tcp::Socket>(entry.smol).close();
            }
        }
        Ok(())
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn local_name(id: SocketId) -> Result<SockAddrV4, SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack.registry.entries.get(&id).ok_or(SocketError::Closed)?;
        let mut local = entry.local;
        match entry.kind {
            EntryKind::Tcp { .. } => {
                if let Some(endpoint) = stack
                    .sockets
                    .get::<tcp::Socket>(entry.smol)
                    .local_endpoint()
                {
                    local = sockaddr(endpoint);
                }
            }
            EntryKind::Udp => {
                let endpoint = stack.sockets.get::<udp::Socket>(entry.smol).endpoint();
                local.port = endpoint.port;
                if let Some(IpAddress::Ipv4(address)) = endpoint.addr {
                    local.address = address.octets();
                }
            }
            EntryKind::Icmp => {}
        }
        if local.is_unspecified() && stack.config().configured && local.port != 0 {
            local.address = stack.config().address;
        }
        Ok(local)
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn peer_name(id: SocketId) -> Result<SockAddrV4, SocketError> {
    crate::net::with_stack_mut(|stack| {
        stack
            .registry
            .entries
            .get(&id)
            .ok_or(SocketError::Closed)?
            .remote
            .ok_or(SocketError::NotConnected)
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn readiness(id: SocketId) -> Result<Readiness, SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack.registry.entries.get(&id).ok_or(SocketError::Closed)?;
        let mut ready = Readiness {
            error: entry.pending_error.is_some(),
            ..Readiness::default()
        };
        match entry.kind {
            EntryKind::Tcp {
                listening,
                connecting,
            } => {
                let socket = stack.sockets.get::<tcp::Socket>(entry.smol);
                ready.readable = if listening {
                    socket.state() == tcp::State::Established
                } else {
                    socket.can_recv() || !socket.may_recv()
                };
                ready.writable =
                    socket.can_send() || (connecting && socket.state() == tcp::State::Established);
                ready.hangup =
                    !listening && !connecting && !socket.may_recv() && !socket.can_recv();
                if connecting && socket.state() == tcp::State::Closed {
                    ready.error = true;
                }
            }
            EntryKind::Udp => {
                let socket = stack.sockets.get::<udp::Socket>(entry.smol);
                ready.readable = socket.can_recv();
                ready.writable = socket.can_send();
            }
            EntryKind::Icmp => {
                let socket = stack.sockets.get::<icmp::Socket>(entry.smol);
                ready.readable = socket.can_recv();
                ready.writable = socket.can_send();
            }
        }
        Ok(ready)
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub fn timeouts(id: SocketId) -> Result<(Option<u64>, Option<u64>), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack.registry.entries.get(&id).ok_or(SocketError::Closed)?;
        Ok((entry.recv_timeout_ticks, entry.send_timeout_ticks))
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

pub enum SocketOption {
    ReuseAddress(bool),
    ReceiveTimeout(Option<u64>),
    SendTimeout(Option<u64>),
    Ttl(u8),
    TcpNoDelay(bool),
}

pub fn set_option(id: SocketId, option: SocketOption) -> Result<(), SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack
            .registry
            .entries
            .get_mut(&id)
            .ok_or(SocketError::Closed)?;
        match option {
            SocketOption::ReuseAddress(value) => entry.reuse_addr = value,
            SocketOption::ReceiveTimeout(value) => entry.recv_timeout_ticks = value,
            SocketOption::SendTimeout(value) => entry.send_timeout_ticks = value,
            SocketOption::Ttl(value) if value != 0 => {
                entry.ttl = value;
                match entry.kind {
                    EntryKind::Tcp { .. } => stack
                        .sockets
                        .get_mut::<tcp::Socket>(entry.smol)
                        .set_hop_limit(Some(value)),
                    EntryKind::Udp => stack
                        .sockets
                        .get_mut::<udp::Socket>(entry.smol)
                        .set_hop_limit(Some(value)),
                    EntryKind::Icmp => stack
                        .sockets
                        .get_mut::<icmp::Socket>(entry.smol)
                        .set_hop_limit(Some(value)),
                }
            }
            SocketOption::Ttl(_) => return Err(SocketError::Invalid),
            SocketOption::TcpNoDelay(value) => {
                if !matches!(entry.kind, EntryKind::Tcp { .. }) {
                    return Err(SocketError::Unsupported);
                }
                entry.tcp_nodelay = value;
                stack
                    .sockets
                    .get_mut::<tcp::Socket>(entry.smol)
                    .set_nagle_enabled(!value);
            }
        }
        Ok(())
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}

#[derive(Debug, Clone, Copy)]
pub struct OptionSnapshot {
    pub socket_type: SocketType,
    pub pending_error: Option<SocketError>,
    pub reuse_addr: bool,
    pub recv_timeout_ticks: Option<u64>,
    pub send_timeout_ticks: Option<u64>,
    pub ttl: u8,
    pub tcp_nodelay: bool,
}

pub fn options(id: SocketId) -> Result<OptionSnapshot, SocketError> {
    crate::net::with_stack_mut(|stack| {
        let entry = stack
            .registry
            .entries
            .get_mut(&id)
            .ok_or(SocketError::Closed)?;
        if matches!(
            entry.kind,
            EntryKind::Tcp {
                connecting: true,
                ..
            }
        ) {
            match stack.sockets.get::<tcp::Socket>(entry.smol).state() {
                tcp::State::Established | tcp::State::CloseWait => {
                    entry.kind = EntryKind::Tcp {
                        listening: false,
                        connecting: false,
                    };
                }
                tcp::State::Closed => {
                    entry.kind = EntryKind::Tcp {
                        listening: false,
                        connecting: false,
                    };
                    entry.pending_error = Some(SocketError::ConnectionRefused);
                }
                _ => {}
            }
        }
        let pending_error = entry.pending_error.take();
        Ok(OptionSnapshot {
            socket_type: match entry.kind {
                EntryKind::Tcp { .. } => SocketType::Stream,
                EntryKind::Udp => SocketType::Datagram,
                EntryKind::Icmp => SocketType::RawIcmp,
            },
            pending_error,
            reuse_addr: entry.reuse_addr,
            recv_timeout_ticks: entry.recv_timeout_ticks,
            send_timeout_ticks: entry.send_timeout_ticks,
            ttl: entry.ttl,
            tcp_nodelay: entry.tcp_nodelay,
        })
    })
    .unwrap_or(Err(SocketError::NetworkDown))
}
