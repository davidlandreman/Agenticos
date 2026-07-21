//! `Filesystem` trait implementation for tmpfs.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::fs::filesystem::{
    DirectoryEntry, DirectoryIterator, FileAttributes, FileHandle, FileMode, FileType, Filesystem,
    FilesystemError, FilesystemStats, UnixMetadata, UnixTimestamp,
};
use crate::lib::arc::Arc;

/// In-memory file body.
pub type FileBody = Arc<Mutex<TmpFile>>;

/// In-memory directory body. Children are name → node.
pub type DirBody = Arc<Mutex<TmpDirectory>>;

#[derive(Debug, Clone, Copy)]
pub struct NodeTimes {
    pub accessed: UnixTimestamp,
    pub modified: UnixTimestamp,
    pub changed: UnixTimestamp,
}

impl NodeTimes {
    fn now() -> Self {
        let now = current_time();
        Self {
            accessed: now,
            modified: now,
            changed: now,
        }
    }

    fn touch_content(&mut self, now: UnixTimestamp) {
        self.modified = now;
        self.changed = now;
    }

    fn touch_namespace(&mut self, now: UnixTimestamp) {
        self.modified = now;
        self.changed = now;
    }
}

pub struct TmpFile {
    pub(crate) data: Vec<u8>,
    pub(crate) times: NodeTimes,
}

pub struct TmpDirectory {
    pub(crate) children: BTreeMap<String, TmpNode>,
    pub(crate) times: NodeTimes,
}

fn current_time() -> UnixTimestamp {
    UnixTimestamp::from_nanoseconds(crate::time::realtime_ns())
}

fn new_file_body() -> FileBody {
    Arc::new(Mutex::new(TmpFile {
        data: Vec::new(),
        times: NodeTimes::now(),
    }))
}

fn new_dir_body() -> DirBody {
    Arc::new(Mutex::new(TmpDirectory {
        children: BTreeMap::new(),
        times: NodeTimes::now(),
    }))
}

/// Either a regular file or a directory. Nodes are reference-counted
/// via `Arc` so multiple paths (or a path + open handles) can hold
/// onto the same body and `unlink` only severs the parent's link.
#[derive(Clone)]
pub enum TmpNode {
    File(FileBody),
    Dir(DirBody),
}

impl Tmpfs {
    /// Expose the root directory body so the overlay-persistence
    /// path (Phase D U11) can recursively walk + serialize the entire
    /// tree. Returns a clone of the Arc so the caller can lock it
    /// without holding any tmpfs-internal locks.
    pub fn root_dir(&self) -> DirBody {
        Arc::clone(&self.root)
    }

    pub(crate) fn restore_node_times(
        &self,
        path: &str,
        times: NodeTimes,
    ) -> Result<(), FilesystemError> {
        match self.resolve(path).ok_or(FilesystemError::NotFound)? {
            TmpNode::File(body) => body.lock().times = times,
            TmpNode::Dir(body) => body.lock().times = times,
        }
        Ok(())
    }
}

impl TmpNode {
    pub fn is_dir(&self) -> bool {
        matches!(self, TmpNode::Dir(_))
    }
    pub fn size(&self) -> u64 {
        match self {
            TmpNode::File(body) => body.lock().data.len() as u64,
            TmpNode::Dir(_) => 0,
        }
    }

    fn times(&self) -> NodeTimes {
        match self {
            TmpNode::File(body) => body.lock().times,
            TmpNode::Dir(body) => body.lock().times,
        }
    }

    fn touch_changed(&self, now: UnixTimestamp) {
        match self {
            TmpNode::File(body) => body.lock().times.changed = now,
            TmpNode::Dir(body) => body.lock().times.changed = now,
        }
    }
}

/// Per-open-handle state held in the side table. Anchors a strong
/// reference to the file body so unlinked-but-open files survive.
struct OpenFile {
    body: FileBody,
    mode: FileMode,
}

/// The `Tmpfs` itself. Owns the root directory and the open-handle
/// table.
pub struct Tmpfs {
    root: DirBody,
    open: Mutex<BTreeMap<u64, OpenFile>>,
    next_handle_id: AtomicU64,
}

impl Tmpfs {
    pub fn new() -> Self {
        Self {
            root: new_dir_body(),
            open: Mutex::new(BTreeMap::new()),
            next_handle_id: AtomicU64::new(1),
        }
    }

