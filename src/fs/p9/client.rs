//! Serialized 9P2000.L client over the virtio-9p transport.
//!
//! One request is in flight at a time (single fixed tag); callers serialize
//! behind the `P9Filesystem` lock. Fids are allocated from a free list; fid 0
//! is the attach root and lives for the mount's lifetime. Transport failures
//! quarantine the channel and every subsequent operation fails with
//! `IoError` rather than wedging or retrying from a desynced stream.

use crate::drivers::virtio::p9::P9Transport;
use crate::fs::filesystem::FilesystemError;
use crate::fs::p9::protocol::{
    map_errno, msg, P9Dirent, P9Stat, Qid, WireReader, WireWriter, GETATTR_BASIC, MAX_WELEM, NOFID,
    NOTAG, TAG,
};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const ROOT_FID: u32 = 0;

/// msize requested from the server; the negotiated value may be lower.
const REQUEST_MSIZE: u32 = 64 * 1024;

/// Wire overhead reserved when sizing read/write/readdir payloads:
/// the largest applicable header is Twrite's 23 bytes, rounded up.
const IO_HEADER_RESERVE: u32 = 32;

/// 9P message header: size[4] type[1] tag[2].
const HEADER_LEN: usize = 7;

/// Bound on symlink-target resolution recursion in the backend.
pub const MAX_SYMLINK_DEPTH: u8 = 8;

