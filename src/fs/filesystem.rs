use crate::drivers::block::BlockDevice;
use core::fmt;

/// Common error types for filesystem operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemError {
    NotFound,
    PermissionDenied,
    InvalidPath,
    DiskFull,
    ReadOnly,
    Corrupted,
    UnsupportedOperation,
    IoError,
    InvalidFilesystem,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    NotEmpty,
    BufferTooSmall,
}

impl fmt::Display for FilesystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilesystemError::NotFound => write!(f, "File or directory not found"),
            FilesystemError::PermissionDenied => write!(f, "Permission denied"),
            FilesystemError::InvalidPath => write!(f, "Invalid path"),
            FilesystemError::DiskFull => write!(f, "Disk full"),
            FilesystemError::ReadOnly => write!(f, "Filesystem is read-only"),
            FilesystemError::Corrupted => write!(f, "Filesystem corrupted"),
            FilesystemError::UnsupportedOperation => write!(f, "Operation not supported"),
            FilesystemError::IoError => write!(f, "I/O error"),
            FilesystemError::InvalidFilesystem => write!(f, "Invalid filesystem"),
            FilesystemError::AlreadyExists => write!(f, "File or directory already exists"),
            FilesystemError::NotADirectory => write!(f, "Not a directory"),
            FilesystemError::IsADirectory => write!(f, "Is a directory"),
            FilesystemError::NotEmpty => write!(f, "Directory not empty"),
            FilesystemError::BufferTooSmall => write!(f, "Buffer too small"),
        }
    }
}

/// File types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    Device,
    Other,
}

/// File attributes
#[derive(Debug, Clone, Copy)]
pub struct FileAttributes {
    pub read_only: bool,
    pub hidden: bool,
    pub system: bool,
    pub archive: bool,
}

/// Directory entry information
#[derive(Clone)]
pub struct DirectoryEntry {
    pub name: [u8; 256],  // Max filename length
    pub name_len: usize,
    pub file_type: FileType,
    pub size: u64,
    pub attributes: FileAttributes,
    pub created: u64,      // Timestamp
    pub modified: u64,     // Timestamp
    pub accessed: u64,     // Timestamp
}

impl DirectoryEntry {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }
}

/// File handle for open files
pub struct FileHandle {
    pub inode: u64,
    pub position: u64,
    pub size: u64,
    pub mode: FileMode,
}

/// File open modes
#[derive(Debug, Clone, Copy)]
pub struct FileMode {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub create: bool,
    pub truncate: bool,
}

impl FileMode {
    pub const READ: Self = Self {
        read: true,
        write: false,
        append: false,
        create: false,
        truncate: false,
    };
    
    pub const WRITE: Self = Self {
        read: false,
        write: true,
        append: false,
        create: true,
        truncate: true,
    };
    
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        append: false,
        create: false,
        truncate: false,
    };
}

/// Filesystem statistics
#[derive(Debug, Clone, Copy)]
pub struct FilesystemStats {
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub block_size: u32,
    pub total_inodes: u64,
    pub free_inodes: u64,
}

/// Main filesystem trait that all filesystem implementations must implement
pub trait Filesystem {
    /// Get the name/type of this filesystem
    fn name(&self) -> &str;
    
    /// Check if this filesystem is read-only
    fn is_read_only(&self) -> bool;
    
    /// Get filesystem statistics
    fn stats(&self) -> Result<FilesystemStats, FilesystemError>;
    
    /// List directory contents
    fn read_dir(&self, path: &str) -> Result<DirectoryIterator, FilesystemError>;
    
    /// Enumerate directory entries into a Vec (convenience method)
    fn enumerate_dir(&self, path: &str) -> Result<alloc::vec::Vec<DirectoryEntry>, FilesystemError> {
        // Default implementation that tries to use read_dir
        // Individual filesystems can override this for better performance
        let mut entries = alloc::vec::Vec::new();
        let mut dir_iter = self.read_dir(path)?;
        
        while let Some(entry) = dir_iter.next_entry() {
            entries.push(entry);
        }
        
        Ok(entries)
    }
    
    /// Get file/directory metadata
    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError>;
    
    /// Open a file
    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError>;
    
    /// Close a file
    fn close(&self, handle: &mut FileHandle) -> Result<(), FilesystemError>;
    
    /// Read from a file
    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError>;
    
    /// Write to a file
    fn write(&self, handle: &mut FileHandle, buffer: &[u8]) -> Result<usize, FilesystemError>;
    
    /// Seek in a file
    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError>;
    
    /// Create a directory
    fn mkdir(&self, path: &str) -> Result<(), FilesystemError>;
    
