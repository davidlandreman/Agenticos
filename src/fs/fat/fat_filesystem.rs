use crate::fs::filesystem::{
    Filesystem, FilesystemError, FilesystemStats, DirectoryEntry, DirectoryIterator,
    FileHandle, FileMode, FileType, FileAttributes
};
use crate::fs::fat::filesystem::FatFilesystem as FatFs;
use crate::fs::fat::types::{ClusterId, FatType};
use alloc::collections::BTreeMap;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Per-open-file metadata the writable wrapper needs to update the
/// SFN after each write. Keyed by `FileHandle.inode` (a synthesized
/// handle ID, NOT a cluster) so the same path opened twice gets
/// distinct entries.
struct OpenWrite {
    parent_cluster: Option<ClusterId>,
    leaf: String,
    /// Current first cluster of the file. Starts at 0 for newly
    /// created empty files; populated by the first `write` once the
    /// FAT writer allocates the head cluster.
    first_cluster: ClusterId,
    /// Current size in bytes (kept in sync with the SFN entry).
    size: u64,
}

/// Wrapper to implement the Filesystem trait for FAT
pub struct FatFilesystemWrapper<'a> {
    inner: FatFs<'a>,
    /// When true, writes flow through to the FAT writer. When false,
    /// every write-side trait method returns `ReadOnly`.
    writable: bool,
    /// Per-open-handle side table for write tracking.
    open_writes: Mutex<BTreeMap<u64, OpenWrite>>,
    /// Monotonic handle ID generator for synthesizing
    /// `FileHandle.inode`. Starts at a high value to avoid colliding
    /// with read-only handles that use cluster IDs as inodes.
    next_handle_id: AtomicU64,
}

const HANDLE_ID_BASE: u64 = 1u64 << 40;

impl<'a> FatFilesystemWrapper<'a> {
    pub fn new(inner: FatFs<'a>) -> Self {
        Self {
            inner,
            writable: false,
            open_writes: Mutex::new(BTreeMap::new()),
            next_handle_id: AtomicU64::new(HANDLE_ID_BASE),
        }
    }

    /// Construct a writable wrapper. Caller must have already
    /// successfully called `inner.enable_writes(...)` (the C-2
    /// dirty-bit gate). Marking the wrapper writable just tells
    /// `is_read_only()` to return false and the trait methods to
    /// route through the FAT writer instead of returning ReadOnly.
    pub fn new_writable(inner: FatFs<'a>) -> Self {
        Self {
            inner,
            writable: true,
            open_writes: Mutex::new(BTreeMap::new()),
            next_handle_id: AtomicU64::new(HANDLE_ID_BASE),
        }
    }

    /// Internal helper: allocate a synthetic handle ID for an
    /// open-for-write. Distinguishes writable handles from the
    /// existing inode-as-cluster pattern via the high bit.
    fn alloc_handle_id(&self) -> u64 {
        self.next_handle_id.fetch_add(1, Ordering::Relaxed)
    }

    /// True if `inode` is a writable-handle ID issued by
    /// `alloc_handle_id`, not a read-only cluster-based inode.
    fn is_write_handle(&self, inode: u64) -> bool {
        inode >= HANDLE_ID_BASE
    }
}

impl<'a> Filesystem for FatFilesystemWrapper<'a> {
    fn name(&self) -> &str {
        match self.inner.fat_type() {
            FatType::Fat12 => "FAT12",
            FatType::Fat16 => "FAT16", 
            FatType::Fat32 => "FAT32",
        }
    }
    
    fn is_read_only(&self) -> bool {
        !self.writable
    }
    
    fn stats(&self) -> Result<FilesystemStats, FilesystemError> {
        // This would require additional methods on FatFilesystem
        // For now, return dummy values
        Ok(FilesystemStats {
            total_blocks: 0,
            free_blocks: 0,
            block_size: 512,
            total_inodes: 0,
            free_inodes: 0,
        })
    }
    
