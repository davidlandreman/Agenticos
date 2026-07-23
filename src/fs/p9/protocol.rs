//! 9P2000.L wire codec.
//!
//! Little-endian framing: every message is `size[4] type[1] tag[2] payload`,
//! strings are `len[2] bytes`, qids are `type[1] version[4] path[8]`. Only
//! the message subset the `/shared` client needs is represented.

use crate::fs::filesystem::FilesystemError;
use alloc::string::String;
use alloc::vec::Vec;

pub const NOTAG: u16 = 0xFFFF;
pub const NOFID: u32 = u32::MAX;

/// Representative ordinary tag used by the wire-codec tests. Production
/// client lanes assign distinct tags so requests can be multiplexed.
#[cfg(feature = "test")]
pub const TAG: u16 = 1;

/// 9P2000.L message types.
pub mod msg {
    pub const RLERROR: u8 = 7;
    pub const TSTATFS: u8 = 8;
    pub const RSTATFS: u8 = 9;
    pub const TLOPEN: u8 = 12;
    pub const RLOPEN: u8 = 13;
    pub const TLCREATE: u8 = 14;
    pub const RLCREATE: u8 = 15;
    pub const TSYMLINK: u8 = 16;
    pub const RSYMLINK: u8 = 17;
    pub const TREADLINK: u8 = 22;
    pub const RREADLINK: u8 = 23;
    pub const TGETATTR: u8 = 24;
    pub const RGETATTR: u8 = 25;
    pub const TSETATTR: u8 = 26;
    pub const RSETATTR: u8 = 27;
    pub const TREADDIR: u8 = 40;
    pub const RREADDIR: u8 = 41;
    pub const TFSYNC: u8 = 50;
    pub const RFSYNC: u8 = 51;
    pub const TLINK: u8 = 70;
    pub const RLINK: u8 = 71;
    pub const TMKDIR: u8 = 72;
    pub const RMKDIR: u8 = 73;
    pub const TRENAMEAT: u8 = 74;
    pub const RRENAMEAT: u8 = 75;
    pub const TUNLINKAT: u8 = 76;
    pub const RUNLINKAT: u8 = 77;
    pub const TVERSION: u8 = 100;
    pub const RVERSION: u8 = 101;
    pub const TATTACH: u8 = 104;
    pub const RATTACH: u8 = 105;
    pub const TWALK: u8 = 110;
    pub const RWALK: u8 = 111;
    pub const TREAD: u8 = 116;
    pub const RREAD: u8 = 117;
    pub const TWRITE: u8 = 118;
    pub const RWRITE: u8 = 119;
    pub const TCLUNK: u8 = 120;
    pub const RCLUNK: u8 = 121;
}

/// Linux open(2) flag values as used by Tlopen/Tlcreate (x86-64 numeric).
pub mod open_flags {
    pub const O_RDONLY: u32 = 0;
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const O_WRONLY: u32 = 0o1;
    pub const O_RDWR: u32 = 0o2;
    pub const O_TRUNC: u32 = 0o1000;
    pub const O_DIRECTORY: u32 = 0o200000;
}

/// Tsetattr `valid` mask bits.
pub mod setattr_valid {
    pub const SIZE: u32 = 0x8;
    pub const ATIME: u32 = 0x10;
    pub const MTIME: u32 = 0x20;
    pub const ATIME_SET: u32 = 0x80;
    pub const MTIME_SET: u32 = 0x100;
}

/// Tgetattr request mask covering every field the kernel consumes.
pub const GETATTR_BASIC: u64 = 0x0000_07FF;

/// Tunlinkat flag selecting rmdir semantics.
pub const AT_REMOVEDIR: u32 = 0x200;

/// Maximum walk elements per Twalk (spec MAXWELEM).
pub const MAX_WELEM: usize = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Qid {
    pub kind: u8,
    pub version: u32,
    pub path: u64,
}

impl Qid {
    pub const TYPE_DIR: u8 = 0x80;
    pub const TYPE_SYMLINK: u8 = 0x02;

    pub fn is_dir(&self) -> bool {
        self.kind & Self::TYPE_DIR != 0
    }

    pub fn is_symlink(&self) -> bool {
        self.kind & Self::TYPE_SYMLINK != 0
    }
}

/// The Rgetattr fields the kernel consumes, including host nanoseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct P9Stat {
    pub qid: Qid,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub size: u64,
    pub blksize: u64,
    pub blocks: u64,
    pub atime_sec: u64,
    pub atime_nsec: u32,
    pub mtime_sec: u64,
    pub mtime_nsec: u32,
    pub ctime_sec: u64,
    pub ctime_nsec: u32,
}

