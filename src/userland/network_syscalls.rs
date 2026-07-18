//! Linux x86-64 IPv4 socket syscall subset.

use alloc::vec;
use alloc::vec::Vec;

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::net::abi::SockAddrV4;
use crate::net::socket::{self, SocketError, SocketOption, SocketType};
use crate::userland::abi::*;
use crate::userland::fdtable::{FdSlot, FdTable, FD_TABLE_SIZE};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const SOCK_NONBLOCK: i32 = 0x800;
const SOCK_CLOEXEC: i32 = 0x80000;
const IPPROTO_ICMP: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;
const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;
const IPPROTO_IP: i32 = 0;
const IP_TTL: i32 = 2;
const TCP_NODELAY: i32 = 1;
const MSG_DONTWAIT: i32 = 0x40;
const IO_MAX: usize = 64 * 1024;
const IOV_MAX: usize = 16;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxSockAddrIn {
    family: u16,
    port_be: u16,
    address: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxTimeval {
    seconds: i64,
    microseconds: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxIovec {
    base: u64,
    len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxMsghdr {
    name: u64,
    name_len: u32,
    _name_pad: u32,
    iov: u64,
    iov_len: u64,
    control: u64,
    control_len: u64,
    flags: i32,
    _flags_pad: u32,
}

fn with_fd_slot(fd: i32) -> Option<FdSlot> {
    if fd < 0 || fd as usize >= FD_TABLE_SIZE {
        return None;
    }
    crate::userland::lifecycle::with_active_user(|process| process.fd_table.get(fd).cloned())
}

fn with_fd_table_mut<R>(f: impl FnOnce(&mut FdTable) -> R) -> R {
    crate::userland::lifecycle::with_active_user(|process| f(&mut process.fd_table))
}

fn socket_id(fd: i32) -> Result<u64, i64> {
    match with_fd_slot(fd) {
        Some(FdSlot::Socket { handle, .. }) => Ok(handle.id()),
        _ => Err(EBADF),
    }
}

pub fn map_socket_error(error: SocketError) -> i64 {
    match error {
        SocketError::NetworkDown => ENETDOWN,
        SocketError::NetworkUnreachable => ENETUNREACH,
        SocketError::TooManySockets => ENFILE,
        SocketError::NoBuffers => ENOBUFS,
        SocketError::Invalid => EINVAL,
        SocketError::AddressInUse => EADDRINUSE,
        SocketError::AddressNotAvailable => EADDRNOTAVAIL,
        SocketError::DestinationRequired => EDESTADDRREQ,
        SocketError::MessageTooLarge => EMSGSIZE,
        SocketError::NotConnected | SocketError::Closed => ENOTCONN,
        SocketError::IsConnected => EISCONN,
        SocketError::InProgress => EINPROGRESS,
        SocketError::Already => EALREADY,
        SocketError::ConnectionRefused => ECONNREFUSED,
        SocketError::TimedOut => ETIMEDOUT,
        SocketError::WouldBlock => EAGAIN,
        SocketError::Unsupported => ENOPROTOOPT,
    }
}

fn read_sockaddr(pointer: u64, length: u64) -> Result<SockAddrV4, i64> {
    if pointer == 0 || length < core::mem::size_of::<LinuxSockAddrIn>() as u64 {
        return Err(EINVAL);
    }
    let raw = crate::userland::usercopy::read_unaligned::<LinuxSockAddrIn>(pointer)?;
    if raw.family != AF_INET as u16 {
        return Err(EAFNOSUPPORT);
    }
    Ok(SockAddrV4 {
        address: raw.address,
        port: u16::from_be(raw.port_be),
    })
}

fn validate_sockaddr_output(pointer: u64, length_pointer: u64) -> Result<(), i64> {
    if pointer == 0 && length_pointer == 0 {
        return Ok(());
    }
    if pointer == 0 || length_pointer == 0 {
        return Err(EFAULT);
    }
    let length = crate::userland::usercopy::read_unaligned::<u32>(length_pointer)?;
    if length < core::mem::size_of::<LinuxSockAddrIn>() as u32 {
        return Err(EINVAL);
    }
    crate::userland::usercopy::ensure_user_range(
        pointer,
        core::mem::size_of::<LinuxSockAddrIn>() as u64,
        true,
    )
}

fn write_sockaddr(pointer: u64, length_pointer: u64, address: SockAddrV4) -> Result<(), i64> {
    if pointer == 0 && length_pointer == 0 {
        return Ok(());
    }
    validate_sockaddr_output(pointer, length_pointer)?;
    let raw = LinuxSockAddrIn {
        family: AF_INET as u16,
        port_be: address.port.to_be(),
        address: address.address,
        zero: [0; 8],
    };
    crate::userland::usercopy::write_unaligned(pointer, &raw)?;
    crate::userland::usercopy::write_unaligned(
        length_pointer,
        &(core::mem::size_of::<LinuxSockAddrIn>() as u32),
    )
}

fn finish(result: i64) -> i64 {
    if result >= 0 {
        crate::userland::lifecycle::clear_network_wait();
    }
    result
}

fn block_for_socket(
    args: &SyscallArgs,
    id: u64,
    timeout_ticks: Option<u64>,
    nonblocking_error: i64,
    timed_out_error: i64,
) -> i64 {
    if socket::nonblocking(id).unwrap_or(true) {
        return nonblocking_error;
    }
    let deadline =
        match crate::userland::lifecycle::prepare_network_wait(args.rax, id, timeout_ticks) {
            Ok(deadline) => deadline,
            Err(()) => return timed_out_error,
        };
    unsafe {
        crate::userland::switch::block_current_ring3_and_yield(
            args,
            crate::userland::lifecycle::Ring3BlockReason::WaitingForNetwork {
                deadline_tick: deadline,
            },
        )
    }
}

pub fn socket_handler(args: &mut SyscallArgs) -> i64 {
    let domain = args.rdi as i32;
    let raw_type = args.rsi as i32;
    let protocol = args.rdx as i32;
    if domain != AF_INET {
        return EAFNOSUPPORT;
    }
    let unknown_flags = raw_type & !(0xf | SOCK_NONBLOCK | SOCK_CLOEXEC);
    if unknown_flags != 0 {
        return EINVAL;
    }
    let socket_type = match raw_type & 0xf {
        SOCK_STREAM if protocol == 0 || protocol == IPPROTO_TCP => SocketType::Stream,
        SOCK_DGRAM if protocol == 0 || protocol == IPPROTO_UDP => SocketType::Datagram,
        SOCK_RAW if protocol == IPPROTO_ICMP => SocketType::RawIcmp,
        SOCK_STREAM | SOCK_DGRAM | SOCK_RAW => return EPROTONOSUPPORT,
        _ => return EINVAL,
    };
    let handle = match socket::create(socket_type, raw_type & SOCK_NONBLOCK != 0) {
        Ok(handle) => handle,
        Err(error) => return map_socket_error(error),
    };
    let fd = with_fd_table_mut(|table| {
        table.alloc(FdSlot::Socket {
            handle,
            cloexec: raw_type & SOCK_CLOEXEC != 0,
        })
    });
    match fd {
        Some(fd) => fd as i64,
        None => {
            socket::drain_deferred_closes();
            EMFILE
        }
    }
}

pub fn bind_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let address = match read_sockaddr(args.rsi, args.rdx) {
        Ok(address) => address,
        Err(e) => return e,
    };
    socket::bind(id, address).map_or_else(map_socket_error, |_| finish(0))
}

pub fn connect_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let remote = match read_sockaddr(args.rsi, args.rdx) {
        Ok(address) => address,
        Err(e) => return e,
    };
    crate::net::poll_once();
    match socket::connect(id, remote) {
        Ok(()) => finish(0),
        Err(SocketError::InProgress) => {
            let send_timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.1);
            block_for_socket(args, id, send_timeout, EINPROGRESS, ETIMEDOUT)
        }
        Err(SocketError::Already) => {
            if socket::nonblocking(id).unwrap_or(true) {
                EALREADY
            } else {
                let send_timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.1);
                block_for_socket(args, id, send_timeout, EALREADY, ETIMEDOUT)
            }
        }
        Err(error) => {
            crate::userland::lifecycle::clear_network_wait();
            map_socket_error(error)
        }
    }
}

