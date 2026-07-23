//! `Filesystem` backend over the 9P2000.L client — the `/shared` mount.
//!
//! Four independently tagged client lanes overlap requests from Git's index
//! workers. Walked path fids are retained, but attributes are never cached:
//! every stat still sends Tgetattr to observe host-side changes. Open
//! sequential reads use one bounded read-ahead window per lane.
//! `FileHandle::inode` carries both the lane and open fid. Symlinks are
//! resolved client-side (bounded depth) because the QEMU server hands back
//! the symlink node itself on walk.

use crate::drivers::virtio::p9::P9Transport;
use crate::fs::filesystem::{
    DirectoryEntry, DirectoryIterator, FileAttributes, FileHandle, FileMode, FileType, Filesystem,
    FilesystemError, FilesystemStats, UnixMetadata, UnixTimestamp,
};
use crate::fs::p9::client::{P9Client, MAX_SYMLINK_DEPTH};
use crate::fs::p9::protocol::{open_flags, setattr_valid, P9Stat, AT_REMOVEDIR};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::arch::x86_64::interrupt_guard::InterruptMutex;

/// Mode bits for files/directories the guest creates. The host-side QEMU
/// process applies its umask; guest chmod is already a validated no-op.
const CREATE_FILE_MODE: u32 = 0o644;
const CREATE_DIR_MODE: u32 = 0o755;

/// Linux `d_type` values surfaced in Rreaddir entries.
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;

/// Match the default four-vCPU guest and Git's parallel index preload while
/// staying comfortably below the virtqueue descriptor budget for 64 KiB I/O.
const CLIENT_LANES: usize = 4;

#[derive(Clone, Copy)]
enum ClientWaiter {
    Kernel(crate::process::ProcessId),
    RingThree(u32),
}

#[derive(Clone, Copy)]
struct QueuedClientWaiter {
    token: u64,
    waiter: ClientWaiter,
}

struct ClientGate {
    locked: bool,
    waiters: VecDeque<QueuedClientWaiter>,
    granted: Option<u64>,
}

/// Scheduler-aware mutex for one tagged 9p client lane.
///
/// A normal spin mutex cannot be held across an interrupt-driven RPC: once
/// the owner sleeps, another Git worker on the same CPU can spin in ring 0 and
/// prevent the owner from ever resuming. Contenders queue here and park on an
/// exact token; unlock transfers ownership FIFO before waking one waiter.
struct P9ClientLock {
    gate: InterruptMutex<ClientGate>,
    client: UnsafeCell<P9Client>,
}

// SAFETY: `gate.locked` grants unique mutable access to `client`; every guard
// releases or transfers that ownership in Drop.
unsafe impl Sync for P9ClientLock {}

struct P9ClientGuard<'a> {
    lock: &'a P9ClientLock,
}

static NEXT_CLIENT_WAIT_TOKEN: AtomicU64 = AtomicU64::new(3 << 62);
const P9_CLIENT_LOCK_DIAGNOSTIC_DEVICE: usize = u16::MAX as usize - 0x100;

impl P9ClientLock {
    fn new(client: P9Client) -> Self {
        Self {
            gate: InterruptMutex::new(ClientGate {
                locked: false,
                waiters: VecDeque::new(),
                granted: None,
            }),
            client: UnsafeCell::new(client),
        }
    }