    /// Split a path into components, skipping leading slash and any
    /// empty segments (e.g., trailing `/`, doubled `//`).
    fn split_path(path: &str) -> Vec<&str> {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }

    /// Resolve a path to a `TmpNode`. Returns `None` if any
    /// intermediate component is missing OR if an intermediate
    /// component exists as a file. Returns the root dir for `"/"`.
    fn resolve(&self, path: &str) -> Option<TmpNode> {
        let comps = Self::split_path(path);
        if comps.is_empty() {
            return Some(TmpNode::Dir(Arc::clone(&self.root)));
        }
        let mut current: DirBody = Arc::clone(&self.root);
        for (i, c) in comps.iter().enumerate() {
            let child = {
                let dir = current.lock();
                dir.children.get(*c).cloned()
            };
            match child {
                Some(TmpNode::Dir(d)) => current = d,
                Some(TmpNode::File(_)) if i == comps.len() - 1 => {
                    return child;
                }
                Some(TmpNode::File(_)) => return None,
                None => return None,
            }
        }
        Some(TmpNode::Dir(current))
    }

    /// Resolve the PARENT directory of `path` plus the final component
    /// name. For root (`/`) returns `None` since root has no parent.
    fn resolve_parent<'p>(&self, path: &'p str) -> Option<(DirBody, &'p str)> {
        let comps_owned: Vec<&'p str> = Self::split_path(path);
        if comps_owned.is_empty() {
            return None;
        }
        let last = *comps_owned.last().unwrap();
        let mut current: DirBody = Arc::clone(&self.root);
        for c in &comps_owned[..comps_owned.len() - 1] {
            let child = {
                let dir = current.lock();
                dir.children.get(*c).cloned()
            };
            match child {
                Some(TmpNode::Dir(d)) => current = d,
                _ => return None,
            }
        }
        Some((current, last))
    }

    /// Open an existing file by node, returning a fresh handle id.
    fn register_open_file(&self, body: FileBody, mode: FileMode) -> u64 {
        let id = self.next_handle_id.fetch_add(1, Ordering::Relaxed);
        let mut tbl = self.open.lock();
        tbl.insert(id, OpenFile { body, mode });
        id
    }
}

/// Validate that a single path component is non-empty and contains no
/// path separator. Caller is responsible for higher-level checks.
fn valid_component(c: &str) -> bool {
    !c.is_empty() && !c.contains('/') && !c.contains('\0')
}

impl Filesystem for Tmpfs {
    fn name(&self) -> &str {
        "tmpfs"
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn stats(&self) -> Result<FilesystemStats, FilesystemError> {
        Ok(FilesystemStats {
            total_blocks: 0,
            free_blocks: 0,
            block_size: 1,
            total_inodes: 0,
            free_inodes: 0,
        })
    }

    fn read_dir(&self, _path: &str) -> Result<DirectoryIterator<'_>, FilesystemError> {
        // We override enumerate_dir; the iterator API is unused for
        // tmpfs.
        Err(FilesystemError::UnsupportedOperation)
    }