pub fn listen_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    // v1 backlog is deliberately clamped to one.
    let _backlog = (args.rsi as i32).max(0).min(1);
    socket::listen(id).map_or_else(map_socket_error, |_| finish(0))
}

fn accept_common(args: &SyscallArgs, flags: i32) -> i64 {
    if flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return EINVAL;
    }
    if let Err(error) = validate_sockaddr_output(args.rsi, args.rdx) {
        return error;
    }
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    crate::net::poll_once();
    match socket::accept(id, flags & SOCK_NONBLOCK != 0) {
        Ok((handle, peer)) => {
            let fd = with_fd_table_mut(|table| {
                table.alloc(FdSlot::Socket {
                    handle,
                    cloexec: flags & SOCK_CLOEXEC != 0,
                })
            });
            let Some(fd) = fd else {
                socket::drain_deferred_closes();
                return EMFILE;
            };
            if let Err(error) = write_sockaddr(args.rsi, args.rdx, peer) {
                let _ = with_fd_table_mut(|table| table.close(fd));
                socket::drain_deferred_closes();
                return error;
            }
            finish(fd as i64)
        }
        Err(SocketError::WouldBlock) => {
            let recv_timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.0);
            block_for_socket(args, id, recv_timeout, EAGAIN, EAGAIN)
        }
        Err(error) => map_socket_error(error),
    }
}