    fn lock(&self) -> P9ClientGuard<'_> {
        loop {
            let waiter = {
                let mut gate = self.gate.lock();
                if !gate.locked {
                    assert!(gate.granted.is_none(), "9p client has an orphaned handoff");
                    gate.locked = true;
                    return P9ClientGuard { lock: self };
                }

                let waiter = if let Some(pid) = crate::userland::lifecycle::current_user_pid() {
                    Some(ClientWaiter::RingThree(pid))
                } else {
                    crate::process::current_io_waiter().map(ClientWaiter::Kernel)
                };
                let Some(waiter) = waiter else {
                    drop(gate);
                    let interrupts_enabled = x86_64::instructions::interrupts::are_enabled();
                    x86_64::instructions::interrupts::enable_and_hlt();
                    if !interrupts_enabled {
                        x86_64::instructions::interrupts::disable();
                    }
                    continue;
                };

                let token = NEXT_CLIENT_WAIT_TOKEN.fetch_add(1, Ordering::Relaxed);
                gate.waiters.push_back(QueuedClientWaiter { token, waiter });
                if let ClientWaiter::RingThree(pid) = waiter {
                    crate::diagnostics::shadow::io::submitted(
                        token,
                        crate::diagnostics::shadow::pager::current_generation(),
                        pid,
                        P9_CLIENT_LOCK_DIAGNOSTIC_DEVICE,
                        0,
                        0,
                    );
                }
                QueuedClientWaiter { token, waiter }
            };

            match waiter.waiter {
                ClientWaiter::RingThree(_) => {
                    crate::userland::switch::block_current_ring3_on_io(waiter.token);
                    let granted = self.take_grant(waiter.token);
                    assert!(granted, "9p client woke without lock handoff");
                    crate::diagnostics::shadow::io::consumed(waiter.token);
                }
                ClientWaiter::Kernel(_) => {
                    let mut consumed_grant = false;
                    let parked = crate::process::park_current_if(
                        crate::process::pcb::BlockReason::WaitingForBlockIo(waiter.token),
                        || {
                            consumed_grant = self.take_grant(waiter.token);
                            !consumed_grant
                        },
                    );
                    if parked {
                        consumed_grant = self.take_grant(waiter.token);
                    }
                    assert!(consumed_grant, "9p client woke without lock handoff");
                }
            }
            return P9ClientGuard { lock: self };
        }
    }

    fn take_grant(&self, token: u64) -> bool {
        let mut gate = self.gate.lock();
        if gate.granted == Some(token) {
            gate.granted = None;
            true
        } else {
            false
        }
    }

    fn unlock(&self) {
        let next = {
            let mut gate = self.gate.lock();
            assert!(gate.locked, "unbalanced 9p client unlock");
            assert!(gate.granted.is_none(), "9p client handoff not consumed");
            match gate.waiters.pop_front() {
                Some(waiter) => {
                    gate.granted = Some(waiter.token);
                    Some(waiter)
                }
                None => {
                    gate.locked = false;
                    None
                }
            }
        };

        let Some(next) = next else { return };
        match next.waiter {
            ClientWaiter::RingThree(pid) => {
                crate::diagnostics::shadow::io::completed(next.token, 0, 0);
                crate::userland::lifecycle::queue_ring3_io_wake(pid, next.token);
            }
            ClientWaiter::Kernel(pid) => {
                let entity = crate::process::entity::EntityId::KernelThread(pid);
                let cpu = {
                    let mut scheduler = crate::process::scheduler::SCHEDULER.lock();
                    scheduler.wake(pid);
                    scheduler.cpu_affinity(entity)
                };
                if let Some(cpu) = cpu {
                    crate::arch::x86_64::smp::notify_cpu(cpu);
                }
            }
        }
    }
}

impl core::ops::Deref for P9ClientGuard<'_> {
    type Target = P9Client;

    fn deref(&self) -> &Self::Target {
        // SAFETY: this guard owns the client gate until Drop.
        unsafe { &*self.lock.client.get() }
    }
}

impl core::ops::DerefMut for P9ClientGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: the gate admits only one guard at a time.
        unsafe { &mut *self.lock.client.get() }
    }
}

impl Drop for P9ClientGuard<'_> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

pub struct P9Filesystem {
    clients: Vec<P9ClientLock>,
    next_client: AtomicUsize,
}

impl P9Filesystem {
    /// Run the version/attach handshake and wrap the client for mounting.
    pub fn new(transport: P9Transport) -> Result<Self, FilesystemError> {
        let mut client = P9Client::new(transport.clone());
        client.handshake()?;
        let msize = client.negotiated_msize();
        let mut clients = Vec::with_capacity(CLIENT_LANES);
        clients.push(P9ClientLock::new(client));
        for lane in 1..CLIENT_LANES {
            clients.push(P9ClientLock::new(P9Client::attach_lane(
                transport.clone(),
                msize,
                lane as u16,
            )?));
        }
        Ok(Self {
            clients,
            next_client: AtomicUsize::new(0),
        })
    }

