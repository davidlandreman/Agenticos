use crate::fs::filesystem::{Filesystem, FilesystemType, FilesystemError, detect_filesystem};
use crate::fs::fat::FatFilesystem;
use crate::drivers::block::BlockDevice;
use crate::{debug_info, debug_error};

// Static storage for mounted filesystems
// In a real OS, these would be dynamically allocated
static mut MOUNTED_FAT_FS: Option<FatFilesystem<'static>> = None;
static mut MOUNTED_FAT_WRAPPER: Option<crate::fs::fat::fat_filesystem::FatFilesystemWrapper<'static>> = None;

/// Mount point information
#[derive(Clone, Copy)]
pub struct MountPoint {
    pub path: &'static str,
    pub filesystem: &'static dyn Filesystem,
    pub device: &'static dyn BlockDevice,
}

/// Virtual Filesystem Manager
pub struct VirtualFilesystem {
    mounts: [Option<MountPoint>; 16],  // Support up to 16 mount points
    mount_count: usize,
}

impl VirtualFilesystem {
    pub const fn new() -> Self {
        Self {
            mounts: [None; 16],
            mount_count: 0,
        }
    }
    
    /// Mount a filesystem at the given path
    pub fn mount(&mut self, path: &'static str, filesystem: &'static dyn Filesystem, device: &'static dyn BlockDevice) -> Result<(), FilesystemError> {
        if self.mount_count >= self.mounts.len() {
            return Err(FilesystemError::DiskFull);  // No more mount points available
        }
        
        // Check if path is already mounted
        for mount in self.mounts.iter().flatten() {
            if mount.path == path {
                return Err(FilesystemError::AlreadyExists);
            }
        }
        
        self.mounts[self.mount_count] = Some(MountPoint {
            path,
            filesystem,
            device,
        });
        self.mount_count += 1;
        
        debug_info!("Mounted {} filesystem at {}", filesystem.name(), path);
        Ok(())
    }
    
    /// Unmount a filesystem
    pub fn unmount(&mut self, path: &str) -> Result<(), FilesystemError> {
        for i in 0..self.mount_count {
            if let Some(mount) = &self.mounts[i] {
                if mount.path == path {
                    // Shift remaining mounts down
                    for j in i..self.mount_count - 1 {
                        self.mounts[j] = self.mounts[j + 1].take();
                    }
                    self.mounts[self.mount_count - 1] = None;
                    self.mount_count -= 1;
                    
                    debug_info!("Unmounted filesystem at {}", path);
                    return Ok(());
                }
            }
        }
        
        Err(FilesystemError::NotFound)
    }
    
    /// Find the filesystem for a given path
    pub fn find_filesystem<'a>(&'a self, path: &'a str) -> Option<(&'a dyn Filesystem, &'a str)> {
        let mut best_match: Option<(&MountPoint, usize)> = None;
        
        // Find the longest matching mount point
        for mount in self.mounts.iter().flatten() {
            if path.starts_with(mount.path) {
                let mount_len = mount.path.len();
                if best_match.is_none() || mount_len > best_match.unwrap().1 {
                    best_match = Some((mount, mount_len));
                }
            }
        }
        
        best_match.map(|(mount, len)| {
            let relative_path = if path.len() > len && path.as_bytes()[len] == b'/' {
                &path[len + 1..]
            } else if path.len() == len {
                ""
            } else {
                &path[len..]
            };
            (mount.filesystem, relative_path)
        })
    }
    
    /// List all mount points
    pub fn list_mounts(&self) -> impl Iterator<Item = &MountPoint> {
        self.mounts.iter().flatten()
    }
}

/// Global VFS instance
static mut VFS: VirtualFilesystem = VirtualFilesystem::new();

/// Get the global VFS instance
pub fn get_vfs() -> &'static mut VirtualFilesystem {
    unsafe { &mut VFS }
}

/// Auto-mount a block device by detecting its filesystem type
pub fn auto_mount(device: &'static dyn BlockDevice, mount_path: &'static str) -> Result<FilesystemType, FilesystemError> {
    let fs_type = detect_filesystem(device)?;
    
    debug_info!("Detected filesystem type: {:?}", fs_type);
    
    match fs_type {
        FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32 => {
            unsafe {
                // Try to create FAT filesystem with 'static lifetime
                match FatFilesystem::new(device) {
                    Ok(fat_fs) => {
                        // Transmute to 'static lifetime - this is safe because the device is 'static
                        let fat_fs_static: FatFilesystem<'static> = core::mem::transmute(fat_fs);
                        
                        // Create the wrapper
                        let wrapper = crate::fs::fat::fat_filesystem::FatFilesystemWrapper::new(fat_fs_static);
                        MOUNTED_FAT_WRAPPER = Some(wrapper);
                        
                        // Mount using VFS
                        if let Some(wrapper_ref) = MOUNTED_FAT_WRAPPER.as_ref() {
                                let vfs = get_vfs();
                                match vfs.mount(mount_path, wrapper_ref as &dyn Filesystem, device) {
                                    Ok(_) => {
                                        debug_info!("Successfully mounted FAT filesystem at {}", mount_path);
                                        Ok(fs_type)
                                    }
                                    Err(e) => {
                                        debug_error!("Failed to mount FAT filesystem: {:?}", e);
                                        Err(e)
                                    }
                                }
                        } else {
                            debug_error!("Failed to create wrapper reference");
                            Err(FilesystemError::InvalidFilesystem)
                        }
                    }
                    Err(_) => {
                        debug_error!("Failed to initialize FAT filesystem");
                        Err(FilesystemError::InvalidFilesystem)
                    }
                }
            }
        }
        FilesystemType::Ext2 | FilesystemType::Ext3 | FilesystemType::Ext4 => {
            debug_info!("Ext filesystem support not yet implemented");
            Err(FilesystemError::UnsupportedOperation)
        }
        FilesystemType::Ntfs => {
            debug_info!("NTFS filesystem support not yet implemented");
            Err(FilesystemError::UnsupportedOperation)
        }
        FilesystemType::Unknown => {
            debug_error!("Unknown filesystem type");
            Err(FilesystemError::InvalidFilesystem)
        }
    }
}

/// Convenience functions that operate on the global VFS

pub fn vfs_open(path: &str, mode: crate::fs::filesystem::FileMode) -> Result<crate::fs::filesystem::FileHandle, FilesystemError> {
    let vfs = get_vfs();
    if let Some((fs, rel_path)) = vfs.find_filesystem(path) {
        fs.open(rel_path, mode)
    } else {
        Err(FilesystemError::NotFound)
    }
}

pub fn vfs_read_dir(path: &str) -> Result<crate::fs::filesystem::DirectoryIterator, FilesystemError> {
    let vfs = get_vfs();
    if let Some((fs, rel_path)) = vfs.find_filesystem(path) {
        fs.read_dir(rel_path)
    } else {
        Err(FilesystemError::NotFound)
    }
}

pub fn vfs_stat(path: &str) -> Result<crate::fs::filesystem::DirectoryEntry, FilesystemError> {
    let vfs = get_vfs();
    if let Some((fs, rel_path)) = vfs.find_filesystem(path) {
        fs.stat(rel_path)
    } else {
        Err(FilesystemError::NotFound)
    }
}