    fn enumerate_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FilesystemError> {
        let node = self.resolve(path).ok_or(FilesystemError::NotFound)?;
        let dir = match node {
            TmpNode::Dir(d) => d,
            _ => return Err(FilesystemError::NotADirectory),
        };

        let mut entries = Vec::new();
        let children = dir.lock();
        for (name, child) in children.children.iter() {
            let times = child.times();
            let bytes = name.as_bytes();
            let copy = bytes.len().min(255);
            let mut entry = DirectoryEntry {
                name: [0u8; 256],
                name_len: copy,
                file_type: match child {
                    TmpNode::Dir(_) => FileType::Directory,
                    TmpNode::File(_) => FileType::File,
                },
                size: child.size(),
                attributes: FileAttributes {
                    read_only: false,
                    hidden: false,
                    system: false,
                    archive: false,
                },
                created: times.changed.seconds,
                modified: times.modified.seconds,
                accessed: times.accessed.seconds,
            };
            entry.name[..copy].copy_from_slice(&bytes[..copy]);
            entries.push(entry);
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        let node = self.resolve(path).ok_or(FilesystemError::NotFound)?;
        let times = node.times();
        // Use the last component as the name (or "/" for root).
        let comps = Self::split_path(path);
        let name = comps.last().copied().unwrap_or("/");
        let bytes = name.as_bytes();
        let copy = bytes.len().min(255);
        let mut entry = DirectoryEntry {
            name: [0u8; 256],
            name_len: copy,
            file_type: if node.is_dir() {
                FileType::Directory
            } else {
                FileType::File
            },
            size: node.size(),
            attributes: FileAttributes {
                read_only: false,
                hidden: false,
                system: false,
                archive: false,
            },
            created: times.changed.seconds,
            modified: times.modified.seconds,
            accessed: times.accessed.seconds,
        };
        entry.name[..copy].copy_from_slice(&bytes[..copy]);
        Ok(entry)
    }

    fn unix_metadata(&self, path: &str) -> Result<UnixMetadata, FilesystemError> {
        let node = self.resolve(path).ok_or(FilesystemError::NotFound)?;
        let times = node.times();
        let size = node.size();
        Ok(UnixMetadata {
            inode: 0,
            mode: if node.is_dir() { 0o040755 } else { 0o100644 },
            uid: 0,
            gid: 0,
            links: if node.is_dir() { 2 } else { 1 },
            size,
            blocks_512: size.div_ceil(512),
            block_size: 1,
            accessed: times.accessed,
            modified: times.modified,
            changed: times.changed,
        })
    }

    fn handle_metadata(&self, handle: &FileHandle) -> Result<UnixMetadata, FilesystemError> {
        let body = {
            let open = self.open.lock();
            Arc::clone(
                &open
                    .get(&handle.inode)
                    .ok_or(FilesystemError::IoError)?
                    .body,
            )
        };
        let file = body.lock();
        let size = file.data.len() as u64;
        Ok(UnixMetadata {
            inode: handle.inode,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            links: 1,
            size,
            blocks_512: size.div_ceil(512),
            block_size: 1,
            accessed: file.times.accessed,
            modified: file.times.modified,
            changed: file.times.changed,
        })
    }

    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError> {
        // Walk the parent. Resolve or create the leaf.
        let (parent, leaf) = self
            .resolve_parent(path)
            .ok_or(FilesystemError::InvalidPath)?;
        if !valid_component(leaf) {
            return Err(FilesystemError::InvalidPath);
        }

        let body = {
            let mut parent_dir = parent.lock();
            match parent_dir.children.get(leaf).cloned() {
                Some(TmpNode::File(body)) => {
                    if mode.truncate && mode.write {
                        let now = current_time();
                        let mut file = body.lock();
                        file.data.clear();
                        file.times.touch_content(now);
                    }
                    body
                }
                Some(TmpNode::Dir(_)) => return Err(FilesystemError::IsADirectory),
                None => {
                    if !mode.create {
                        return Err(FilesystemError::NotFound);
                    }
                    let now = current_time();
                    let body = new_file_body();
                    body.lock().times = NodeTimes {
                        accessed: now,
                        modified: now,
                        changed: now,
                    };
                    parent_dir
                        .children
                        .insert(leaf.to_string(), TmpNode::File(Arc::clone(&body)));
                    parent_dir.times.touch_namespace(now);
                    body
                }
            }
        };

        let size = body.lock().data.len() as u64;
        let id = self.register_open_file(body, mode);
        Ok(FileHandle {
            inode: id,
            position: 0,
            size,
            mode,
        })
    }

    fn close(&self, handle: &mut FileHandle) -> Result<(), FilesystemError> {
        let mut tbl = self.open.lock();
        tbl.remove(&handle.inode);
        Ok(())
    }

    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError> {
        let entry = {
            let tbl = self.open.lock();
            tbl.get(&handle.inode)
                .map(|of| Arc::clone(&of.body))
                .ok_or(FilesystemError::IoError)?
        };
        let file = entry.lock();
        if handle.position >= file.data.len() as u64 {
            return Ok(0);
        }
        let start = handle.position as usize;
        let end = (start + buffer.len()).min(file.data.len());
        let n = end - start;
        buffer[..n].copy_from_slice(&file.data[start..end]);
        handle.position += n as u64;
        handle.size = file.data.len() as u64;
        Ok(n)
    }

    fn write(&self, handle: &mut FileHandle, buffer: &[u8]) -> Result<usize, FilesystemError> {
        // Mode check.
        let entry = {
            let tbl = self.open.lock();
            let of = tbl.get(&handle.inode).ok_or(FilesystemError::IoError)?;
            if !of.mode.write {
                return Err(FilesystemError::PermissionDenied);
            }
            Arc::clone(&of.body)
        };
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut file = entry.lock();
        let start = handle.position as usize;
        let needed_len = start + buffer.len();
        if file.data.len() < needed_len {
            file.data.resize(needed_len, 0);
        }
        file.data[start..start + buffer.len()].copy_from_slice(buffer);
        file.times.touch_content(current_time());
        handle.position = needed_len as u64;
        handle.size = file.data.len() as u64;
        Ok(buffer.len())
    }

    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        // POSIX permits seeking past EOF; size grows on next write.
        handle.position = position;
        Ok(position)
    }