pub fn accept_handler(args: &mut SyscallArgs) -> i64 {
    accept_common(args, 0)
}

pub fn accept4_handler(args: &mut SyscallArgs) -> i64 {
    accept_common(args, args.r10 as i32)
}

fn send_common(
    args: &SyscallArgs,
    data: &[u8],
    destination: Option<SockAddrV4>,
    flags: i32,
) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    crate::net::poll_once();
    match socket::send(id, data, destination) {
        Ok(length) => finish(length as i64),
        Err(SocketError::WouldBlock) => {
            if flags & MSG_DONTWAIT != 0 {
                return EAGAIN;
            }
            let timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.1);
            block_for_socket(args, id, timeout, EAGAIN, EAGAIN)
        }
        Err(error) => map_socket_error(error),
    }
}

pub fn write_connected(args: &SyscallArgs, id: u64, data: &[u8]) -> i64 {
    crate::net::poll_once();
    match socket::send(id, data, None) {
        Ok(length) => finish(length as i64),
        Err(SocketError::WouldBlock) => {
            let timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.1);
            block_for_socket(args, id, timeout, EAGAIN, EAGAIN)
        }
        Err(error) => map_socket_error(error),
    }
}

pub fn sendto_handler(args: &mut SyscallArgs) -> i64 {
    let length = args.rdx as usize;
    if length > IO_MAX {
        return EMSGSIZE;
    }
    let mut data = vec![0; length];
    if let Err(error) = crate::userland::usercopy::copy_from_user(&mut data, args.rsi) {
        return error;
    }
    let destination = if args.r8 == 0 {
        None
    } else {
        match read_sockaddr(args.r8, args.r9) {
            Ok(address) => Some(address),
            Err(e) => return e,
        }
    };
    send_common(args, &data, destination, args.r10 as i32)
}

fn recv_common(
    args: &SyscallArgs,
    capacity: usize,
    flags: i32,
) -> Result<(Vec<u8>, Option<SockAddrV4>), i64> {
    let id = socket_id(args.rdi as i32)?;
    crate::net::poll_once();
    let mut data = vec![0; capacity.min(IO_MAX)];
    match socket::recv(id, &mut data) {
        Ok((length, source)) => {
            data.truncate(length);
            crate::userland::lifecycle::clear_network_wait();
            Ok((data, source))
        }
        Err(SocketError::WouldBlock) => {
            if flags & MSG_DONTWAIT != 0 || socket::nonblocking(id).unwrap_or(true) {
                return Err(EAGAIN);
            }
            let timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.0);
            Err(block_for_socket(args, id, timeout, EAGAIN, EAGAIN))
        }
        Err(error) => Err(map_socket_error(error)),
    }
}

pub fn read_connected(args: &SyscallArgs, id: u64, pointer: u64, capacity: usize) -> i64 {
    crate::net::poll_once();
    let mut data = vec![0; capacity.min(IO_MAX)];
    match socket::recv(id, &mut data) {
        Ok((length, _)) => {
            if let Err(error) = crate::userland::usercopy::copy_to_user(pointer, &data[..length]) {
                return error;
            }
            finish(length as i64)
        }
        Err(SocketError::WouldBlock) => {
            let timeout = socket::timeouts(id).ok().and_then(|timeouts| timeouts.0);
            block_for_socket(args, id, timeout, EAGAIN, EAGAIN)
        }
        Err(error) => map_socket_error(error),
    }
}

pub fn recvfrom_handler(args: &mut SyscallArgs) -> i64 {
    if let Err(error) = validate_sockaddr_output(args.r8, args.r9) {
        return error;
    }
    let (data, source) = match recv_common(args, args.rdx as usize, args.r10 as i32) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = crate::userland::usercopy::copy_to_user(args.rsi, &data) {
        return error;
    }
    if let Some(source) = source {
        if let Err(error) = write_sockaddr(args.r8, args.r9, source) {
            return error;
        }
    }
    finish(data.len() as i64)
}

