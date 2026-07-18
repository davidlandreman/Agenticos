//! `Filesystem` trait implementation for tmpfs.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::fs::filesystem::{
    DirectoryEntry, DirectoryIterator, FileAttributes, FileHandle, FileMode, FileType, Filesystem,
    FilesystemError, FilesystemStats,
};
use crate::lib::arc::Arc;

/// In-memory file body.
pub type FileBody = Arc<Mutex<Vec<u8>>>;

/// In-memory directory body. Children are name → node.
pub type DirBody = Arc<Mutex<BTreeMap<String, TmpNode>>>;

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
}

impl TmpNode {
    pub fn is_dir(&self) -> bool {
        matches!(self, TmpNode::Dir(_))
    }
    pub fn size(&self) -> u64 {
        match self {
            TmpNode::File(body) => body.lock().len() as u64,
            TmpNode::Dir(_) => 0,
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
            root: Arc::new(Mutex::new(BTreeMap::new())),
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
                dir.get(*c).cloned()
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
                dir.get(*c).cloned()
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
        for (name, child) in children.iter() {
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
                created: 0,
                modified: 0,
                accessed: 0,
            };
            entry.name[..copy].copy_from_slice(&bytes[..copy]);
            entries.push(entry);
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        let node = self.resolve(path).ok_or(FilesystemError::NotFound)?;
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
            created: 0,
            modified: 0,
            accessed: 0,
        };
        entry.name[..copy].copy_from_slice(&bytes[..copy]);
        Ok(entry)
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
            match parent_dir.get(leaf).cloned() {
                Some(TmpNode::File(body)) => {
                    if mode.truncate && mode.write {
                        body.lock().clear();
                    }
                    body
                }
                Some(TmpNode::Dir(_)) => return Err(FilesystemError::IsADirectory),
                None => {
                    if !mode.create {
                        return Err(FilesystemError::NotFound);
                    }
                    let body = Arc::new(Mutex::new(Vec::new()));
                    parent_dir.insert(leaf.to_string(), TmpNode::File(Arc::clone(&body)));
                    body
                }
            }
        };

        let size = body.lock().len() as u64;
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
        let data = entry.lock();
        if handle.position >= data.len() as u64 {
            return Ok(0);
        }
        let start = handle.position as usize;
        let end = (start + buffer.len()).min(data.len());
        let n = end - start;
        buffer[..n].copy_from_slice(&data[start..end]);
        handle.position += n as u64;
        handle.size = data.len() as u64;
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
        let mut data = entry.lock();
        let start = handle.position as usize;
        let needed_len = start + buffer.len();
        if data.len() < needed_len {
            data.resize(needed_len, 0);
        }
        data[start..start + buffer.len()].copy_from_slice(buffer);
        handle.position = needed_len as u64;
        handle.size = data.len() as u64;
        Ok(buffer.len())
    }

    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        // POSIX permits seeking past EOF; size grows on next write.
        handle.position = position;
        Ok(position)
    }

    fn mkdir(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, leaf) = self
            .resolve_parent(path)
            .ok_or(FilesystemError::InvalidPath)?;
        if !valid_component(leaf) {
            return Err(FilesystemError::InvalidPath);
        }
        let mut dir = parent.lock();
        if dir.contains_key(leaf) {
            return Err(FilesystemError::AlreadyExists);
        }
        dir.insert(
            leaf.to_string(),
            TmpNode::Dir(Arc::new(Mutex::new(BTreeMap::new()))),
        );
        Ok(())
    }

    fn unlink(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, leaf) = self
            .resolve_parent(path)
            .ok_or(FilesystemError::InvalidPath)?;
        let mut dir = parent.lock();
        match dir.get(leaf) {
            Some(TmpNode::File(_)) => {
                dir.remove(leaf);
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
        match dir.get(leaf) {
            Some(TmpNode::Dir(d)) => {
                let inner = d.lock();
                if !inner.is_empty() {
                    return Err(FilesystemError::NotEmpty);
                }
                drop(inner);
                dir.remove(leaf);
                Ok(())
            }
            Some(TmpNode::File(_)) => Err(FilesystemError::NotADirectory),
            None => Err(FilesystemError::NotFound),
        }
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
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
            let node = dir.remove(src_leaf).ok_or(FilesystemError::NotFound)?;
            let node_is_dir = node.is_dir();
            if let Some(existing) = dir.get(dst_leaf) {
                // POSIX rename atomically replaces a regular file
                // destination; reject directory-over-file or
                // file-over-directory mismatches.
                if existing.is_dir() != node_is_dir {
                    // Restore and fail.
                    dir.insert(src_leaf.to_string(), node);
                    return Err(if node_is_dir {
                        FilesystemError::NotADirectory
                    } else {
                        FilesystemError::IsADirectory
                    });
                }
            }
            dir.insert(dst_leaf.to_string(), node);
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
            (&mut *a_dir, &mut *b_dir)
        } else {
            (&mut *b_dir, &mut *a_dir)
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
        dst_dir.insert(dst_leaf.to_string(), node);
        Ok(())
    }

    fn sync(&self) -> Result<(), FilesystemError> {
        Ok(())
    }
}

/// Truncate a file to `new_size`. Extending writes zeros (POSIX
/// `ftruncate`). Lives outside the trait until `Filesystem::truncate`
/// is added in U5.
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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
    let mut data = entry.lock();
    data.resize(new_size as usize, 0);
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
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as tmpfs_tests;