/// One Rreaddir entry. `type_byte` is a Linux `d_type` value. The entry's
/// qid is parsed but not carried: consumers stat through a walked fid.
#[derive(Debug, Clone)]
pub struct P9Dirent {
    pub offset: u64,
    pub type_byte: u8,
    pub name: String,
}

/// Map an Rlerror errno onto the kernel filesystem error surface.
pub fn map_errno(errno: u32) -> FilesystemError {
    match errno {
        1 | 13 => FilesystemError::PermissionDenied, // EPERM, EACCES
        2 => FilesystemError::NotFound,              // ENOENT
        17 => FilesystemError::AlreadyExists,        // EEXIST
        20 => FilesystemError::NotADirectory,        // ENOTDIR
        21 => FilesystemError::IsADirectory,         // EISDIR
        28 => FilesystemError::DiskFull,             // ENOSPC
        30 => FilesystemError::ReadOnly,             // EROFS
        36 => FilesystemError::InvalidPath,          // ENAMETOOLONG
        39 | 66 => FilesystemError::NotEmpty,        // ENOTEMPTY (Linux, SUS)
        _ => FilesystemError::IoError,
    }
}

/// T-message builder. The size prefix is backpatched by `finish`.
pub struct WireWriter {
    buf: Vec<u8>,
}

impl WireWriter {
    pub fn request(kind: u8, tag: u16) -> Self {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&[0, 0, 0, 0, kind]);
        buf.extend_from_slice(&tag.to_le_bytes());
        Self { buf }
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.buf.push(value);
        self
    }

    pub fn u16(&mut self, value: u16) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u32(&mut self, value: u32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// 9P string: len[2] followed by UTF-8 bytes, no terminator.
    pub fn string(&mut self, value: &str) -> &mut Self {
        let bytes = value.as_bytes();
        let len = bytes.len().min(u16::MAX as usize);
        self.u16(len as u16);
        self.buf.extend_from_slice(&bytes[..len]);
        self
    }

    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(value);
        self
    }

    pub fn finish(mut self) -> Vec<u8> {
        let size = self.buf.len() as u32;
        self.buf[..4].copy_from_slice(&size.to_le_bytes());
        self.buf
    }
}

/// Checked little-endian reader over one R-message.
pub struct WireReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> WireReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8], FilesystemError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.buf.len())
            .ok_or(FilesystemError::IoError)?;
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, FilesystemError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, FilesystemError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, FilesystemError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, FilesystemError> {
        let bytes = self.take(8)?;
        let mut raw = [0u8; 8];
        raw.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(raw))
    }

    pub fn qid(&mut self) -> Result<Qid, FilesystemError> {
        Ok(Qid {
            kind: self.u8()?,
            version: self.u32()?,
            path: self.u64()?,
        })
    }

    pub fn string(&mut self) -> Result<String, FilesystemError> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes)
            .map(String::from)
            .map_err(|_| FilesystemError::IoError)
    }

    /// Parse the Rgetattr payload (after valid[8]) — caller has consumed the
    /// header and reads `valid` itself if it cares.
    pub fn rgetattr_body(&mut self) -> Result<P9Stat, FilesystemError> {
        let qid = self.qid()?;
        let mode = self.u32()?;
        let uid = self.u32()?;
        let gid = self.u32()?;
        let nlink = self.u64()?;
        let _rdev = self.u64()?;
        let size = self.u64()?;
        let blksize = self.u64()?;
        let blocks = self.u64()?;
        let atime_sec = self.u64()?;
        let atime_nsec = self.u64()?;
        let mtime_sec = self.u64()?;
        let mtime_nsec = self.u64()?;
        let ctime_sec = self.u64()?;
        let ctime_nsec = self.u64()?;
        // btime/gen/data_version follow; callers don't consume them.
        Ok(P9Stat {
            qid,
            mode,
            uid,
            gid,
            nlink,
            size,
            blksize,
            blocks,
            atime_sec,
            atime_nsec: atime_nsec.min(999_999_999) as u32,
            mtime_sec,
            mtime_nsec: mtime_nsec.min(999_999_999) as u32,
            ctime_sec,
            ctime_nsec: ctime_nsec.min(999_999_999) as u32,
        })
    }

    /// Parse one Rreaddir data-region entry.
    pub fn dirent(&mut self) -> Result<P9Dirent, FilesystemError> {
        let _qid = self.qid()?;
        let offset = self.u64()?;
        let type_byte = self.u8()?;
        let name = self.string()?;
        Ok(P9Dirent {
            offset,
            type_byte,
            name,
        })
    }
}
