use crate::fs::filesystem::{
    Filesystem, FilesystemError, FilesystemStats, DirectoryEntry, DirectoryIterator,
    FileHandle, FileMode, FileType, FileAttributes
};
use crate::fs::fat::filesystem::{FatFilesystem as FatFs, FileHandle as FatFileHandle};
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
    
    fn read_dir(&self, path: &str) -> Result<DirectoryIterator, FilesystemError> {
        // For now, only support root directory
        if path.is_empty() || path == "/" {
            Ok(DirectoryIterator::new(self, path))
        } else {
            Err(FilesystemError::NotFound)
        }
    }
    
    fn enumerate_dir(&self, path: &str) -> Result<alloc::vec::Vec<DirectoryEntry>, FilesystemError> {
        // Override the default implementation to directly access FAT directory entries
        if path.is_empty() || path == "/" {
            let mut entries = alloc::vec::Vec::new();
            
            // Use the FAT-specific list_root_array method
            let mut fat_files = [crate::fs::fat::filesystem::FileHandle {
                name: [0; 13],
                size: 0,
                first_cluster: crate::fs::fat::types::ClusterId(0),
                is_directory: false,
            }; 64]; // Buffer for up to 64 files
            
            match self.inner.list_root_array(&mut fat_files, 64) {
                Ok(count) => {
                    for i in 0..count {
                        let fat_file = &fat_files[i];
                        
                        // Convert FAT FileHandle to filesystem DirectoryEntry
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
                        
                        // Copy the name, trimming null bytes
                        let name_bytes = &fat_file.name;
                        let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
                        let copy_len = len.min(255);
                        entry.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
                        entry.name_len = copy_len;
                        
                        entries.push(entry);
                    }
                }
                Err(_) => {
                    return Err(FilesystemError::IoError);
                }
            }
            
            Ok(entries)
        } else {
            Err(FilesystemError::NotFound)
        }
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
        // The FAT filesystem read_file method only supports reading entire files,
        // so we need to allocate a temporary buffer for the whole file
        
        // Check if we're at EOF
        if handle.position >= handle.size {
            return Ok(0);
        }
        
        // Reconstruct the FAT file handle from the generic handle
        // The inode contains the first cluster number
        let fat_handle = crate::fs::fat::filesystem::FileHandle {
            name: [0; 13], // Name doesn't matter for reading
            size: handle.size as u32,
            first_cluster: crate::fs::fat::types::ClusterId(handle.inode as u32),
            is_directory: false,
        };
        
        // FAT filesystems use cluster sizes from 512 bytes to 32KB
        // Use a conservative 4KB (8 sectors) cluster size for buffer allocation
        const ASSUMED_CLUSTER_SIZE: usize = 4096;
        
        // Allocate buffer for entire file, rounded up to cluster boundary
        // This ensures read_file has enough buffer space for cluster-aligned reads
        let buffer_size = ((handle.size as usize + ASSUMED_CLUSTER_SIZE - 1) / ASSUMED_CLUSTER_SIZE) * ASSUMED_CLUSTER_SIZE;
        let mut file_buffer = alloc::vec![0u8; buffer_size];
        
        // Read the entire file
        match self.inner.read_file(&fat_handle, &mut file_buffer) {
            Ok(_) => {
                // Calculate how much to copy from the current position
                let remaining = handle.size - handle.position;
                let to_copy = core::cmp::min(buffer.len(), remaining as usize);
                
                // Copy the requested portion
                buffer[..to_copy].copy_from_slice(
                    &file_buffer[handle.position as usize..handle.position as usize + to_copy]
                );
                
                // Update position
                handle.position += to_copy as u64;
                
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