    fn lock_any(&self) -> (usize, P9ClientGuard<'_>) {
        let lane = self.next_client.fetch_add(1, Ordering::Relaxed) % self.clients.len();
        (lane, self.clients[lane].lock())
    }

    fn lock_path(&self, path: &str) -> P9ClientGuard<'_> {
        // Stable FNV-1a assignment keeps one reusable fid per path while
        // distributing Git's independent index entries across all lanes.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in path.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.clients[hash as usize % self.clients.len()].lock()
    }

    fn invalidate_path_caches(&self, path: &str) {
        for client in &self.clients {
            client.lock().invalidate_path(path);
        }
    }

    fn lock_handle(&self, handle: &FileHandle) -> Result<P9ClientGuard<'_>, FilesystemError> {
        let lane = (handle.inode >> 32) as usize;
        self.clients
            .get(lane)
            .map(P9ClientLock::lock)
            .ok_or(FilesystemError::InvalidPath)
    }
}

fn encode_handle_fid(lane: usize, fid: u32) -> u64 {
    ((lane as u64) << 32) | u64::from(fid)
}

fn decode_handle_fid(handle: &FileHandle) -> u32 {
    handle.inode as u32
}

fn stat_path_resolved(client: &mut P9Client, path: &str) -> Result<P9Stat, FilesystemError> {
    let stat = client.getattr_path_cached(path)?;
    if !stat.qid.is_symlink() {
        return Ok(stat);
    }
    let (fid, stat) = walk_resolved(client, path, MAX_SYMLINK_DEPTH)?;
    clunk_quiet(client, fid);
    Ok(stat)
}

/// Split a mount-relative path ("a/b/c", "/a", "/") into parent and leaf.
fn split_parent_leaf(path: &str) -> Result<(&str, &str), FilesystemError> {
    let trimmed = path.trim_end_matches('/').trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(FilesystemError::InvalidPath);
    }
    let (parent, leaf) = match trimmed.rfind('/') {
        Some(index) => (&trimmed[..index], &trimmed[index + 1..]),
        None => ("", trimmed),
    };
    if leaf.is_empty() || leaf == "." || leaf == ".." {
        return Err(FilesystemError::InvalidPath);
    }
    Ok((parent, leaf))
}

/// The display name for a path: its last real component, or "/" for root.
fn leaf_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|component| !component.is_empty())
        .unwrap_or("/")
}

fn file_type_from_mode(mode: u32) -> FileType {
    match mode & 0o170000 {
        0o040000 => FileType::Directory,
        0o120000 => FileType::Symlink,
        0o100000 => FileType::File,
        _ => FileType::Other,
    }
}

fn file_type_from_dtype(type_byte: u8) -> FileType {
    match type_byte {
        DT_DIR => FileType::Directory,
        DT_REG => FileType::File,
        DT_LNK => FileType::Symlink,
        _ => FileType::Other,
    }
}

fn entry_from_parts(name: &str, file_type: FileType, stat: Option<&P9Stat>) -> DirectoryEntry {
    let mut name_buf = [0u8; 256];
    let bytes = name.as_bytes();
    let len = bytes.len().min(name_buf.len());
    name_buf[..len].copy_from_slice(&bytes[..len]);
    DirectoryEntry {
        name: name_buf,
        name_len: len,
        file_type,
        size: stat.map(|s| s.size).unwrap_or(0),
        attributes: FileAttributes {
            read_only: false,
            hidden: false,
            system: false,
            archive: false,
        },
        created: stat.map(|s| s.ctime_sec).unwrap_or(0),
        modified: stat.map(|s| s.mtime_sec).unwrap_or(0),
        accessed: stat.map(|s| s.atime_sec).unwrap_or(0),
    }
}

fn metadata_from_stat(stat: &P9Stat) -> UnixMetadata {
    UnixMetadata {
        inode: stat.qid.path,
        mode: stat.mode,
        uid: stat.uid,
        gid: stat.gid,
        links: stat.nlink,
        size: stat.size,
        blocks_512: stat.blocks,
        block_size: (stat.blksize as u32).max(512),
        accessed: UnixTimestamp {
            seconds: stat.atime_sec,
            nanoseconds: stat.atime_nsec,
        },
        modified: UnixTimestamp {
            seconds: stat.mtime_sec,
            nanoseconds: stat.mtime_nsec,
        },
        changed: UnixTimestamp {
            seconds: stat.ctime_sec,
            nanoseconds: stat.ctime_nsec,
        },
    }
}