    fn read_dir(&self, path: &str) -> Result<DirectoryIterator<'_>, FilesystemError> {
        // Validate the path can be resolved as a directory before
        // returning an iterator. The legacy `DirectoryIterator` only
        // walks the root, but rejecting bad paths up front is cheap
        // and preserves the iterator contract.
        match self.inner.resolve_directory(path) {
            Ok(_) => Ok(DirectoryIterator::new(self, path)),
            Err(_) => Err(FilesystemError::NotFound),
        }
    }

    fn enumerate_dir(&self, path: &str) -> Result<alloc::vec::Vec<DirectoryEntry>, FilesystemError> {
        // Resolve the path to a directory cluster (or root) and then
        // walk the entries via the long-name-aware walker so the
        // returned `DirectoryEntry.name` carries the decoded VFAT LFN
        // (or 8.3 with lowercase-attr bits) rather than the raw 8.3
        // form.
        let cluster = self
            .inner
            .resolve_directory(path)
            .map_err(|_| FilesystemError::NotFound)?;

        let mut entries = alloc::vec::Vec::new();
        self.inner
            .walk_directory(cluster, |name, raw, _first_cluster, is_dir| {
                // Skip FAT "." and ".." synthetic entries — userland
                // gets those from the kernel's path normalizer.
                let name_bytes = name.as_bytes();
                if name_bytes == b"." || name_bytes == b".." {
                    return false;
                }

                let copy_len = name_bytes.len().min(255);
                let mut entry = DirectoryEntry {
                    name: [0u8; 256],
                    name_len: 0,
                    file_type: if is_dir {
                        crate::fs::filesystem::FileType::Directory
                    } else {
                        crate::fs::filesystem::FileType::File
                    },
                    size: raw.file_size as u64,
                    attributes: crate::fs::filesystem::FileAttributes {
                        read_only: raw.attributes().is_read_only(),
                        hidden: raw.attributes().is_hidden(),
                        system: raw.attributes().is_system(),
                        archive: raw.attributes().is_archive(),
                    },
                    created: 0,
                    modified: 0,
                    accessed: 0,
                };
                entry.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
                entry.name_len = copy_len;
                entries.push(entry);
                false
            })
            .map_err(|_| FilesystemError::IoError)?;

        Ok(entries)
    }
    
    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        match self.inner.find_file_with_long_name(path) {
            Ok((fat_file, long_name, long_len)) => {
                let mut entry = DirectoryEntry {
                    name: [0; 256],
                    name_len: 0,
                    file_type: if fat_file.is_directory { FileType::Directory } else { FileType::File },
                    size: fat_file.size as u64,
                    attributes: FileAttributes {
                        read_only: false,  // Would need to parse FAT attributes
                        hidden: false,
                        system: false,
                        archive: false,
                    },
                    created: 0,
                    modified: 0,
                    accessed: 0,
                };
                let copy = long_len.min(256);
                entry.name[..copy].copy_from_slice(&long_name[..copy]);
                entry.name_len = copy;
                Ok(entry)
            }
            Err(_) => Err(FilesystemError::NotFound),
        }
    }
    
    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError> {
        // Read-only fast path: no writable intent.
        if !mode.write && !mode.create && !mode.truncate {
            return match self.inner.find_file(path) {
                Ok(fat_file) => {
                    if fat_file.is_directory {
                        return Err(FilesystemError::IsADirectory);
                    }
                    Ok(FileHandle {
                        inode: fat_file.first_cluster.0 as u64,
                        position: 0,
                        size: fat_file.size as u64,
                        mode,
                    })
                }
                Err(_) => Err(FilesystemError::NotFound),
            };
        }

        // Writable intent — require writable mount.
        if !self.writable {
            return Err(FilesystemError::ReadOnly);
        }

        // Look up the file. Create if missing and create=true.
        let existing = self.inner.find_file(path);
        let (first_cluster, size) = match existing {
            Ok(fh) => {
                if fh.is_directory {
                    return Err(FilesystemError::IsADirectory);
                }
                if mode.truncate {
                    // Free existing chain, reset to empty. Per C-1:
                    // tombstone-or-detach before freeing — here we
                    // detach by zeroing the SFN's first_cluster and
                    // size BEFORE freeing the chain.
                    let (parent, leaf) = self.inner.resolve_parent(path)
                        .map_err(map_fat_err)?;
                    if fh.first_cluster.0 >= 2 {
                        self.inner.update_sfn_size_and_cluster(parent, leaf, ClusterId(0), 0)
                            .map_err(map_fat_err)?;
                        self.inner.free_cluster_chain(fh.first_cluster)
                            .map_err(map_fat_err)?;
                    }
                    (ClusterId(0), 0u64)
                } else {
                    (fh.first_cluster, fh.size as u64)
                }
            }
            Err(_) => {
                if !mode.create {
                    return Err(FilesystemError::NotFound);
                }
                let _new_first = self.inner.create_file(path).map_err(map_fat_err)?;
                (ClusterId(0), 0u64)
            }
        };

        let (parent, leaf) = self.inner.resolve_parent(path).map_err(map_fat_err)?;
        let handle_id = self.alloc_handle_id();
        let mut tbl = self.open_writes.lock();
        tbl.insert(handle_id, OpenWrite {
            parent_cluster: parent,
            leaf: alloc::string::ToString::to_string(leaf),
            first_cluster,
            size,
        });
        Ok(FileHandle {
            inode: handle_id,
            position: if mode.append { size } else { 0 },
            size,
            mode,
        })
    }
    
    fn close(&self, handle: &mut FileHandle) -> Result<(), FilesystemError> {
        if self.is_write_handle(handle.inode) {
            let mut tbl = self.open_writes.lock();
            tbl.remove(&handle.inode);
        }
        Ok(())
    }
    
    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError> {
        if handle.position >= handle.size {
            return Ok(0);
        }

        // For write-side handles, the inode is a synthetic ID, not a
        // cluster. Look up the actual first_cluster in the side
        // table.
        let first_cluster = if self.is_write_handle(handle.inode) {
            let tbl = self.open_writes.lock();
            let entry = tbl.get(&handle.inode).ok_or(FilesystemError::IoError)?;
            entry.first_cluster
        } else {
            ClusterId(handle.inode as u32)
        };

        // Reconstruct the FAT file handle from the generic handle.
        let fat_handle = crate::fs::fat::filesystem::FileHandle {
            name: [0; 13],
            size: handle.size as u32,
            first_cluster,
            is_directory: false,
        };

        // Hot path: a full-file read from position 0 with a buffer at least as
        // large as the file. This is what `File::read_to_vec` and similar
        // read-everything callers do. Pass the caller's buffer straight to the
        // FAT cluster walker — no intermediate file-sized allocation, no
        // double-touch of every page.
        //
        // Before this branch, every `read()` allocated and zero-filled a temp
        // buffer the size of the entire file (rounded to cluster), then
        // memcpy'd into the caller — for a 5.79 MiB binary that meant ~1414
        // page faults to zero the temp PLUS ~1414 to copy into the caller.
        // For multi-MiB user binaries the cost dominated load time and
        // appeared as a hang under interactive load.
        if handle.position == 0 && buffer.len() >= handle.size as usize {
            return match self.inner.read_file(&fat_handle, buffer) {
                Ok(_) => Ok(handle.size as usize),
                Err(_) => Err(FilesystemError::IoError),
            };
        }

        // Fallback: partial read or short caller buffer — read the file into a
        // temp the size of the file, then copy out the requested slice. This
        // path is taken by callers that want a window into the middle of a
        // file or that pass a smaller buffer than the file size; the cost is
        // intrinsic to that interface (read_file requires buffer >= file.size)
        // and not exercised by the multi-MiB-binary load path.
        const ASSUMED_CLUSTER_SIZE: usize = 4096;
        let buffer_size = ((handle.size as usize + ASSUMED_CLUSTER_SIZE - 1)
            / ASSUMED_CLUSTER_SIZE)
            * ASSUMED_CLUSTER_SIZE;
        let mut file_buffer = alloc::vec![0u8; buffer_size];

        match self.inner.read_file(&fat_handle, &mut file_buffer) {
            Ok(_) => {
                let remaining = handle.size - handle.position;
                let to_copy = core::cmp::min(buffer.len(), remaining as usize);
                buffer[..to_copy].copy_from_slice(
                    &file_buffer
                        [handle.position as usize..handle.position as usize + to_copy],
                );
                Ok(to_copy)
            }
            Err(_) => Err(FilesystemError::IoError),
        }
    }
    
    fn write(&self, handle: &mut FileHandle, buffer: &[u8]) -> Result<usize, FilesystemError> {
        if !self.writable {
            return Err(FilesystemError::ReadOnly);
        }
        if !self.is_write_handle(handle.inode) {
            return Err(FilesystemError::PermissionDenied);
        }
        // Snapshot entry under the table lock, then release before
        // doing the actual I/O so we don't hold it across IDE writes.
        let (parent_cluster, leaf, first_cluster_in) = {
            let tbl = self.open_writes.lock();
            let entry = tbl.get(&handle.inode).ok_or(FilesystemError::IoError)?;
            (entry.parent_cluster, entry.leaf.clone(), entry.first_cluster)
        };
        let mut new_first_cluster = first_cluster_in;
        let bytes_written = self
            .inner
            .write_file_at(first_cluster_in, handle.position, buffer, &mut new_first_cluster)
            .map_err(map_fat_err)?;
        let new_size = (handle.position + bytes_written as u64).max(handle.size);
        // Update the SFN to reflect new size and (possibly new)
        // first_cluster.
        self.inner
            .update_sfn_size_and_cluster(parent_cluster, &leaf, new_first_cluster, new_size)
            .map_err(map_fat_err)?;
        // Re-sync side table.
        {
            let mut tbl = self.open_writes.lock();
            if let Some(entry) = tbl.get_mut(&handle.inode) {
                entry.first_cluster = new_first_cluster;
                entry.size = new_size;
            }
        }
        handle.position += bytes_written as u64;
        handle.size = new_size;
        Ok(bytes_written)
    }
    
    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        if position > handle.size {
            return Err(FilesystemError::InvalidPath);
        }
        
        handle.position = position;
        Ok(position)
    }
    
    fn mkdir(&self, _path: &str) -> Result<(), FilesystemError> {
        // mkdir on FAT is deferred to a follow-up — needs `.`/`..`
        // entry generation and parent-link wiring. Until then,
        // userland sees this as unsupported on /data (but tmpfs at /
        // still works).
        Err(FilesystemError::UnsupportedOperation)
    }

    fn unlink(&self, path: &str) -> Result<(), FilesystemError> {
        if !self.writable {
            return Err(FilesystemError::ReadOnly);
        }
        self.inner.unlink_file(path).map_err(map_fat_err)
    }

    fn rmdir(&self, _path: &str) -> Result<(), FilesystemError> {
        Err(FilesystemError::UnsupportedOperation)
    }

    fn rename(&self, _old_path: &str, _new_path: &str) -> Result<(), FilesystemError> {
        Err(FilesystemError::UnsupportedOperation)
    }

    fn sync(&self) -> Result<(), FilesystemError> {
        if self.writable {
            self.inner.sync_writes().map_err(map_fat_err)?;
        }
        Ok(())
    }
}

/// Translate FAT-layer errors to the public Filesystem error space.
fn map_fat_err(e: crate::fs::fat::types::FatError) -> FilesystemError {
    use crate::fs::fat::types::FatError as F;
    match e {
        F::NotFound => FilesystemError::NotFound,
        F::ReadOnly => FilesystemError::ReadOnly,
        F::InvalidPath => FilesystemError::InvalidPath,
        F::DiskFull => FilesystemError::DiskFull,
        F::BufferTooSmall => FilesystemError::BufferTooSmall,
        F::UnsupportedOperation => FilesystemError::UnsupportedOperation,
        _ => FilesystemError::IoError,
    }
}