fn read_iovecs(pointer: u64, count: u64) -> Result<Vec<LinuxIovec>, i64> {
    if count as usize > IOV_MAX {
        return Err(EINVAL);
    }
    let mut iovecs = Vec::with_capacity(count as usize);
    let mut total = 0u64;
    for index in 0..count {
        let address = pointer
            .checked_add(index * core::mem::size_of::<LinuxIovec>() as u64)
            .ok_or(EFAULT)?;
        let iovec = crate::userland::usercopy::read_unaligned::<LinuxIovec>(address)?;
        total = total.checked_add(iovec.len).ok_or(EINVAL)?;
        if total > IO_MAX as u64 {
            return Err(EMSGSIZE);
        }
        iovecs.push(iovec);
    }
    Ok(iovecs)
}

pub fn sendmsg_handler(args: &mut SyscallArgs) -> i64 {
    let message = match crate::userland::usercopy::read_unaligned::<LinuxMsghdr>(args.rsi) {
        Ok(value) => value,
        Err(e) => return e,
    };
    if message.control_len != 0 {
        return ENOPROTOOPT;
    }
    let iovecs = match read_iovecs(message.iov, message.iov_len) {
        Ok(value) => value,
        Err(e) => return e,
    };
    let total = iovecs.iter().map(|iov| iov.len as usize).sum();
    let mut data = Vec::with_capacity(total);
    for iovec in iovecs {
        let start = data.len();
        data.resize(start + iovec.len as usize, 0);
        if let Err(error) =
            crate::userland::usercopy::copy_from_user(&mut data[start..], iovec.base)
        {
            return error;
        }
    }
    let destination = if message.name == 0 {
        None
    } else {
        match read_sockaddr(message.name, message.name_len as u64) {
            Ok(value) => Some(value),
            Err(e) => return e,
        }
    };
    send_common(args, &data, destination, args.rdx as i32)
}

pub fn recvmsg_handler(args: &mut SyscallArgs) -> i64 {
    let mut message = match crate::userland::usercopy::read_unaligned::<LinuxMsghdr>(args.rsi) {
        Ok(value) => value,
        Err(e) => return e,
    };
    if message.control_len != 0 {
        return ENOPROTOOPT;
    }
    let iovecs = match read_iovecs(message.iov, message.iov_len) {
        Ok(value) => value,
        Err(e) => return e,
    };
    let capacity = iovecs.iter().map(|iov| iov.len as usize).sum();
    let (data, source) = match recv_common(args, capacity, args.rdx as i32) {
        Ok(value) => value,
        Err(e) => return e,
    };
    let mut copied = 0usize;
    for iovec in iovecs {
        let length = (data.len() - copied).min(iovec.len as usize);
        if length == 0 {
            break;
        }
        if let Err(error) =
            crate::userland::usercopy::copy_to_user(iovec.base, &data[copied..copied + length])
        {
            return error;
        }
        copied += length;
    }
    if let Some(source) = source {
        if message.name != 0 {
            let length_pointer = args.rsi + 8;
            if let Err(error) = write_sockaddr(message.name, length_pointer, source) {
                return error;
            }
            message.name_len = core::mem::size_of::<LinuxSockAddrIn>() as u32;
        }
    }
    message.flags = 0;
    if let Err(error) = crate::userland::usercopy::write_unaligned(args.rsi, &message) {
        return error;
    }
    finish(copied as i64)
}

pub fn getsockname_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let address = match socket::local_name(id) {
        Ok(address) => address,
        Err(error) => return map_socket_error(error),
    };
    write_sockaddr(args.rsi, args.rdx, address).map_or_else(|error| error, |_| 0)
}

pub fn getpeername_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let address = match socket::peer_name(id) {
        Ok(address) => address,
        Err(error) => return map_socket_error(error),
    };
    write_sockaddr(args.rsi, args.rdx, address).map_or_else(|e| e, |_| 0)
}

pub fn shutdown_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    socket::shutdown(id, args.rsi as i32).map_or_else(map_socket_error, |_| finish(0))
}

fn timeout_ticks(value: LinuxTimeval) -> Result<Option<u64>, i64> {
    if value.seconds < 0 || !(0..1_000_000).contains(&value.microseconds) {
        return Err(EINVAL);
    }
    if value.seconds == 0 && value.microseconds == 0 {
        return Ok(None);
    }
    let milliseconds = (value.seconds as u64)
        .saturating_mul(1000)
        .saturating_add((value.microseconds as u64 + 999) / 1000);
    Ok(Some((milliseconds + 9) / 10))
}