fn clunk_quiet(client: &mut P9Client, fid: u32) {
    let _ = client.clunk(fid);
}

/// Walk to `path` without following a leaf symlink (lstat semantics).
fn walk_nofollow(client: &mut P9Client, path: &str) -> Result<(u32, P9Stat), FilesystemError> {
    let fid = client.walk_path(path)?;
    match client.getattr(fid) {
        Ok(stat) => Ok((fid, stat)),
        Err(error) => {
            clunk_quiet(client, fid);
            Err(error)
        }
    }
}

/// Walk to `path`, resolving leaf symlinks client-side. Absolute targets are
/// interpreted relative to the share root; resolution depth is bounded.
fn walk_resolved(
    client: &mut P9Client,
    path: &str,
    depth: u8,
) -> Result<(u32, P9Stat), FilesystemError> {
    let (fid, stat) = walk_nofollow(client, path)?;
    if !stat.qid.is_symlink() {
        return Ok((fid, stat));
    }
    if depth == 0 {
        clunk_quiet(client, fid);
        return Err(FilesystemError::IoError); // ELOOP-equivalent
    }
    let target = match client.readlink(fid) {
        Ok(target) => target,
        Err(error) => {
            clunk_quiet(client, fid);
            return Err(error);
        }
    };
    clunk_quiet(client, fid);
    let resolved: String = if target.starts_with('/') {
        target
    } else {
        match path.trim_end_matches('/').rfind('/') {
            Some(index) => format!("{}/{}", &path[..index], target),
            None => target,
        }
    };
    walk_resolved(client, &resolved, depth - 1)
}

fn open_existing(
    client: &mut P9Client,
    path: &str,
    mode: FileMode,
) -> Result<FileHandle, FilesystemError> {
    let (fid, stat) = walk_resolved(client, path, MAX_SYMLINK_DEPTH)?;
    let mut flags = if mode.write {
        open_flags::O_RDWR
    } else {
        open_flags::O_RDONLY
    };
    if mode.write && mode.truncate {
        flags |= open_flags::O_TRUNC;
    }
    if let Err(error) = client.lopen(fid, flags) {
        clunk_quiet(client, fid);
        return Err(error);
    }
    let size = if mode.write && mode.truncate {
        0
    } else {
        stat.size
    };
    Ok(FileHandle {
        inode: fid as u64,
        position: if mode.append { size } else { 0 },
        size,
        mode,
    })
}

fn create_new(
    client: &mut P9Client,
    path: &str,
    mode: FileMode,
) -> Result<FileHandle, FilesystemError> {
    let (parent, leaf) = split_parent_leaf(path)?;
    let dfid = client.walk_path(parent)?;
    let flags = if mode.write {
        open_flags::O_RDWR
    } else {
        open_flags::O_RDONLY
    };
    // On success the directory fid becomes the open fid of the new file.
    if let Err(error) = client.lcreate(dfid, leaf, flags, CREATE_FILE_MODE) {
        clunk_quiet(client, dfid);
        return Err(error);
    }
    Ok(FileHandle {
        inode: dfid as u64,
        position: 0,
        size: 0,
        mode,
    })
}

impl Filesystem for P9Filesystem {
    fn name(&self) -> &str {
        "9p"
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn stats(&self) -> Result<FilesystemStats, FilesystemError> {
        let (_, mut client) = self.lock_any();
        let root_fid = client.root_fid();
        let statfs = client.statfs(root_fid)?;
        Ok(FilesystemStats {
            total_blocks: statfs.blocks,
            free_blocks: statfs.bfree,
            block_size: statfs.bsize.max(512),
            total_inodes: statfs.files,
            free_inodes: statfs.ffree,
        })
    }

    fn read_dir(&self, _path: &str) -> Result<DirectoryIterator<'_>, FilesystemError> {
        // Callers use enumerate_dir, same as ext2.
        Err(FilesystemError::UnsupportedOperation)
    }

