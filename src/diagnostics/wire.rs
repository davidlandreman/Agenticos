//! Versioned crash-capsule wire schema and allocation-free encoder.

pub const MAGIC: [u8; 8] = *b"AGCRASH\0";
pub const SCHEMA_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 80;
pub const SECTION_HEADER_LEN: usize = 16;

#[repr(C)]
pub struct CapsuleHeader {
    pub magic: [u8; 8],
    pub schema_version: u16,
    pub header_len: u16,
    pub total_len: u32,
    pub flags: u64,
    pub run_id: [u8; 16],
    pub build_id: [u8; 20],
    pub owner_cpu: u8,
    pub online_cpu_mask: u8,
    pub captured_cpu_mask: u8,
    pub record_kind: u8,
    pub record_sequence: u64,
    pub payload_crc32: u32,
    pub header_crc32: u32,
}

#[repr(C)]
pub struct SectionHeader {
    pub kind: u16,
    pub version: u16,
    pub len: u32,
    pub flags: u32,
    pub crc32: u32,
}

const _: () = {
    assert!(core::mem::size_of::<CapsuleHeader>() == HEADER_LEN);
    assert!(core::mem::size_of::<SectionHeader>() == SECTION_HEADER_LEN);
};

#[derive(Clone, Copy)]
#[repr(u16)]
pub enum SectionKind {
    RunMetadata = 1,
    Trigger = 2,
    CpuSnapshots = 3,
    TraceTail = 5,
    Violation = 10,
    Backtrace = 11,
    Footer = 12,
}

pub struct Writer<'a> {
    bytes: &'a mut [u8],
    len: usize,
    truncated: bool,
}

impl<'a> Writer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            len: 0,
            truncated: false,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn zeros(&mut self, count: usize) -> Option<usize> {
        let start = self.reserve(count)?;
        self.bytes[start..start + count].fill(0);
        Some(start)
    }

    fn reserve(&mut self, count: usize) -> Option<usize> {
        let end = match self.len.checked_add(count) {
            Some(end) if end <= self.bytes.len() => end,
            _ => {
                self.truncated = true;
                return None;
            }
        };
        let start = self.len;
        self.len = end;
        Some(start)
    }

    pub fn u8(&mut self, value: u8) -> bool {
        self.raw(&[value])
    }

    pub fn u16(&mut self, value: u16) -> bool {
        self.raw(&value.to_le_bytes())
    }

    pub fn u32(&mut self, value: u32) -> bool {
        self.raw(&value.to_le_bytes())
    }

    pub fn u64(&mut self, value: u64) -> bool {
        self.raw(&value.to_le_bytes())
    }

    pub fn raw(&mut self, value: &[u8]) -> bool {
        let Some(start) = self.reserve(value.len()) else {
            return false;
        };
        self.bytes[start..start + value.len()].copy_from_slice(value);
        true
    }

    pub fn patch_u32(&mut self, offset: usize, value: u32) {
        if offset + 4 <= self.len {
            self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    pub fn section<F>(&mut self, kind: SectionKind, version: u16, flags: u32, write: F)
    where
        F: FnOnce(&mut Self),
    {
        let header = self.len;
        if self.zeros(SECTION_HEADER_LEN).is_none() {
            return;
        }
        let payload = self.len;
        write(self);
        let payload_len = self.len - payload;
        let checksum = crc32(&self.bytes[payload..self.len]);
        self.bytes[header..header + 2].copy_from_slice(&(kind as u16).to_le_bytes());
        self.bytes[header + 2..header + 4].copy_from_slice(&version.to_le_bytes());
        self.bytes[header + 4..header + 8].copy_from_slice(&(payload_len as u32).to_le_bytes());
        self.bytes[header + 8..header + 12].copy_from_slice(&flags.to_le_bytes());
        self.bytes[header + 12..header + 16].copy_from_slice(&checksum.to_le_bytes());
    }
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

pub const fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
        index += 1;
    }
    hash
}
