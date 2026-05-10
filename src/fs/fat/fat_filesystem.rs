use crate::fs::filesystem::{
    Filesystem, FilesystemError, FilesystemStats, DirectoryEntry, DirectoryIterator,
    FileHandle, FileMode, FileType, FileAttributes
};
use crate::fs::fat::filesystem::FatFilesystem as FatFs;
use crate::fs::fat::types::FatType;

/// Wrapper to implement the Filesystem trait for FAT
pub struct FatFilesystemWrapper<'a> {
    inner: FatFs<'a>,
}

impl<'a> FatFilesystemWrapper<'a> {
    pub fn new(inner: FatFs<'a>) -> Self {
        Self { inner }
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
        // For now, FAT is read-only
        true
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
        // walk the directory entries. Works uniformly for the root
        // and any nested subdirectory.
        let cluster = self
            .inner
            .resolve_directory(path)
            .map_err(|_| FilesystemError::NotFound)?;

        let mut entries = alloc::vec::Vec::new();
        let mut fat_files = [crate::fs::fat::filesystem::FileHandle {
            name: [0; 13],
            size: 0,
            first_cluster: crate::fs::fat::types::ClusterId(0),
            is_directory: false,
        }; 64];

        let count = self
            .inner
            .list_directory(cluster, &mut fat_files, 64)
            .map_err(|_| FilesystemError::IoError)?;

        for fat_file in fat_files.iter().take(count) {
            let name_bytes = &fat_file.name;
            let len = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
            // Skip the FAT "." and ".." entries — userland gets enough
            // of those by walking via the kernel's normalizer.
            let copy_len = len.min(255);
            let name_slice = &name_bytes[..copy_len];
            if name_slice == b"." || name_slice == b".." {
                continue;
            }

            let mut entry = DirectoryEntry {
                name: [0u8; 256],
                name_len: 0,
                file_type: if fat_file.is_directory {
                    crate::fs::filesystem::FileType::Directory
                } else {
                    crate::fs::filesystem::FileType::File
                },
                size: fat_file.size as u64,
                attributes: crate::fs::filesystem::FileAttributes {
                    read_only: false,
                    hidden: false,
                    system: false,
                    archive: false,
                },
                created: 0,
                modified: 0,
                accessed: 0,
            };
            entry.name[..copy_len].copy_from_slice(name_slice);
            entry.name_len = copy_len;
            entries.push(entry);
        }

        Ok(entries)
    }
    
    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        // Try to find the file
        match self.inner.find_file(path) {
            Ok(fat_file) => {
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
                
                // Copy name
                let name_bytes = &fat_file.name;
                let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
                entry.name[..len].copy_from_slice(&name_bytes[..len]);
                entry.name_len = len;
                
                Ok(entry)
            }
            Err(_) => Err(FilesystemError::NotFound),
        }
    }
    
    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError> {
        if mode.write || mode.create {
            return Err(FilesystemError::ReadOnly);
        }
        
        match self.inner.find_file(path) {
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
        }
    }
    
    fn close(&self, _handle: &mut FileHandle) -> Result<(), FilesystemError> {
        // Nothing to do for FAT
        Ok(())
    }
    
    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError> {
        if handle.position >= handle.size {
            return Ok(0);
        }

        // Reconstruct the FAT file handle from the generic handle.
        // The inode contains the first cluster number.
        let fat_handle = crate::fs::fat::filesystem::FileHandle {
            name: [0; 13],
            size: handle.size as u32,
            first_cluster: crate::fs::fat::types::ClusterId(handle.inode as u32),
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
    
    fn write(&self, _handle: &mut FileHandle, _buffer: &[u8]) -> Result<usize, FilesystemError> {
        Err(FilesystemError::ReadOnly)
    }
    
    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        if position > handle.size {
            return Err(FilesystemError::InvalidPath);
        }
        
        handle.position = position;
        Ok(position)
    }
    
    fn mkdir(&self, _path: &str) -> Result<(), FilesystemError> {
        Err(FilesystemError::ReadOnly)
    }
    
    fn unlink(&self, _path: &str) -> Result<(), FilesystemError> {
        Err(FilesystemError::ReadOnly)
    }
    
    fn rmdir(&self, _path: &str) -> Result<(), FilesystemError> {
        Err(FilesystemError::ReadOnly)
    }
    
    fn rename(&self, _old_path: &str, _new_path: &str) -> Result<(), FilesystemError> {
        Err(FilesystemError::ReadOnly)
    }
    
    fn sync(&self) -> Result<(), FilesystemError> {
        // Nothing to sync for read-only filesystem
        Ok(())
    }
}