    fn enumerate_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (dirfid, stat) = walk_resolved(&mut client, path, MAX_SYMLINK_DEPTH)?;
        if !stat.qid.is_dir() {
            clunk_quiet(&mut client, dirfid);
            return Err(FilesystemError::NotADirectory);
        }
        // An opened fid cannot be walked, so readdir runs on a clone while
        // `dirfid` stays walkable for the per-entry getattr pass.
        let read_fid = match client.walk(dirfid, &[]) {
            Ok(fid) => fid,
            Err(error) => {
                clunk_quiet(&mut client, dirfid);
                return Err(error);
            }
        };
        let mut dirents = Vec::new();
        let listing: Result<(), FilesystemError> = (|| {
            client.lopen(read_fid, open_flags::O_RDONLY | open_flags::O_DIRECTORY)?;
            let mut offset = 0u64;
            loop {
                let batch = client.readdir(read_fid, offset)?;
                let Some(last) = batch.last() else {
                    return Ok(());
                };
                offset = last.offset;
                dirents.extend(
                    batch
                        .into_iter()
                        .filter(|entry| entry.name != "." && entry.name != ".."),
                );
            }
        })();
        clunk_quiet(&mut client, read_fid);
        if let Err(error) = listing {
            clunk_quiet(&mut client, dirfid);
            return Err(error);
        }