    /// Remove a file
    fn unlink(&self, path: &str) -> Result<(), FilesystemError>;
    
    /// Remove a directory
    fn rmdir(&self, path: &str) -> Result<(), FilesystemError>;
    
    /// Rename a file or directory
    fn rename(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError>;
    
    /// Flush all pending writes
    fn sync(&self) -> Result<(), FilesystemError>;
}

/// Iterator over directory entries
pub struct DirectoryIterator<'a> {
    filesystem: &'a dyn Filesystem,
    path: [u8; 256],
    path_len: usize,
    index: usize,
    entries: alloc::vec::Vec<DirectoryEntry>,
    loaded: bool,
}

impl<'a> DirectoryIterator<'a> {
    pub fn new(filesystem: &'a dyn Filesystem, path: &str) -> Self {
        let mut path_buf = [0u8; 256];
        let path_bytes = path.as_bytes();
        let len = path_bytes.len().min(256);
        path_buf[..len].copy_from_slice(&path_bytes[..len]);
        
        Self {
            filesystem,
            path: path_buf,
            path_len: len,
            index: 0,
            entries: alloc::vec::Vec::new(),
            loaded: false,
        }
    }
    
    /// Get the path as a string slice
    pub fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
    
    /// Load directory entries from the filesystem
    fn load_entries(&mut self) {
        if self.loaded {
            return;
        }
        
        // For now, we'll use a workaround since we can't easily access the FAT-specific
        // directory iteration from the generic filesystem trait.
        // This is a bridge implementation that could be improved with better filesystem abstractions.
        
        // We can't directly access filesystem-specific directory reading here since
        // the trait doesn't expose it. For a complete implementation, we'd need to
        // either:
        // 1. Add a method to the Filesystem trait to enumerate entries
        // 2. Use filesystem-specific casting 
        // 3. Store entries when the DirectoryIterator is created
        
        // For now, this will remain empty, but the framework is in place
        self.loaded = true;
    }
    
    /// Get the next directory entry
    pub fn next_entry(&mut self) -> Option<DirectoryEntry> {
        self.load_entries();
        
        if self.index < self.entries.len() {
            let entry = self.entries[self.index].clone();
            self.index += 1;
            Some(entry)
        } else {
            None
        }
    }
}

impl<'a> Iterator for DirectoryIterator<'a> {
    type Item = DirectoryEntry;
    
    fn next(&mut self) -> Option<Self::Item> {
        self.next_entry()
    }
}

/// Filesystem type detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemType {
    Fat12,
    Fat16,
    Fat32,
    Ext2,
    Ext3,
    Ext4,
    Ntfs,
    Unknown,
}

/// Detect filesystem type from a block device (or partition)
pub fn detect_filesystem(device: &dyn BlockDevice) -> Result<FilesystemType, FilesystemError> {
    let mut buffer = [0u8; 512];
    
    // Read the first sector (boot sector/superblock)
    device.read_blocks(0, 1, &mut buffer)
        .map_err(|_| FilesystemError::IoError)?;
    
    // Check for FAT filesystem signatures
    if buffer[510] == 0x55 && buffer[511] == 0xAA {
        // Valid boot sector signature, might be FAT
        
        // Check for FAT32 signature
        if &buffer[82..87] == b"FAT32" {
            return Ok(FilesystemType::Fat32);
        }
        
        // Check for FAT12/16 signature
        if &buffer[54..59] == b"FAT12" {
            return Ok(FilesystemType::Fat12);
        }
        if &buffer[54..59] == b"FAT16" {
            return Ok(FilesystemType::Fat16);
        }
        
        // Additional FAT detection based on cluster count
        // This would require parsing the BPB, which we can do if needed
    }
    
    // Check for ext2/3/4 filesystem
    // Ext superblock starts at offset 1024 (block 1 for 1K blocks, or within block 0 for larger blocks)
    let mut ext_buffer = [0u8; 512];
    device.read_blocks(2, 1, &mut ext_buffer)  // Read sectors 2-3 (bytes 1024-1535)
        .map_err(|_| FilesystemError::IoError)?;
    
    // Check ext2/3/4 magic number at offset 56 of superblock (0x438 from start of partition)
    if ext_buffer[56] == 0x53 && ext_buffer[57] == 0xEF {
        // This is an ext2/3/4 filesystem
        // We could further distinguish between them by checking features
        return Ok(FilesystemType::Ext2);  // For now, just return Ext2
    }
    
    // Check for NTFS
    if &buffer[3..7] == b"NTFS" {
        return Ok(FilesystemType::Ntfs);
    }
    
    Ok(FilesystemType::Unknown)
}