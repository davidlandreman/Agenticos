//! QEMU `fw_cfg` reader.
//!
//! Used to pass small config blobs from the host (`-fw_cfg
//! name=opt/...,string=...`) into the running kernel without rebuilding. Pure
//! port I/O; safe to call before the heap is up. Returns silently when
//! `fw_cfg` is absent (e.g., real hardware).

use x86_64::instructions::port::Port;

const SELECTOR_PORT: u16 = 0x510;
const DATA_PORT: u16 = 0x511;

/// Standard fw_cfg selectors.
const SIGNATURE: u16 = 0x0000;
const FILE_DIR: u16 = 0x0019;

/// QEMU's fw_cfg signature ("QEMU"), little-endian byte order on the wire.
const SIGNATURE_BYTES: [u8; 4] = *b"QEMU";

/// Maximum file-name length QEMU exposes (per fw_cfg spec).
const FILE_NAME_LEN: usize = 56;

/// One entry in the file directory.
#[repr(C)]
struct FwCfgFile {
    size: [u8; 4],     // big-endian
    select: [u8; 2],   // big-endian
    _reserved: [u8; 2],
    name: [u8; FILE_NAME_LEN],
}

unsafe fn select(key: u16) {
    let mut port: Port<u16> = Port::new(SELECTOR_PORT);
    port.write(key);
}

unsafe fn read_byte() -> u8 {
    let mut port: Port<u8> = Port::new(DATA_PORT);
    port.read()
}

unsafe fn read_into(buf: &mut [u8]) {
    for slot in buf.iter_mut() {
        *slot = read_byte();
    }
}

/// Returns true when fw_cfg is present.
pub fn present() -> bool {
    let mut sig = [0u8; 4];
    unsafe {
        select(SIGNATURE);
        read_into(&mut sig);
    }
    sig == SIGNATURE_BYTES
}

/// Look up a named file in the fw_cfg directory.
///
/// Returns `Some((selector, size))` when found. The directory is small
/// (kilobytes at most) and we walk it linearly.
pub fn find_file(name: &str) -> Option<(u16, u32)> {
    if !present() {
        return None;
    }

    let count = unsafe {
        select(FILE_DIR);
        let mut hdr = [0u8; 4];
        read_into(&mut hdr);
        u32::from_be_bytes(hdr)
    };

    let mut entry = FwCfgFile {
        size: [0; 4],
        select: [0; 2],
        _reserved: [0; 2],
        name: [0; FILE_NAME_LEN],
    };

    for _ in 0..count {
        unsafe {
            read_into(&mut entry.size);
            read_into(&mut entry.select);
            read_into(&mut entry._reserved);
            read_into(&mut entry.name);
        }

        let nlen = entry
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(FILE_NAME_LEN);
        let entry_name = match core::str::from_utf8(&entry.name[..nlen]) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if entry_name == name {
            let size = u32::from_be_bytes(entry.size);
            let selector = u16::from_be_bytes(entry.select);
            return Some((selector, size));
        }
    }

    None
}

/// Read a fw_cfg file by name into a caller-provided buffer.
///
/// Returns the populated prefix on success. If the file is larger than the
/// buffer, only the buffer-sized prefix is read (caller decides whether to
/// treat truncation as an error). Returns `None` if fw_cfg is absent or the
/// file is missing.
pub fn read_file(name: &str, buf: &mut [u8]) -> Option<usize> {
    let (selector, size) = find_file(name)?;
    let to_read = core::cmp::min(buf.len(), size as usize);
    unsafe {
        select(selector);
        read_into(&mut buf[..to_read]);
    }
    Some(to_read)
}