    fn truncate(&self, handle: &mut FileHandle, size: u64) -> Result<(), FilesystemError> {
        ftruncate(self, handle, size)
    }

    fn set_times(
        &self,
        path: &str,
        accessed: Option<UnixTimestamp>,
        modified: Option<UnixTimestamp>,
    ) -> Result<(), FilesystemError> {
        let node = self.resolve(path).ok_or(FilesystemError::NotFound)?;
        if accessed.is_none() && modified.is_none() {
            return Ok(());
        }
        let changed = current_time();
        match node {
            TmpNode::File(body) => {
                let mut file = body.lock();
                if let Some(accessed) = accessed {
                    file.times.accessed = accessed;
                }
                if let Some(modified) = modified {
                    file.times.modified = modified;
                }
                file.times.changed = changed;
            }
            TmpNode::Dir(body) => {
                let mut dir = body.lock();
                if let Some(accessed) = accessed {
                    dir.times.accessed = accessed;
                }
                if let Some(modified) = modified {
                    dir.times.modified = modified;
                }
                dir.times.changed = changed;
            }
        }
        Ok(())
    }

    fn mkdir(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, leaf) = self
            .resolve_parent(path)
            .ok_or(FilesystemError::InvalidPath)?;
        if !valid_component(leaf) {
            return Err(FilesystemError::InvalidPath);
        }
        let mut dir = parent.lock();
        if dir.children.contains_key(leaf) {
            return Err(FilesystemError::AlreadyExists);
        }
        let now = current_time();
        let child = new_dir_body();
        child.lock().times = NodeTimes {
            accessed: now,
            modified: now,
            changed: now,
        };
        dir.children.insert(leaf.to_string(), TmpNode::Dir(child));
        dir.times.touch_namespace(now);
        Ok(())
    }

    fn unlink(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, leaf) = self
            .resolve_parent(path)
            .ok_or(FilesystemError::InvalidPath)?;
        let mut dir = parent.lock();
        match dir.children.get(leaf).cloned() {
            Some(node @ TmpNode::File(_)) => {
                let now = current_time();
                node.touch_changed(now);
                dir.children.remove(leaf);
                dir.times.touch_namespace(now);
                Ok(())
            }
            Some(TmpNode::Dir(_)) => Err(FilesystemError::IsADirectory),
            None => Err(FilesystemError::NotFound),
        }
    }

    fn rmdir(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, leaf) = self
            .resolve_parent(path)
            .ok_or(FilesystemError::InvalidPath)?;
        let mut dir = parent.lock();
        match dir.children.get(leaf).cloned() {
            Some(TmpNode::Dir(d)) => {
                let inner = d.lock();
                if !inner.children.is_empty() {
                    return Err(FilesystemError::NotEmpty);
                }
                drop(inner);
                let now = current_time();
                TmpNode::Dir(Arc::clone(&d)).touch_changed(now);
                dir.children.remove(leaf);
                dir.times.touch_namespace(now);
                Ok(())
            }
            Some(TmpNode::File(_)) => Err(FilesystemError::NotADirectory),
            None => Err(FilesystemError::NotFound),
        }
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
        if old_path == new_path {
            return self
                .resolve(old_path)
                .map(|_| ())
                .ok_or(FilesystemError::NotFound);
        }
        let (src_parent, src_leaf) = self
            .resolve_parent(old_path)
            .ok_or(FilesystemError::InvalidPath)?;
        let (dst_parent, dst_leaf) = self
            .resolve_parent(new_path)
            .ok_or(FilesystemError::InvalidPath)?;
        if !valid_component(dst_leaf) {
            return Err(FilesystemError::InvalidPath);
        }

        // Same-directory rename: take a single lock to avoid ordering.
        if Arc::ptr_eq(&src_parent, &dst_parent) {
            let mut dir = src_parent.lock();
            let node = dir
                .children
                .remove(src_leaf)
                .ok_or(FilesystemError::NotFound)?;
            let node_is_dir = node.is_dir();
            if let Some(existing) = dir.children.get(dst_leaf) {
                // POSIX rename atomically replaces a regular file
                // destination; reject directory-over-file or
                // file-over-directory mismatches.
                if existing.is_dir() != node_is_dir {
                    // Restore and fail.
                    dir.children.insert(src_leaf.to_string(), node);
                    return Err(if node_is_dir {
                        FilesystemError::NotADirectory
                    } else {
                        FilesystemError::IsADirectory
                    });
                }
            }
            let now = current_time();
            node.touch_changed(now);
            if let Some(replaced) = dir.children.insert(dst_leaf.to_string(), node) {
                replaced.touch_changed(now);
            }
            dir.times.touch_namespace(now);
            return Ok(());
        }

        // Cross-directory rename. Lock in deterministic order to avoid
        // the (theoretical) deadlock if the kernel ever goes
        // preemptive. Using Arc's pointer address for ordering.
        let (a, b) = if (Arc::as_ptr(&src_parent) as usize) < (Arc::as_ptr(&dst_parent) as usize) {
            (&src_parent, &dst_parent)
        } else {
            (&dst_parent, &src_parent)
        };
        let mut a_dir = a.lock();
        let mut b_dir = b.lock();

        // Identify src and dst maps via pointer equality.
        let same_as_src = Arc::ptr_eq(a, &src_parent);
        let (src_dir, dst_dir): (
            &mut BTreeMap<String, TmpNode>,
            &mut BTreeMap<String, TmpNode>,
        ) = if same_as_src {
            (&mut a_dir.children, &mut b_dir.children)
        } else {
            (&mut b_dir.children, &mut a_dir.children)
        };

        let node = src_dir.remove(src_leaf).ok_or(FilesystemError::NotFound)?;
        let node_is_dir = node.is_dir();
        if let Some(existing) = dst_dir.get(dst_leaf) {
            if existing.is_dir() != node_is_dir {
                src_dir.insert(src_leaf.to_string(), node);
                return Err(if node_is_dir {
                    FilesystemError::NotADirectory
                } else {
                    FilesystemError::IsADirectory
                });
            }
        }
        let now = current_time();
        node.touch_changed(now);
        if let Some(replaced) = dst_dir.insert(dst_leaf.to_string(), node) {
            replaced.touch_changed(now);
        }
        a_dir.times.touch_namespace(now);
        b_dir.times.touch_namespace(now);
        Ok(())
    }

    fn sync(&self) -> Result<(), FilesystemError> {
        Ok(())
    }
}