        let mut entries = Vec::with_capacity(dirents.len());
        for dirent in dirents {
            // Sizes/timestamps need a getattr; a vanished entry (concurrent
            // unlink from another instance) degrades to the dirent type.
            let stat = match client.walk(dirfid, &[&dirent.name]) {
                Ok(fid) => {
                    let stat = client.getattr(fid).ok();
                    clunk_quiet(&mut client, fid);
                    stat
                }
                Err(_) => None,
            };
            let file_type = match &stat {
                Some(stat) => file_type_from_mode(stat.mode),
                None => file_type_from_dtype(dirent.type_byte),
            };
            entries.push(entry_from_parts(&dirent.name, file_type, stat.as_ref()));
        }
        clunk_quiet(&mut client, dirfid);
        Ok(entries)
    }

    fn enumerate_dir_names(&self, path: &str) -> Result<Vec<DirectoryEntry>, FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (dirfid, stat) = walk_resolved(&mut client, path, MAX_SYMLINK_DEPTH)?;
        if !stat.qid.is_dir() {
            clunk_quiet(&mut client, dirfid);
            return Err(FilesystemError::NotADirectory);
        }
        let read_fid = match client.walk(dirfid, &[]) {
            Ok(fid) => fid,
            Err(error) => {
                clunk_quiet(&mut client, dirfid);
                return Err(error);
            }
        };
        let listing: Result<Vec<DirectoryEntry>, FilesystemError> = (|| {
            client.lopen(read_fid, open_flags::O_RDONLY | open_flags::O_DIRECTORY)?;
            let mut entries = Vec::new();
            let mut offset = 0u64;
            loop {
                let batch = client.readdir(read_fid, offset)?;
                let Some(last) = batch.last() else {
                    return Ok(entries);
                };
                offset = last.offset;
                entries.extend(
                    batch
                        .into_iter()
                        .filter(|entry| entry.name != "." && entry.name != "..")
                        .map(|entry| {
                            entry_from_parts(
                                &entry.name,
                                file_type_from_dtype(entry.type_byte),
                                None,
                            )
                        }),
                );
            }
        })();
        clunk_quiet(&mut client, read_fid);
        clunk_quiet(&mut client, dirfid);
        listing
    }

    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        let mut client = self.lock_path(path);
        let stat = stat_path_resolved(&mut client, path)?;
        Ok(entry_from_parts(
            leaf_name(path),
            file_type_from_mode(stat.mode),
            Some(&stat),
        ))
    }

    fn unix_metadata(&self, path: &str) -> Result<UnixMetadata, FilesystemError> {
        let mut client = self.lock_path(path);
        let stat = stat_path_resolved(&mut client, path)?;
        Ok(metadata_from_stat(&stat))
    }

    fn symlink_metadata(&self, path: &str) -> Result<UnixMetadata, FilesystemError> {
        let mut client = self.lock_path(path);
        let stat = client.getattr_path_cached(path)?;
        Ok(metadata_from_stat(&stat))
    }

    fn handle_metadata(&self, handle: &FileHandle) -> Result<UnixMetadata, FilesystemError> {
        let mut client = self.lock_handle(handle)?;
        let stat = client.getattr(decode_handle_fid(handle))?;
        Ok(metadata_from_stat(&stat))
    }

    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError> {
        let (lane, mut client) = self.lock_any();
        let result = match open_existing(&mut client, path, mode) {
            Err(FilesystemError::NotFound) if mode.create => {
                match create_new(&mut client, path, mode) {
                    // Lost a create race against another instance: the file
                    // exists now, so open it.
                    Err(FilesystemError::AlreadyExists) => open_existing(&mut client, path, mode),
                    other => other,
                }
            }
            other => other,
        };
        drop(client);
        if mode.create && result.is_ok() {
            self.invalidate_path_caches(path);
        }
        result.map(|mut handle| {
            handle.inode = encode_handle_fid(lane, handle.inode as u32);
            handle
        })
    }

    fn close(&self, handle: &mut FileHandle) -> Result<(), FilesystemError> {
        let mut client = self.lock_handle(handle)?;
        client.clunk(decode_handle_fid(handle))
    }

    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError> {
        if !handle.mode.read {
            return Err(FilesystemError::PermissionDenied);
        }
        let mut client = self.lock_handle(handle)?;
        let fid = decode_handle_fid(handle);
        let mut done = 0usize;
        while done < buffer.len() {
            let read = client.read(fid, handle.position, &mut buffer[done..])?;
            if read == 0 {
                break;
            }
            done += read;
            handle.position += read as u64;
            handle.size = handle.size.max(handle.position);
        }
        Ok(done)
    }

    fn write(&self, handle: &mut FileHandle, buffer: &[u8]) -> Result<usize, FilesystemError> {
        if !handle.mode.write {
            return Err(FilesystemError::PermissionDenied);
        }
        let mut client = self.lock_handle(handle)?;
        let fid = decode_handle_fid(handle);
        // Append re-reads the live size so concurrent appenders from other
        // instances interleave instead of overwriting.
        let position = if handle.mode.append {
            client.getattr(fid)?.size
        } else {
            handle.position
        };
        let mut done = 0usize;
        while done < buffer.len() {
            let written = client.write(fid, position + done as u64, &buffer[done..])?;
            if written == 0 {
                return Err(FilesystemError::IoError);
            }
            done += written;
        }
        handle.position = position + done as u64;
        handle.size = handle.size.max(handle.position);
        Ok(done)
    }

    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        // Positions past EOF are legal on writable mounts; the host handles
        // the sparse-gap semantics on the next write.
        handle.position = position;
        Ok(position)
    }

    fn truncate(&self, handle: &mut FileHandle, size: u64) -> Result<(), FilesystemError> {
        if !handle.mode.write {
            return Err(FilesystemError::PermissionDenied);
        }
        let mut client = self.lock_handle(handle)?;
        client.setattr(
            decode_handle_fid(handle),
            setattr_valid::SIZE,
            size,
            UnixTimestamp::ZERO,
            UnixTimestamp::ZERO,
        )?;
        handle.size = size;
        Ok(())
    }

    fn set_times(
        &self,
        path: &str,
        accessed: Option<UnixTimestamp>,
        modified: Option<UnixTimestamp>,
    ) -> Result<(), FilesystemError> {
        if accessed.is_none() && modified.is_none() {
            return Ok(());
        }
        let (_, mut client) = self.lock_any();
        let (fid, _stat) = walk_resolved(&mut client, path, MAX_SYMLINK_DEPTH)?;
        let mut valid = 0u32;
        if accessed.is_some() {
            valid |= setattr_valid::ATIME | setattr_valid::ATIME_SET;
        }
        if modified.is_some() {
            valid |= setattr_valid::MTIME | setattr_valid::MTIME_SET;
        }
        let result = client.setattr(
            fid,
            valid,
            0,
            accessed.unwrap_or(UnixTimestamp::ZERO),
            modified.unwrap_or(UnixTimestamp::ZERO),
        );
        clunk_quiet(&mut client, fid);
        result
    }

    fn sync_handle(&self, handle: &FileHandle, data_only: bool) -> Result<(), FilesystemError> {
        let mut client = self.lock_handle(handle)?;
        client.fsync(decode_handle_fid(handle), data_only)
    }

    fn link(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
        let (_, mut client) = self.lock_any();
        let fid = client.walk_path(old_path)?;
        let (parent, leaf) = match split_parent_leaf(new_path) {
            Ok(parts) => parts,
            Err(error) => {
                clunk_quiet(&mut client, fid);
                return Err(error);
            }
        };
        let dfid = match client.walk_path(parent) {
            Ok(dfid) => dfid,
            Err(error) => {
                clunk_quiet(&mut client, fid);
                return Err(error);
            }
        };
        let result = client.link(dfid, fid, leaf);
        clunk_quiet(&mut client, dfid);
        clunk_quiet(&mut client, fid);
        drop(client);
        if result.is_ok() {
            self.invalidate_path_caches(new_path);
        }
        result
    }

    fn symlink(&self, target: &str, link_path: &str) -> Result<(), FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (parent, leaf) = split_parent_leaf(link_path)?;
        let dfid = client.walk_path(parent)?;
        let result = client.symlink(dfid, leaf, target);
        clunk_quiet(&mut client, dfid);
        drop(client);
        if result.is_ok() {
            self.invalidate_path_caches(link_path);
        }
        result
    }

    fn read_link(&self, path: &str) -> Result<Vec<u8>, FilesystemError> {
        let (_, mut client) = self.lock_any();
        let fid = client.walk_path(path)?;
        let result = client.readlink(fid);
        clunk_quiet(&mut client, fid);
        result.map(String::into_bytes)
    }

    fn mkdir(&self, path: &str) -> Result<(), FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (parent, leaf) = split_parent_leaf(path)?;
        let dfid = client.walk_path(parent)?;
        let result = client.mkdir(dfid, leaf, CREATE_DIR_MODE);
        clunk_quiet(&mut client, dfid);
        drop(client);
        if result.is_ok() {
            self.invalidate_path_caches(path);
        }
        result
    }

    fn unlink(&self, path: &str) -> Result<(), FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (parent, leaf) = split_parent_leaf(path)?;
        let dfid = client.walk_path(parent)?;
        let result = client.unlinkat(dfid, leaf, 0);
        clunk_quiet(&mut client, dfid);
        drop(client);
        if result.is_ok() {
            self.invalidate_path_caches(path);
        }
        result
    }

    fn rmdir(&self, path: &str) -> Result<(), FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (parent, leaf) = split_parent_leaf(path)?;
        let dfid = client.walk_path(parent)?;
        let result = client.unlinkat(dfid, leaf, AT_REMOVEDIR);
        clunk_quiet(&mut client, dfid);
        drop(client);
        if result.is_ok() {
            self.invalidate_path_caches(path);
        }
        result
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
        let (_, mut client) = self.lock_any();
        let (old_parent, old_leaf) = split_parent_leaf(old_path)?;
        let (new_parent, new_leaf) = split_parent_leaf(new_path)?;
        let old_dfid = client.walk_path(old_parent)?;
        let new_dfid = match client.walk_path(new_parent) {
            Ok(dfid) => dfid,
            Err(error) => {
                clunk_quiet(&mut client, old_dfid);
                return Err(error);
            }
        };
        let result = client.renameat(old_dfid, old_leaf, new_dfid, new_leaf);
        clunk_quiet(&mut client, new_dfid);
        clunk_quiet(&mut client, old_dfid);
        drop(client);
        if result.is_ok() {
            self.invalidate_path_caches(old_path);
            self.invalidate_path_caches(new_path);
        }
        result
    }

    fn sync(&self) -> Result<(), FilesystemError> {
        // Every write already reached the host kernel; there is no guest
        // dirty state to flush.
        Ok(())
    }
}