pub struct P9Client {
    transport: P9Transport,
    msize: u32,
    response: Vec<u8>,
    next_fid: u32,
    free_fids: Vec<u32>,
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
            next_fid: ROOT_FID + 1,
            free_fids: Vec::new(),
            poisoned: false,
        }
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

        let mut writer = WireWriter::request(msg::TATTACH, TAG);
        writer
            .u32(ROOT_FID)
            .u32(NOFID)
            .string("root")
            .string("")
            .u32(0);
        let end = self.rpc(writer.finish(), msg::RATTACH, TAG)?;
        WireReader::new(&self.response[HEADER_LEN..end]).qid()?;
        Ok(())
    }

    /// Largest read/write/readdir payload that fits the negotiated msize.
    pub fn io_unit(&self) -> usize {
        self.msize.saturating_sub(IO_HEADER_RESERVE) as usize
    }

    fn alloc_fid(&mut self) -> u32 {
        self.free_fids.pop().unwrap_or_else(|| {
            let fid = self.next_fid;
            self.next_fid = self.next_fid.wrapping_add(1).max(ROOT_FID + 1);
            fid
        })
    }

    fn release_fid(&mut self, fid: u32) {
        if fid != ROOT_FID {
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
            let mut writer = WireWriter::request(msg::TWALK, TAG);
            writer.u32(source).u32(newfid).u16(chunk.len() as u16);
            for name in chunk {
                writer.string(name);
            }
            let end = match self.rpc(writer.finish(), msg::RWALK, TAG) {
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
        self.walk(ROOT_FID, &names)
    }

    pub fn clunk(&mut self, fid: u32) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TCLUNK, TAG);
        writer.u32(fid);
        let result = self.rpc(writer.finish(), msg::RCLUNK, TAG).map(|_| ());
        // The fid number is retired locally even if the server errored; a
        // stale server-side fid surfaces as a later walk error, never as
        // silent reuse of live state.
        self.release_fid(fid);
        result
    }

    pub fn getattr(&mut self, fid: u32) -> Result<P9Stat, FilesystemError> {
        let mut writer = WireWriter::request(msg::TGETATTR, TAG);
        writer.u32(fid).u64(GETATTR_BASIC);
        let end = self.rpc(writer.finish(), msg::RGETATTR, TAG)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let _valid = reader.u64()?;
        reader.rgetattr_body()
    }

    /// Tsetattr limited to the fields the kernel mutates: size, atime, mtime.
    pub fn setattr(
        &mut self,
        fid: u32,
        valid: u32,
        size: u64,
        atime_sec: u64,
        mtime_sec: u64,
    ) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TSETATTR, TAG);
        writer
            .u32(fid)
            .u32(valid)
            .u32(0) // mode
            .u32(0) // uid
            .u32(0) // gid
            .u64(size)
            .u64(atime_sec)
            .u64(0) // atime_nsec
            .u64(mtime_sec)
            .u64(0); // mtime_nsec
        self.rpc(writer.finish(), msg::RSETATTR, TAG).map(|_| ())
    }

    pub fn lopen(&mut self, fid: u32, flags: u32) -> Result<Qid, FilesystemError> {
        let mut writer = WireWriter::request(msg::TLOPEN, TAG);
        writer.u32(fid).u32(flags);
        let end = self.rpc(writer.finish(), msg::RLOPEN, TAG)?;
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
        let mut writer = WireWriter::request(msg::TLCREATE, TAG);
        writer.u32(dfid).string(name).u32(flags).u32(mode).u32(0);
        let end = self.rpc(writer.finish(), msg::RLCREATE, TAG)?;
        WireReader::new(&self.response[HEADER_LEN..end]).qid()
    }

    pub fn read(
        &mut self,
        fid: u32,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, FilesystemError> {
        let count = out.len().min(self.io_unit()) as u32;
        let mut writer = WireWriter::request(msg::TREAD, TAG);
        writer.u32(fid).u64(offset).u32(count);
        let end = self.rpc(writer.finish(), msg::RREAD, TAG)?;
        let mut reader = WireReader::new(&self.response[HEADER_LEN..end]);
        let returned = reader.u32()? as usize;
        let data = reader.take(returned)?;
        if returned > out.len() {
            self.poison();
            return Err(FilesystemError::IoError);
        }
        out[..returned].copy_from_slice(data);
        Ok(returned)
    }

    pub fn write(&mut self, fid: u32, offset: u64, data: &[u8]) -> Result<usize, FilesystemError> {
        let count = data.len().min(self.io_unit());
        let mut writer = WireWriter::request(msg::TWRITE, TAG);
        writer.u32(fid).u64(offset).u32(count as u32);
        writer.bytes(&data[..count]);
        let end = self.rpc(writer.finish(), msg::RWRITE, TAG)?;
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
        let mut writer = WireWriter::request(msg::TREADDIR, TAG);
        writer.u32(fid).u64(offset).u32(count);
        let end = self.rpc(writer.finish(), msg::RREADDIR, TAG)?;
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
        let mut writer = WireWriter::request(msg::TMKDIR, TAG);
        writer.u32(dfid).string(name).u32(mode).u32(0);
        self.rpc(writer.finish(), msg::RMKDIR, TAG).map(|_| ())
    }

    pub fn unlinkat(&mut self, dfid: u32, name: &str, flags: u32) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TUNLINKAT, TAG);
        writer.u32(dfid).string(name).u32(flags);
        self.rpc(writer.finish(), msg::RUNLINKAT, TAG).map(|_| ())
    }

    pub fn renameat(
        &mut self,
        old_dfid: u32,
        old_name: &str,
        new_dfid: u32,
        new_name: &str,
    ) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TRENAMEAT, TAG);
        writer
            .u32(old_dfid)
            .string(old_name)
            .u32(new_dfid)
            .string(new_name);
        self.rpc(writer.finish(), msg::RRENAMEAT, TAG).map(|_| ())
    }

    pub fn symlink(&mut self, dfid: u32, name: &str, target: &str) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TSYMLINK, TAG);
        writer.u32(dfid).string(name).string(target).u32(0);
        self.rpc(writer.finish(), msg::RSYMLINK, TAG).map(|_| ())
    }

    pub fn readlink(&mut self, fid: u32) -> Result<String, FilesystemError> {
        let mut writer = WireWriter::request(msg::TREADLINK, TAG);
        writer.u32(fid);
        let end = self.rpc(writer.finish(), msg::RREADLINK, TAG)?;
        WireReader::new(&self.response[HEADER_LEN..end]).string()
    }

    pub fn link(&mut self, dfid: u32, fid: u32, name: &str) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TLINK, TAG);
        writer.u32(dfid).u32(fid).string(name);
        self.rpc(writer.finish(), msg::RLINK, TAG).map(|_| ())
    }

    pub fn fsync(&mut self, fid: u32, data_only: bool) -> Result<(), FilesystemError> {
        let mut writer = WireWriter::request(msg::TFSYNC, TAG);
        writer.u32(fid).u32(u32::from(data_only));
        self.rpc(writer.finish(), msg::RFSYNC, TAG).map(|_| ())
    }

    pub fn statfs(&mut self, fid: u32) -> Result<Statfs, FilesystemError> {
        let mut writer = WireWriter::request(msg::TSTATFS, TAG);
        writer.u32(fid);
        let end = self.rpc(writer.finish(), msg::RSTATFS, TAG)?;
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