/// Truncate a file to `new_size`. Extending writes zeros (POSIX
/// `ftruncate`). Kept as a helper so the trait implementation and focused
/// tmpfs tests exercise the same mutation path.
pub fn ftruncate(
    fs: &Tmpfs,
    handle: &mut FileHandle,
    new_size: u64,
) -> Result<(), FilesystemError> {
    let entry = {
        let tbl = fs.open.lock();
        let of = tbl.get(&handle.inode).ok_or(FilesystemError::IoError)?;
        if !of.mode.write {
            return Err(FilesystemError::PermissionDenied);
        }
        Arc::clone(&of.body)
    };
    let mut file = entry.lock();
    file.data.resize(new_size as usize, 0);
    file.times.touch_content(current_time());
    handle.size = new_size;
    Ok(())
}

#[cfg(feature = "test")]
mod tests {
    use super::*;
    use crate::lib::test_utils::Testable;

    fn open_write_read(fs: &Tmpfs, path: &str, content: &[u8]) {
        let mut h = fs
            .open(
                path,
                FileMode {
                    read: true,
                    write: true,
                    append: false,
                    create: true,
                    truncate: true,
                },
            )
            .expect("open create");
        let n = fs.write(&mut h, content).expect("write");
        assert_eq!(n, content.len());
        fs.close(&mut h).expect("close");

        let mut h2 = fs.open(path, FileMode::READ).expect("open read");
        let mut buf = alloc::vec![0u8; content.len()];
        let n = fs.read(&mut h2, &mut buf).expect("read");
        assert_eq!(n, content.len());
        assert_eq!(&buf[..n], content);
        fs.close(&mut h2).expect("close");
    }