pub fn setsockopt_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let level = args.rsi as i32;
    let name = args.rdx as i32;
    let pointer = args.r10;
    let length = args.r8 as usize;
    let option = match (level, name) {
        (SOL_SOCKET, SO_REUSEADDR) if length >= 4 => SocketOption::ReuseAddress(
            match crate::userland::usercopy::read_unaligned::<i32>(pointer) {
                Ok(v) => v != 0,
                Err(e) => return e,
            },
        ),
        (SOL_SOCKET, SO_RCVTIMEO) if length >= core::mem::size_of::<LinuxTimeval>() => {
            SocketOption::ReceiveTimeout(
                match crate::userland::usercopy::read_unaligned(pointer).and_then(timeout_ticks) {
                    Ok(v) => v,
                    Err(e) => return e,
                },
            )
        }
        (SOL_SOCKET, SO_SNDTIMEO) if length >= core::mem::size_of::<LinuxTimeval>() => {
            SocketOption::SendTimeout(
                match crate::userland::usercopy::read_unaligned(pointer).and_then(timeout_ticks) {
                    Ok(v) => v,
                    Err(e) => return e,
                },
            )
        }
        (IPPROTO_IP, IP_TTL) if length >= 4 => {
            let ttl = match crate::userland::usercopy::read_unaligned::<i32>(pointer) {
                Ok(v) if (1..=255).contains(&v) => v as u8,
                Ok(_) => return EINVAL,
                Err(e) => return e,
            };
            SocketOption::Ttl(ttl)
        }
        (IPPROTO_TCP, TCP_NODELAY) if length >= 4 => SocketOption::TcpNoDelay(
            match crate::userland::usercopy::read_unaligned::<i32>(pointer) {
                Ok(v) => v != 0,
                Err(e) => return e,
            },
        ),
        _ => return ENOPROTOOPT,
    };
    socket::set_option(id, option).map_or_else(map_socket_error, |_| 0)
}

fn timeval_from_ticks(ticks: Option<u64>) -> LinuxTimeval {
    let milliseconds = ticks.unwrap_or(0).saturating_mul(10);
    LinuxTimeval {
        seconds: (milliseconds / 1000) as i64,
        microseconds: ((milliseconds % 1000) * 1000) as i64,
    }
}

fn write_option<T>(value_pointer: u64, length_pointer: u64, value: &T) -> Result<(), i64> {
    let available = crate::userland::usercopy::read_unaligned::<u32>(length_pointer)? as usize;
    if available < core::mem::size_of::<T>() {
        return Err(EINVAL);
    }
    crate::userland::usercopy::write_unaligned(value_pointer, value)?;
    crate::userland::usercopy::write_unaligned(length_pointer, &(core::mem::size_of::<T>() as u32))
}

pub fn getsockopt_handler(args: &mut SyscallArgs) -> i64 {
    let id = match socket_id(args.rdi as i32) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let snapshot = match socket::options(id) {
        Ok(value) => value,
        Err(error) => return map_socket_error(error),
    };
    let result = match (args.rsi as i32, args.rdx as i32) {
        (SOL_SOCKET, SO_TYPE) => write_option(
            args.r10,
            args.r8,
            &(match snapshot.socket_type {
                SocketType::Stream => SOCK_STREAM,
                SocketType::Datagram => SOCK_DGRAM,
                SocketType::RawIcmp => SOCK_RAW,
            }),
        ),
        (SOL_SOCKET, SO_ERROR) => write_option(
            args.r10,
            args.r8,
            &snapshot
                .pending_error
                .map(|error| -map_socket_error(error) as i32)
                .unwrap_or(0),
        ),
        (SOL_SOCKET, SO_REUSEADDR) => {
            write_option(args.r10, args.r8, &(snapshot.reuse_addr as i32))
        }
        (SOL_SOCKET, SO_RCVTIMEO) => write_option(
            args.r10,
            args.r8,
            &timeval_from_ticks(snapshot.recv_timeout_ticks),
        ),
        (SOL_SOCKET, SO_SNDTIMEO) => write_option(
            args.r10,
            args.r8,
            &timeval_from_ticks(snapshot.send_timeout_ticks),
        ),
        (IPPROTO_IP, IP_TTL) => write_option(args.r10, args.r8, &(snapshot.ttl as i32)),
        (IPPROTO_TCP, TCP_NODELAY) if snapshot.socket_type == SocketType::Stream => {
            write_option(args.r10, args.r8, &(snapshot.tcp_nodelay as i32))
        }
        _ => Err(ENOPROTOOPT),
    };
    result.map_or_else(|e| e, |_| 0)
}
