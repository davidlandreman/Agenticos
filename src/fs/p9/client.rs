//! Serialized 9P2000.L client over the virtio-9p transport.
//!
//! One request is in flight at a time (single fixed tag); callers serialize
//! behind the `P9Filesystem` lock. Fids are allocated from a free list; fid 0
//! is the attach root and lives for the mount's lifetime. Transport failures
//! quarantine the channel and every subsequent operation fails with
//! `IoError` rather than wedging or retrying from a desynced stream.

use crate::drivers::virtio::p9::P9Transport;
use crate::fs::filesystem::FilesystemError;
use crate::fs::filesystem::UnixTimestamp;
use crate::fs::p9::protocol::{
    map_errno, msg, P9Dirent, P9Stat, Qid, WireReader, WireWriter, GETATTR_BASIC, MAX_WELEM, NOFID,
    NOTAG,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

pub const ROOT_FID: u32 = 0;

/// Keep each concurrently attached client lane in a disjoint fid namespace.
const FID_LANE_STRIDE: u32 = 1 << 24;

/// msize requested from the server; the negotiated value may be lower.
const REQUEST_MSIZE: u32 = 64 * 1024;

/// Wire overhead reserved when sizing read/write/readdir payloads:
/// the largest applicable header is Twrite's 23 bytes, rounded up.
const IO_HEADER_RESERVE: u32 = 32;

/// 9P message header: size[4] type[1] tag[2].
const HEADER_LEN: usize = 7;

/// Bound reusable path fids so a mount cannot grow kernel memory without
/// limit. Metadata itself is never cached; every hit still sends Tgetattr.
const MAX_PATH_FIDS: usize = 16 * 1024;

/// Bound on symlink-target resolution recursion in the backend.
pub const MAX_SYMLINK_DEPTH: u8 = 8;

pub struct P9Client {
    transport: P9Transport,
    msize: u32,
    response: Vec<u8>,
    root_fid: u32,
    tag: u16,
    next_fid: u32,
    free_fids: Vec<u32>,
    path_fids: BTreeMap<String, u32>,
    read_ahead_fid: u32,
    read_ahead_offset: u64,
    read_ahead: Vec<u8>,
    poisoned: bool,
}

pub struct Statfs {
    pub bsize: u32,
    pub blocks: u64,
    pub bfree: u64,
    pub files: u64,
    pub ffree: u64,
}

impl P9Client {
    pub fn new(transport: P9Transport) -> Self {
        Self {
            transport,
            msize: REQUEST_MSIZE,
            response: vec![0u8; 8192],
            root_fid: ROOT_FID,
            tag: 1,
            next_fid: ROOT_FID + 1,
            free_fids: Vec::new(),
            path_fids: BTreeMap::new(),
            read_ahead_fid: NOFID,
            read_ahead_offset: 0,
            read_ahead: Vec::new(),
            poisoned: false,
        }
    }

    /// Attach another independently tagged lane after the primary lane has
    /// negotiated the connection version. Tversion is connection-wide and
    /// must not be repeated because it invalidates every existing fid.
    pub fn attach_lane(
        transport: P9Transport,
        msize: u32,
        lane: u16,
    ) -> Result<Self, FilesystemError> {
        let root_fid = u32::from(lane)
            .checked_mul(FID_LANE_STRIDE)
            .ok_or(FilesystemError::IoError)?;
        let mut client = Self {
            transport,
            msize,
            response: vec![0u8; msize as usize],
            root_fid,
            tag: lane.checked_add(1).ok_or(FilesystemError::IoError)?,
            next_fid: root_fid + 1,
            free_fids: Vec::new(),
            path_fids: BTreeMap::new(),
            read_ahead_fid: NOFID,
            read_ahead_offset: 0,
            read_ahead: Vec::with_capacity(msize.saturating_sub(IO_HEADER_RESERVE) as usize),
            poisoned: false,
        };
        client.attach()?;
        Ok(client)
    }

    /// Version + attach. Must succeed exactly once before any other op.
    pub fn handshake(&mut self) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TVERSION, NOTAG);
        writer.u32(REQUEST_MSIZE).string("9P2000.L");
        let end = self.rpc(writer.finish(), msg::RVERSION, NOTAG)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let server_msize = reader.u32()?;
        let server_version = reader.string()?;
        if server_version != "9P2000.L" {
            return Err(FilesystemError::UnsupportedFeature);
        }
        if server_msize < 4096 {
            return Err(FilesystemError::UnsupportedFeature);
        }
        self.msize = server_msize.min(REQUEST_MSIZE);
        self.response = vec![0u8; self.msize as usize];
        self.read_ahead = Vec::with_capacity(self.io_unit());

        self.attach()
    }

    fn attach(&mut self) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TATTACH, self.tag);
        writer
            .u32(self.root_fid)
            .u32(NOFID)
            .string("root")
            .string("")
            .u32(0);
        let end = self.rpc(writer.finish(), msg::RATTACH, self.tag)?;
        WireReader::new(&self.response[HEADER_LEN..end]).qid()?;
        Ok(())
    }

    pub fn negotiated_msize(&self) -> u32 {
        self.msize
    }

    pub fn root_fid(&self) -> u32 {
        self.root_fid
    }

    /// Largest read/write/readdir payload that fits the negotiated msize.
    pub fn io_unit(&self) -> usize {
        self.msize.saturating_sub(IO_HEADER_RESERVE) as usize
    }

    fn alloc_fid(&mut self) -> u32 {
        self.free_fids.pop().unwrap_or_else(|| {
            let fid = self.next_fid;
            self.next_fid = self.next_fid.wrapping_add(1).max(self.root_fid + 1);
            fid
        })
    }

    fn release_fid(&mut self, fid: u32) {
        if fid != self.root_fid {
            self.free_fids.push(fid);
        }
    }

    fn poison(&mut self) {
        self.poisoned = true;
    }

    /// Send one T-message and validate the R-message envelope. Returns the
    /// R-message end offset within the response buffer.
    fn rpc(
        &mut self,
        request: Vec<u8>,
        expected_type: u8,
        expected_tag: u16,
    ) -> Result<usize, FilesystemError> {
        if self.poisoned {
            return Err(FilesystemError::IoError);
        }
        if request.len() > self.msize as usize {
            return Err(FilesystemError::IoError);
        }
        let used = match self.transport.rpc(&request, &mut self.response) {
            Ok(used) => used,
            Err(_) => {
                // The transport quarantines itself; mirror that here so the
                // failure is sticky at this layer too.
                self.poison();
                return Err(FilesystemError::IoError);
            }
        };
        let mut header = WireReader::new(&self.response[..used]);
        let size = header.u32()? as usize;
        let kind = header.u8()?;
        let tag = header.u16()?;
        if size < HEADER_LEN || size > used || tag != expected_tag {
            self.poison();
            return Err(FilesystemError::IoError);
        }
        if kind == msg::RLERROR {
            let errno = WireReader::new(&self.response[HEADER_LEN..size]).u32()?;
            return Err(map_errno(errno));
        }
        if kind != expected_type {
            self.poison();
            return Err(FilesystemError::IoError);
        }
        Ok(size)
    }

    /// Walk `names` starting at `start_fid`, returning a fresh fid. Chunked
    /// at MAXWELEM. On any failure no new fid survives server-side.
    pub fn walk(&mut self, start_fid: u32, names: &[&str]) -> Result<u32, FilesystemError> {
        let newfid = self.alloc_fid();
        let mut source = start_fid;
        let mut walked_any = false;
        let mut chunks = names.chunks(MAX_WELEM);
        loop {
            let chunk: &[&str] = chunks.next().unwrap_or(&[]);
            if chunk.is_empty() && walked_any {
                break;
            }
            let mut writer = WireWriter::request(msg::TWALK, self.tag);
            writer.u32(source).u32(newfid).u16(chunk.len() as u16);
            for name in chunk {
                writer.string(name);
            }
            let end = match self.rpc(writer.finish(), msg::RWALK, self.tag) {
                Ok(end) => end,
                Err(error) => {
                    // A failed Twalk never creates newfid; a failed later
                    // chunk (source == newfid) leaves it at the intermediate
                    // directory, which must be clunked.
                    if walked_any {
                        let _ = self.clunk(newfid);
                    } else {
                        self.release_fid(newfid);
                    }
                    return Err(error);
                }
            };
            let nwqid = WireReader::new(&self.response[HEADER_LEN..end]).u16()? as usize;
            if nwqid < chunk.len() {
                // Partial walk: newfid was not created by this Twalk.
                if walked_any {
                    let _ = self.clunk(newfid);
                } else {
                    self.release_fid(newfid);
                }
                return Err(FilesystemError::NotFound);
            }
            walked_any = true;
            source = newfid;
            if chunk.len() < MAX_WELEM {
                break;
            }
        }
        Ok(newfid)
    }

    /// Walk from the attach root along `path` ("/a/b", "a/b", "/" and ""
    /// are all accepted; "." components are dropped).
    pub fn walk_path(&mut self, path: &str) -> Result<u32, FilesystemError> {
        let names: Vec<&str> = path
            .split('/')
            .filter(|component| !component.is_empty() && *component != ".")
            .collect();
        self.walk(self.root_fid, &names)
    }

    pub fn clunk(&mut self, fid: u32) -> Result<(), FilesystemError> {
        if self.read_ahead_fid == fid {
            self.read_ahead_fid = NOFID;
            self.read_ahead.clear();
        }
        let mut writer = WireWriter::request(msg::TCLUNK, self.tag);
        writer.u32(fid);
        let result = self.rpc(writer.finish(), msg::RCLUNK, self.tag).map(|_| ());
        // The fid number is retired locally even if the server errored; a
        // stale server-side fid surfaces as a later walk error, never as
        // silent reuse of live state.
        self.release_fid(fid);
        result
    }

    pub fn getattr(&mut self, fid: u32) -> Result<P9Stat, FilesystemError> {
        let mut writer = WireWriter::request(msg::TGETATTR, self.tag);
        writer.u32(fid).u64(GETATTR_BASIC);
        let end = self.rpc(writer.finish(), msg::RGETATTR, self.tag)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let _valid = reader.u64()?;
        reader.rgetattr_body()
    }

    /// Get fresh attributes for a path while retaining its walked fid for
    /// later calls. A cached fid removes Twalk/Tclunk traffic only; Tgetattr
    /// always reaches the server, preserving live size/time/status checks.
    pub fn getattr_path_cached(&mut self, path: &str) -> Result<P9Stat, FilesystemError> {
        if let Some(fid) = self.path_fids.get(path).copied() {
            match self.getattr(fid) {
                Ok(stat) => return Ok(stat),
                Err(_) => {
                    self.path_fids.remove(path);
                    let _ = self.clunk(fid);
                }
            }
        }

        let fid = self.walk_path(path)?;
        let stat = match self.getattr(fid) {
            Ok(stat) => stat,
            Err(error) => {
                let _ = self.clunk(fid);
                return Err(error);
            }
        };
        if self.path_fids.len() < MAX_PATH_FIDS {
            self.path_fids.insert(path.to_string(), fid);
        } else {
            let _ = self.clunk(fid);
        }
        Ok(stat)
    }

    /// Drop reusable fids for a mutated path and its descendants. Callers
    /// invoke this on every lane after an in-guest namespace mutation.
    pub fn invalidate_path(&mut self, path: &str) {
        let descendant_prefix = alloc::format!("{}/", path.trim_end_matches('/'));
        let stale: Vec<String> = self
            .path_fids
            .keys()
            .filter(|cached| *cached == path || cached.starts_with(&descendant_prefix))
            .cloned()
            .collect();
        for key in stale {
            if let Some(fid) = self.path_fids.remove(&key) {
                let _ = self.clunk(fid);
            }
        }
    }

    /// Tsetattr limited to the fields the kernel mutates: size, atime, mtime.
    pub fn setattr(
        &mut self,
        fid: u32,
        valid: u32,
        size: u64,
        atime: UnixTimestamp,
        mtime: UnixTimestamp,
    ) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TSETATTR, self.tag);
        writer
            .u32(fid)
            .u32(valid)
            .u32(0) // mode
            .u32(0) // uid
            .u32(0) // gid
            .u64(size)
            .u64(atime.seconds)
            .u64(atime.nanoseconds as u64)
            .u64(mtime.seconds)
            .u64(mtime.nanoseconds as u64);
        self.rpc(writer.finish(), msg::RSETATTR, self.tag)
            .map(|_| ())
    }

    pub fn lopen(&mut self, fid: u32, flags: u32) -> Result<Qid, FilesystemError> {
        let mut writer = WireWriter::request(msg::TLOPEN, self.tag);
        writer.u32(fid).u32(flags);
        let end = self.rpc(writer.finish(), msg::RLOPEN, self.tag)?;
        WireReader::new(&self.response[HEADER_LEN..end]).qid()
    }

    /// Create `name` under the directory `dfid`; on success `dfid` becomes
    /// the open fid of the new file.
    pub fn lcreate(
        &mut self,
        dfid: u32,
        name: &str,
        flags: u32,
        mode: u32,
    ) -> Result<Qid, FilesystemError> {
        let mut writer = WireWriter::request(msg::TLCREATE, self.tag);
        writer.u32(dfid).string(name).u32(flags).u32(mode).u32(0);
        let end = self.rpc(writer.finish(), msg::RLCREATE, self.tag)?;
        WireReader::new(&self.response[HEADER_LEN..end]).qid()
    }

    pub fn read(
        &mut self,
        fid: u32,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, FilesystemError> {
        if out.is_empty() {
            return Ok(0);
        }
        let cached_end = self
            .read_ahead_offset
            .saturating_add(self.read_ahead.len() as u64);
        if self.read_ahead_fid == fid && offset >= self.read_ahead_offset && offset < cached_end {
            let start = (offset - self.read_ahead_offset) as usize;
            let count = out.len().min(self.read_ahead.len() - start);
            out[..count].copy_from_slice(&self.read_ahead[start..start + count]);
            return Ok(count);
        }

        // Guest libc and Git commonly issue 4 KiB reads. Fetch one negotiated
        // 9p window and serve subsequent sequential reads from it, reducing
        // VM exits without caching metadata or retaining data after close.
        let count = self.io_unit() as u32;
        let mut writer = WireWriter::request(msg::TREAD, self.tag);
        writer.u32(fid).u64(offset).u32(count);
        let end = self.rpc(writer.finish(), msg::RREAD, self.tag)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let returned = reader.u32()? as usize;
        let data = reader.take(returned)?;
        if returned > self.io_unit() {
            self.poison();
            return Err(FilesystemError::IoError);
        }
        self.read_ahead.clear();
        self.read_ahead.extend_from_slice(data);
        self.read_ahead_fid = fid;
        self.read_ahead_offset = offset;
        let copied = out.len().min(returned);
        out[..copied].copy_from_slice(&self.read_ahead[..copied]);
        Ok(copied)
    }

    pub fn write(&mut self, fid: u32, offset: u64, data: &[u8]) -> Result<usize, FilesystemError> {
        if self.read_ahead_fid == fid {
            self.read_ahead_fid = NOFID;
            self.read_ahead.clear();
        }
        let count = data.len().min(self.io_unit());
        let mut writer = WireWriter::request(msg::TWRITE, self.tag);
        writer.u32(fid).u64(offset).u32(count as u32);
        writer.bytes(&data[..count]);
        let end = self.rpc(writer.finish(), msg::RWRITE, self.tag)?;
        let written = WireReader::new(&self.response[HEADER_LEN..end]).u32()? as usize;
        if written > count {
            self.poison();
            return Err(FilesystemError::IoError);
        }
        Ok(written)
    }

    /// One Rreaddir page. An empty result means end-of-directory.
    pub fn readdir(&mut self, fid: u32, offset: u64) -> Result<Vec<P9Dirent>, FilesystemError> {
        let count = self.io_unit() as u32;
        let mut writer = WireWriter::request(msg::TREADDIR, self.tag);
        writer.u32(fid).u64(offset).u32(count);
        let end = self.rpc(writer.finish(), msg::RREADDIR, self.tag)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let data_len = reader.u32()? as usize;
        let mut data = WireReader::new(reader.take(data_len)?);
        let mut entries = Vec::new();
        while data.remaining() > 0 {
            entries.push(data.dirent()?);
        }
        Ok(entries)
    }

    pub fn mkdir(&mut self, dfid: u32, name: &str, mode: u32) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TMKDIR, self.tag);
        writer.u32(dfid).string(name).u32(mode).u32(0);
        self.rpc(writer.finish(), msg::RMKDIR, self.tag).map(|_| ())
    }

    pub fn unlinkat(&mut self, dfid: u32, name: &str, flags: u32) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TUNLINKAT, self.tag);
        writer.u32(dfid).string(name).u32(flags);
        self.rpc(writer.finish(), msg::RUNLINKAT, self.tag)
            .map(|_| ())
    }

    pub fn renameat(
        &mut self,
        old_dfid: u32,
        old_name: &str,
        new_dfid: u32,
        new_name: &str,
    ) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TRENAMEAT, self.tag);
        writer
            .u32(old_dfid)
            .string(old_name)
            .u32(new_dfid)
            .string(new_name);
        self.rpc(writer.finish(), msg::RRENAMEAT, self.tag)
            .map(|_| ())
    }

    pub fn symlink(&mut self, dfid: u32, name: &str, target: &str) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TSYMLINK, self.tag);
        writer.u32(dfid).string(name).string(target).u32(0);
        self.rpc(writer.finish(), msg::RSYMLINK, self.tag)
            .map(|_| ())
    }

    pub fn readlink(&mut self, fid: u32) -> Result<String, FilesystemError> {
        let mut writer = WireWriter::request(msg::TREADLINK, self.tag);
        writer.u32(fid);
        let end = self.rpc(writer.finish(), msg::RREADLINK, self.tag)?;
        WireReader::new(&self.response[HEADER_LEN..end]).string()
    }

    pub fn link(&mut self, dfid: u32, fid: u32, name: &str) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TLINK, self.tag);
        writer.u32(dfid).u32(fid).string(name);
        self.rpc(writer.finish(), msg::RLINK, self.tag).map(|_| ())
    }

    pub fn fsync(&mut self, fid: u32, data_only: bool) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TFSYNC, self.tag);
        writer.u32(fid).u32(u32::from(data_only));
        self.rpc(writer.finish(), msg::RFSYNC, self.tag).map(|_| ())
    }

    pub fn statfs(&mut self, fid: u32) -> Result<Statfs, FilesystemError> {
        let mut writer = WireWriter::request(msg::TSTATFS, self.tag);
        writer.u32(fid);
        let end = self.rpc(writer.finish(), msg::RSTATFS, self.tag)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let _fs_type = reader.u32()?;
        let bsize = reader.u32()?;
        let blocks = reader.u64()?;
        let bfree = reader.u64()?;
        let _bavail = reader.u64()?;
        let files = reader.u64()?;
        let ffree = reader.u64()?;
        Ok(Statfs {
            bsize,
            blocks,
            bfree,
            files,
            ffree,
        })
    }
}
