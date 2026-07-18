//! Text-only guest ↔ host clipboard transport.
//!
//! QEMU exposes COM3 as a private Unix-socket chardev. A small host bridge
//! translates the bounded binary protocol below to the host's native text
//! clipboard commands. Clipboard calls are serialized because the wire is a
//! single request/response stream.

use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::drivers::serial::Com3;
use crate::userland::abi::{EINVAL, EIO, EMSGSIZE, EOPNOTSUPP, ETIMEDOUT};

pub const OP_COPY: u8 = 1;
pub const OP_PASTE: u8 = 2;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

const REQUEST_MAGIC: [u8; 4] = *b"ACCB";
const RESPONSE_MAGIC: [u8; 4] = *b"ACBR";
const PROTOCOL_VERSION: u8 = 1;
const HEADER_LEN: usize = 10;
const RESPONSE_TIMEOUT_TICKS: u64 = 6_000;

const STATUS_OK: u8 = 0;
const STATUS_BAD_REQUEST: u8 = 1;
const STATUS_UNSUPPORTED: u8 = 2;
const STATUS_HOST_ERROR: u8 = 3;
const STATUS_TOO_LARGE: u8 = 4;
const STATUS_INVALID_TEXT: u8 = 5;

static TRANSACTION: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHeader {
    pub status: u8,
    pub payload_len: usize,
}

pub fn request_header(op: u8, payload_len: usize) -> Result<[u8; HEADER_LEN], i64> {
    let payload_len = u32::try_from(payload_len).map_err(|_| EMSGSIZE)?;
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&REQUEST_MAGIC);
    header[4] = PROTOCOL_VERSION;
    header[5] = op;
    header[6..10].copy_from_slice(&payload_len.to_le_bytes());
    Ok(header)
}

pub fn parse_response_header(header: &[u8; HEADER_LEN]) -> Result<ResponseHeader, i64> {
    if header[..4] != RESPONSE_MAGIC || header[4] != PROTOCOL_VERSION {
        return Err(EIO);
    }
    let payload_len = u32::from_le_bytes([header[6], header[7], header[8], header[9]]) as usize;
    if payload_len > MAX_TEXT_BYTES {
        return Err(EMSGSIZE);
    }
    Ok(ResponseHeader {
        status: header[5],
        payload_len,
    })
}

pub fn status_errno(status: u8) -> i64 {
    match status {
        STATUS_BAD_REQUEST | STATUS_INVALID_TEXT => EINVAL,
        STATUS_UNSUPPORTED => EOPNOTSUPP,
        STATUS_TOO_LARGE => EMSGSIZE,
        STATUS_HOST_ERROR => EIO,
        _ => EIO,
    }
}

fn read_exact(com: &Com3, destination: &mut [u8], deadline: u64) -> Result<(), i64> {
    for slot in destination {
        loop {
            if let Some(byte) = com.read_byte() {
                *slot = byte;
                break;
            }
            if crate::arch::x86_64::interrupts::get_timer_ticks() >= deadline {
                return Err(ETIMEDOUT);
            }
            // SYSCALL entry masks IF. We are on the process's kernel stack at
            // this point, so it is safe to wait for the next timer interrupt;
            // the 10 ms tick also bounds polling latency for the non-IRQ UART.
            x86_64::instructions::interrupts::enable_and_hlt();
        }
    }
    Ok(())
}

fn transact(op: u8, request_payload: &[u8]) -> Result<Vec<u8>, i64> {
    let com = crate::drivers::serial::com3().ok_or(EIO)?;
    let _transaction = TRANSACTION.lock();
    let header = request_header(op, request_payload.len())?;
    // SYSCALL masks IF, but a maximum-size UART transfer can take seconds.
    // The kernel stack is established and the transaction lock is held, so
    // re-enable interrupts before sending to keep timers and input responsive.
    x86_64::instructions::interrupts::enable();
    com.write_all(&header);
    com.write_all(request_payload);

    let deadline =
        crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(RESPONSE_TIMEOUT_TICKS);
    let mut raw_response = [0u8; HEADER_LEN];
    read_exact(com, &mut raw_response, deadline)?;
    let response = parse_response_header(&raw_response)?;
    let mut payload = alloc::vec![0u8; response.payload_len];
    read_exact(com, &mut payload, deadline)?;

    if response.status != STATUS_OK {
        return Err(status_errno(response.status));
    }
    Ok(payload)
}

/// `clipboard(op, buffer, len_or_capacity) -> byte_count | -errno`.
///
/// COPY consumes exactly `len` UTF-8 bytes and returns zero. PASTE fills up
/// to `capacity` bytes and returns the exact byte count. Both directions are
/// deliberately text-only and bounded to [`MAX_TEXT_BYTES`].
pub fn syscall_handler(args: &SyscallArgs) -> i64 {
    let op = match u8::try_from(args.rdi) {
        Ok(op) => op,
        Err(_) => return EINVAL,
    };
    let pointer = args.rsi;
    let length = match usize::try_from(args.rdx) {
        Ok(length) if length <= MAX_TEXT_BYTES => length,
        _ => return EMSGSIZE,
    };

    match op {
        OP_COPY => {
            let mut text = alloc::vec![0u8; length];
            if let Err(error) = crate::userland::usercopy::copy_from_user(&mut text, pointer) {
                return error;
            }
            if core::str::from_utf8(&text).is_err() {
                return EINVAL;
            }
            match transact(OP_COPY, &text) {
                Ok(payload) if payload.is_empty() => 0,
                Ok(_) => EIO,
                Err(error) => error,
            }
        }
        OP_PASTE => {
            let text = match transact(OP_PASTE, &[]) {
                Ok(text) => text,
                Err(error) => return error,
            };
            if text.len() > length {
                return EMSGSIZE;
            }
            if core::str::from_utf8(&text).is_err() {
                return EINVAL;
            }
            if let Err(error) = crate::userland::usercopy::copy_to_user(pointer, &text) {
                return error;
            }
            text.len() as i64
        }
        _ => EINVAL,
    }
}

/// Bring up COM3 only for normal interactive QEMU boots. Physical hardware
/// and the in-kernel test boot leave the channel unavailable and calls fail
/// closed with `EIO`.
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub fn init() {
    crate::drivers::serial::init_clipboard();
}
