//! Length-prefixed framing over a `Com2` byte stream.
//!
//! Each frame is `[u32 LE header_len][JSON header][u32 LE binary_len][binary]`.
//! Header is always present (may be empty); binary trailer is empty when
//! `binary_len == 0`.

use alloc::vec::Vec;

use crate::drivers::serial::Com2;

pub const MAX_HEADER: usize = 64 * 1024;
pub const MAX_BINARY: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    OversizeHeader,
    OversizeBinary,
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub struct Frame {
    pub header: Vec<u8>,
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub binary: Option<Vec<u8>>,
}

/// Block until one byte is available on COM2. Yields between polls so the
/// dispatcher process does not hot-spin when traffic is idle.
fn read_byte_blocking(com: &Com2) -> u8 {
    loop {
        if let Some(b) = com.read_byte() {
            return b;
        }
        crate::process::sleep_ms(10);
    }
}

fn read_n(com: &Com2, n: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n);
    for _ in 0..n {
        buf.push(read_byte_blocking(com));
    }
    buf
}

fn read_u32_le(com: &Com2) -> u32 {
    let bytes = read_n(com, 4);
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Drain `n` bytes from COM2 and discard them. Used to stay in sync with the
/// peer when an oversize frame arrives (we cannot allocate to hold it, so we
/// drop it on the floor and resume).
fn drain_n(com: &Com2, n: usize) {
    for _ in 0..n {
        let _ = read_byte_blocking(com);
    }
}

/// Read one full frame from COM2. Blocks (with cooperative yields) until the
/// frame is complete. Oversize frames are drained from the wire and reported
/// as errors so the next frame parses cleanly.
pub fn read_frame(com: &Com2) -> Result<Frame, FrameError> {
    let header_len = read_u32_le(com) as usize;
    if header_len > MAX_HEADER {
        let binary_len = read_u32_le(com) as usize;
        // Keep the wire in sync — even if the binary is also nuts, drain it.
        // u32 max is 4 GiB; we draw the line at MAX_BINARY for sanity.
        let to_drain = header_len.saturating_add(binary_len.min(MAX_BINARY));
        drain_n(com, to_drain);
        return Err(FrameError::OversizeHeader);
    }
    let header = read_n(com, header_len);

    let binary_len = read_u32_le(com) as usize;
    if binary_len > MAX_BINARY {
        drain_n(com, binary_len.min(MAX_BINARY));
        return Err(FrameError::OversizeBinary);
    }
    let binary = if binary_len == 0 {
        None
    } else {
        Some(read_n(com, binary_len))
    };

    Ok(Frame { header, binary })
}

/// Write one frame to COM2. The header bytes are written first with a u32 LE
/// length prefix, then the optional binary trailer with its own u32 LE prefix
/// (zero when `binary` is `None`).
pub fn write_frame(com: &Com2, header: &[u8], binary: Option<&[u8]>) {
    let header_len = header.len() as u32;
    com.write_all(&header_len.to_le_bytes());
    com.write_all(header);

    let binary_len = binary.map(|b| b.len()).unwrap_or(0) as u32;
    com.write_all(&binary_len.to_le_bytes());
    if let Some(b) = binary {
        com.write_all(b);
    }
}