    fn test_tmpfs_write_then_read() {
        let fs = Tmpfs::new();
        open_write_read(&fs, "/hello.txt", b"hello world");
    }

    fn test_tmpfs_mkdir_and_nested_file() {
        let fs = Tmpfs::new();
        fs.mkdir("/var").expect("mkdir /var");
        fs.mkdir("/var/log").expect("mkdir /var/log");
        open_write_read(&fs, "/var/log/syslog", b"line1\nline2\n");

        let entries = fs.enumerate_dir("/var/log").expect("enumerate");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name_str(), "syslog");
        assert_eq!(entries[0].file_type, FileType::File);
    }

    fn test_tmpfs_unlink() {
        let fs = Tmpfs::new();
        open_write_read(&fs, "/foo", b"x");
        fs.unlink("/foo").expect("unlink");
        assert!(matches!(
            fs.open("/foo", FileMode::READ),
            Err(FilesystemError::NotFound)
        ));
    }

    fn test_tmpfs_unlink_while_open_keeps_data() {
        let fs = Tmpfs::new();
        open_write_read(&fs, "/ephemeral", b"data");
        let mut h = fs.open("/ephemeral", FileMode::READ).expect("open");
        fs.unlink("/ephemeral").expect("unlink");
        // Read should still work — node anchored in open_handles.
        let mut buf = [0u8; 4];
        let n = fs.read(&mut h, &mut buf).expect("read after unlink");
        assert_eq!(n, 4);
        assert_eq!(&buf, b"data");
        fs.close(&mut h).expect("close");
        // After close, the node is gone.
        assert!(matches!(
            fs.open("/ephemeral", FileMode::READ),
            Err(FilesystemError::NotFound)
        ));
    }

    fn test_tmpfs_rmdir_rejects_non_empty() {
        let fs = Tmpfs::new();
        fs.mkdir("/dir").expect("mkdir");
        open_write_read(&fs, "/dir/x", b"y");
        assert!(matches!(fs.rmdir("/dir"), Err(FilesystemError::NotEmpty)));
        fs.unlink("/dir/x").expect("unlink x");
        fs.rmdir("/dir").expect("rmdir empty");
    }

    fn test_tmpfs_rename_same_dir() {
        let fs = Tmpfs::new();
        open_write_read(&fs, "/a", b"data");
        fs.rename("/a", "/b").expect("rename");
        assert!(matches!(
            fs.open("/a", FileMode::READ),
            Err(FilesystemError::NotFound)
        ));
        let mut h = fs.open("/b", FileMode::READ).expect("open /b");
        let mut buf = [0u8; 4];
        fs.read(&mut h, &mut buf).expect("read");
        assert_eq!(&buf, b"data");
    }

    fn test_tmpfs_rename_cross_dir() {
        let fs = Tmpfs::new();
        fs.mkdir("/x").expect("mkdir x");
        fs.mkdir("/y").expect("mkdir y");
        open_write_read(&fs, "/x/a", b"hi");
        fs.rename("/x/a", "/y/a").expect("rename cross");
        assert!(matches!(
            fs.open("/x/a", FileMode::READ),
            Err(FilesystemError::NotFound)
        ));
        let mut h = fs.open("/y/a", FileMode::READ).expect("open /y/a");
        let mut buf = [0u8; 2];
        fs.read(&mut h, &mut buf).expect("read");
        assert_eq!(&buf, b"hi");
    }

    fn test_tmpfs_write_past_eof_extends() {
        let fs = Tmpfs::new();
        let mut h = fs
            .open(
                "/sparse",
                FileMode {
                    read: true,
                    write: true,
                    append: false,
                    create: true,
                    truncate: true,
                },
            )
            .expect("open");
        fs.seek(&mut h, 10).expect("seek 10");
        let n = fs.write(&mut h, b"AB").expect("write");
        assert_eq!(n, 2);
        // File should be 12 bytes total, with zeros in 0..10.
        assert_eq!(h.size, 12);
        fs.seek(&mut h, 0).expect("seek 0");
        let mut buf = [0u8; 12];
        let r = fs.read(&mut h, &mut buf).expect("read");
        assert_eq!(r, 12);
        assert_eq!(&buf[..10], &[0u8; 10]);
        assert_eq!(&buf[10..], b"AB");
    }

    fn test_tmpfs_mkdir_rejects_existing() {
        let fs = Tmpfs::new();
        fs.mkdir("/dup").expect("mkdir first");
        assert!(matches!(
            fs.mkdir("/dup"),
            Err(FilesystemError::AlreadyExists)
        ));
    }

    fn test_tmpfs_unlink_directory_returns_isadir() {
        let fs = Tmpfs::new();
        fs.mkdir("/dir").expect("mkdir");
        assert!(matches!(
            fs.unlink("/dir"),
            Err(FilesystemError::IsADirectory)
        ));
    }

    fn test_tmpfs_open_directory_returns_isadir() {
        let fs = Tmpfs::new();
        fs.mkdir("/dir").expect("mkdir");
        assert!(matches!(
            fs.open("/dir", FileMode::READ),
            Err(FilesystemError::IsADirectory)
        ));
    }

    fn test_tmpfs_ftruncate_extends_and_shrinks() {
        let fs = Tmpfs::new();
        let mut h = fs
            .open(
                "/t",
                FileMode {
                    read: true,
                    write: true,
                    append: false,
                    create: true,
                    truncate: true,
                },
            )
            .expect("open");
        fs.write(&mut h, b"hello").expect("write");
        ftruncate(&fs, &mut h, 100).expect("extend");
        assert_eq!(h.size, 100);
        fs.seek(&mut h, 0).expect("seek");
        let mut buf = [0u8; 100];
        fs.read(&mut h, &mut buf).expect("read");
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(&buf[5..], &[0u8; 95]);
        ftruncate(&fs, &mut h, 3).expect("shrink");
        assert_eq!(h.size, 3);
    }

    fn test_tmpfs_set_times_roundtrip_and_omit() {
        let fs = Tmpfs::new();
        open_write_read(&fs, "/dated", b"x");
        let accessed = UnixTimestamp {
            seconds: 123,
            nanoseconds: 120_000_000,
        };
        let modified = UnixTimestamp {
            seconds: 456,
            nanoseconds: 340_000_000,
        };
        fs.set_times("/dated", Some(accessed), Some(modified))
            .expect("set both times");
        let first = fs.unix_metadata("/dated").expect("stat dated");
        assert_eq!(first.accessed, accessed);
        assert_eq!(first.modified, modified);

        fs.set_times(
            "/dated",
            None,
            Some(UnixTimestamp {
                seconds: 789,
                nanoseconds: 560_000_000,
            }),
        )
        .expect("omit atime");
        let second = fs.unix_metadata("/dated").expect("stat dated again");
        assert_eq!(second.accessed, accessed);
        assert_eq!(second.modified.seconds, 789);
        assert_eq!(second.modified.nanoseconds, 560_000_000);
    }

    fn old_times() -> NodeTimes {
        NodeTimes {
            accessed: UnixTimestamp::from_seconds(1),
            modified: UnixTimestamp::from_seconds(2),
            changed: UnixTimestamp::from_seconds(3),
        }
    }

    fn assert_kernel_time(value: UnixTimestamp) {
        assert!(
            value.seconds > 3,
            "automatic timestamp must replace fixture time"
        );
        assert_eq!(
            value.nanoseconds % 10_000_000,
            0,
            "automatic timestamps must expose PIT's 10 ms resolution"
        );
    }

    fn test_tmpfs_mutations_update_file_times() {
        let fs = Tmpfs::new();
        let mut handle = fs
            .open(
                "/mutated",
                FileMode {
                    read: true,
                    write: true,
                    append: false,
                    create: true,
                    truncate: true,
                },
            )
            .expect("create fixture");

        fs.restore_node_times("/mutated", old_times()).unwrap();
        fs.write(&mut handle, b"x").expect("write");
        let after_write = fs.unix_metadata("/mutated").unwrap();
        assert_kernel_time(after_write.modified);
        assert_kernel_time(after_write.changed);

        fs.restore_node_times("/mutated", old_times()).unwrap();
        ftruncate(&fs, &mut handle, 1).expect("same-size truncate");
        let after_truncate = fs.unix_metadata("/mutated").unwrap();
        assert_kernel_time(after_truncate.modified);
        assert_kernel_time(after_truncate.changed);

        fs.restore_node_times("/mutated", old_times()).unwrap();
        assert_eq!(fs.write(&mut handle, b"").expect("empty write"), 0);
        let after_empty_write = fs.unix_metadata("/mutated").unwrap();
        assert_eq!(after_empty_write.modified, old_times().modified);
        assert_eq!(after_empty_write.changed, old_times().changed);

        fs.restore_node_times("/mutated", old_times()).unwrap();
        let mut truncated = fs
            .open(
                "/mutated",
                FileMode {
                    read: false,
                    write: true,
                    append: false,
                    create: false,
                    truncate: true,
                },
            )
            .expect("O_TRUNC open");
        let after_open_truncate = fs.unix_metadata("/mutated").unwrap();
        assert_kernel_time(after_open_truncate.modified);
        assert_kernel_time(after_open_truncate.changed);
        fs.close(&mut truncated).unwrap();
        fs.close(&mut handle).unwrap();
    }

    fn test_tmpfs_namespace_mutations_update_parent_times() {
        let fs = Tmpfs::new();
        fs.restore_node_times("/", old_times()).unwrap();
        let mut child = fs
            .open(
                "/child",
                FileMode {
                    read: false,
                    write: true,
                    append: false,
                    create: true,
                    truncate: false,
                },
            )
            .expect("create child");
        let root_after_create = fs.unix_metadata("/").unwrap();
        assert_kernel_time(root_after_create.modified);
        assert_kernel_time(root_after_create.changed);

        fs.restore_node_times("/", old_times()).unwrap();
        fs.unlink("/child").expect("unlink child");
        let root_after_unlink = fs.unix_metadata("/").unwrap();
        assert_kernel_time(root_after_unlink.modified);
        assert_kernel_time(root_after_unlink.changed);
        let unlinked = fs.handle_metadata(&child).expect("fstat unlinked child");
        assert_kernel_time(unlinked.changed);
        fs.close(&mut child).unwrap();

        fs.restore_node_times("/", old_times()).unwrap();
        fs.mkdir("/empty").expect("mkdir child");
        let root_after_mkdir = fs.unix_metadata("/").unwrap();
        assert_kernel_time(root_after_mkdir.modified);
        assert_kernel_time(root_after_mkdir.changed);
        fs.restore_node_times("/", old_times()).unwrap();
        fs.rmdir("/empty").expect("rmdir child");
        let root_after_rmdir = fs.unix_metadata("/").unwrap();
        assert_kernel_time(root_after_rmdir.modified);
        assert_kernel_time(root_after_rmdir.changed);

        fs.mkdir("/src").unwrap();
        fs.mkdir("/dst").unwrap();
        let mut moved = fs
            .open(
                "/src/file",
                FileMode {
                    read: false,
                    write: true,
                    append: false,
                    create: true,
                    truncate: false,
                },
            )
            .unwrap();
        fs.restore_node_times("/src", old_times()).unwrap();
        fs.restore_node_times("/dst", old_times()).unwrap();
        fs.restore_node_times("/src/file", old_times()).unwrap();
        fs.rename("/src/file", "/dst/file")
            .expect("cross-dir rename");
        for path in ["/src", "/dst"] {
            let parent = fs.unix_metadata(path).unwrap();
            assert_kernel_time(parent.modified);
            assert_kernel_time(parent.changed);
        }
        assert_kernel_time(fs.unix_metadata("/dst/file").unwrap().changed);
        fs.close(&mut moved).unwrap();
    }

    pub fn get_tests() -> &'static [&'static dyn Testable] {
        &[
            &test_tmpfs_write_then_read,
            &test_tmpfs_mkdir_and_nested_file,
            &test_tmpfs_unlink,
            &test_tmpfs_unlink_while_open_keeps_data,
            &test_tmpfs_rmdir_rejects_non_empty,
            &test_tmpfs_rename_same_dir,
            &test_tmpfs_rename_cross_dir,
            &test_tmpfs_write_past_eof_extends,
            &test_tmpfs_mkdir_rejects_existing,
            &test_tmpfs_unlink_directory_returns_isadir,
            &test_tmpfs_open_directory_returns_isadir,
            &test_tmpfs_ftruncate_extends_and_shrinks,
            &test_tmpfs_set_times_roundtrip_and_omit,
            &test_tmpfs_mutations_update_file_times,
            &test_tmpfs_namespace_mutations_update_parent_times,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as tmpfs_tests;
