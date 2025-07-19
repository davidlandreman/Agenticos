
use crate::fs::vfs::{VirtualFilesystem, get_vfs, vfs_open, vfs_read_dir, vfs_stat};
use crate::fs::filesystem::{FileHandle as FsFileHandle, FileMode, DirectoryEntry, FilesystemError};
use crate::fs::file_handle::{File, FileError, FileResult};
use crate::lib::arc::Arc;
use crate::drivers::block::BlockDevice;
use alloc::{string::{String, ToString}, vec::Vec};
use core::fmt;

pub struct FileSystemManager;

#[derive(Debug)]
pub enum FsError {
    FileNotFound,
    AccessDenied,
    BufferTooSmall,
    IoError,
    NotImplemented,
    InvalidPath,
    NotAFile,
    NotADirectory,
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::FileNotFound => write!(f, "File not found"),
            FsError::AccessDenied => write!(f, "Access denied"),
            FsError::BufferTooSmall => write!(f, "Buffer too small"),
            FsError::IoError => write!(f, "I/O error"),
            FsError::NotImplemented => write!(f, "Not implemented"),
            FsError::InvalidPath => write!(f, "Invalid path"),
            FsError::NotAFile => write!(f, "Not a file"),
            FsError::NotADirectory => write!(f, "Not a directory"),
        }
    }
}

pub type FsResult<T> = Result<T, FsError>;

impl FileSystemManager {
    pub fn new() -> Self {
        FileSystemManager
    }

    pub fn read<const N: usize>(path: &str) -> FsResult<([u8; N], usize)> {
        match File::open_read(path) {
            Ok(file) => {
                let mut buffer = [0u8; N];
                match file.read(&mut buffer) {
                    Ok(bytes_read) => Ok((buffer, bytes_read)),
                    Err(_) => Err(FsError::IoError),
                }
            }
            Err(_) => Err(FsError::FileNotFound),
        }
    }

    pub fn read_to_string<const N: usize>(path: &str) -> FsResult<(String, usize)> {
        match File::open_read(path) {
            Ok(file) => {
                match file.read_to_string() {
                    Ok(content) => {
                        let len = content.len();
                        let truncated = if content.len() > N {
                            content[..N].to_string()
                        } else {
                            content
                        };
                        Ok((truncated, len))
                    }
                    Err(_) => Err(FsError::IoError),
                }
            }
            Err(_) => Err(FsError::FileNotFound),
        }
    }

    pub fn write(path: &str, contents: &[u8]) -> FsResult<()> {
        match File::open_write(path) {
            Ok(file) => {
                match file.write(contents) {
                    Ok(_) => Ok(()),
                    Err(_) => Err(FsError::IoError),
                }
            }
            Err(_) => Err(FsError::AccessDenied),
        }
    }

    pub fn exists(path: &str) -> bool {
        vfs_stat(path).is_ok()
    }

    pub fn metadata(path: &str) -> FsResult<DirectoryEntry> {
        vfs_stat(path)
            .map_err(|_| FsError::FileNotFound)
    }

    pub fn read_dir(path: &'static str) -> FsResult<crate::fs::filesystem::DirectoryIterator<'static>> {
        vfs_read_dir(path)
            .map_err(|_| FsError::IoError)
    }

    pub fn is_file(path: &str) -> FsResult<bool> {
        let metadata = Self::metadata(path)?;
        Ok(metadata.file_type == crate::fs::filesystem::FileType::File)
    }

    pub fn is_dir(path: &str) -> FsResult<bool> {
        let metadata = Self::metadata(path)?;
        Ok(metadata.file_type == crate::fs::filesystem::FileType::Directory)
    }

    pub fn mount_disk(device: &'static dyn BlockDevice, filesystem: &'static dyn crate::fs::filesystem::Filesystem, mount_point: &'static str) -> FsResult<()> {
        let vfs = get_vfs();
        vfs.mount(mount_point, filesystem, device)
            .map_err(|_| FsError::IoError)
    }
}

pub struct Path<'a> {
    path: &'a str,
}

impl<'a> Path<'a> {
    pub fn new(path: &'a str) -> Self {
        Path { path }
    }

    pub fn as_str(&self) -> &str {
        self.path
    }

    pub fn file_name(&self) -> Option<&str> {
        self.path.rfind('/').map(|pos| &self.path[pos + 1..])
    }

    pub fn parent(&self) -> Option<Path<'a>> {
        self.path.rfind('/').and_then(|pos| {
            if pos == 0 {
                Some(Path::new("/"))
            } else {
                Some(Path::new(&self.path[..pos]))
            }
        })
    }

    pub fn is_absolute(&self) -> bool {
        self.path.starts_with('/')
    }

    pub fn components(&self) -> PathComponents<'a> {
        PathComponents::new(self.path)
    }
}

pub struct PathComponents<'a> {
    path: &'a str,
    position: usize,
}

impl<'a> PathComponents<'a> {
    fn new(path: &'a str) -> Self {
        let position = if path.starts_with('/') { 1 } else { 0 };
        PathComponents { path, position }
    }
}

impl<'a> Iterator for PathComponents<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.path.len() {
            return None;
        }

        let remaining = &self.path[self.position..];
        if let Some(slash_pos) = remaining.find('/') {
            let component = &remaining[..slash_pos];
            self.position += slash_pos + 1;
            Some(component)
        } else {
            self.position = self.path.len();
            Some(remaining)
        }
    }
}

/// Execute a closure with an open file handle
/// This avoids lifetime issues by ensuring the file handle only exists
/// within the scope of the closure
pub fn with_file<F, R>(path: &str, mode: FileMode, f: F) -> FsResult<R>
where
    F: FnOnce(&mut FsFileHandle, &dyn crate::fs::filesystem::Filesystem) -> FsResult<R>,
{
    let mut handle = vfs_open(path, mode)
        .map_err(|_| FsError::FileNotFound)?;
    
    // Get the filesystem for this path
    let vfs = get_vfs();
    if let Some((fs, _)) = vfs.find_filesystem(path) {
        f(&mut handle, fs)
    } else {
        Err(FsError::FileNotFound)
    }
}

/// Read an entire file into a buffer using a callback
pub fn read_with<F, R>(path: &str, f: F) -> FsResult<R>
where
    F: FnOnce(&mut FsFileHandle, &dyn crate::fs::filesystem::Filesystem) -> FsResult<R>,
{
    with_file(path, FileMode::READ, f)
}

/// Write to a file using a callback
pub fn write_with<F, R>(path: &str, f: F) -> FsResult<R>
where
    F: FnOnce(&mut FsFileHandle, &dyn crate::fs::filesystem::Filesystem) -> FsResult<R>,
{
    with_file(path, FileMode::WRITE, f)
}

pub fn read<const N: usize>(path: &str) -> FsResult<([u8; N], usize)> {
    FileSystemManager::read(path)
}

pub fn read_to_string<const N: usize>(path: &str) -> FsResult<(String, usize)> {
    FileSystemManager::read_to_string::<N>(path)
}

pub fn write(path: &str, contents: &[u8]) -> FsResult<()> {
    FileSystemManager::write(path, contents)
}

pub fn exists(path: &str) -> bool {
    FileSystemManager::exists(path)
}

pub fn metadata(path: &str) -> FsResult<DirectoryEntry> {
    FileSystemManager::metadata(path)
}

pub fn read_dir(path: &'static str) -> FsResult<crate::fs::filesystem::DirectoryIterator<'static>> {
    FileSystemManager::read_dir(path)
}

/// Read the entire contents of a file into a Vec<u8>
/// Returns the buffer and the number of bytes read
pub fn read_entire_file(path: &str) -> FsResult<(Vec<u8>, usize)> {
    match File::open_read(path) {
        Ok(file) => {
            match file.read_to_vec() {
                Ok(content) => {
                    let len = content.len();
                    Ok((content, len))
                }
                Err(_) => Err(FsError::IoError),
            }
        }
        Err(_) => Err(FsError::FileNotFound),
    }
}

/// Process a file line by line using a callback
pub fn for_each_line<F>(path: &str, mut f: F) -> FsResult<()>
where
    F: FnMut(&str) -> FsResult<()>,
{
    match File::open_read(path) {
        Ok(file) => {
            match file.read_to_string() {
                Ok(content) => {
                    for line in content.lines() {
                        f(line)?;
                    }
                    Ok(())
                }
                Err(_) => Err(FsError::IoError),
            }
        }
        Err(_) => Err(FsError::FileNotFound),
    }
}

/// Create a new file with Arc-based handle
pub fn create_file(path: &str) -> FsResult<Arc<File>> {
    File::create(path)
        .map_err(|_| FsError::AccessDenied)
}

/// Open a file for reading with Arc-based handle
pub fn open_file_read(path: &str) -> FsResult<Arc<File>> {
    File::open_read(path)
        .map_err(|_| FsError::FileNotFound)
}

/// Open a file for writing with Arc-based handle
pub fn open_file_write(path: &str) -> FsResult<Arc<File>> {
    File::open_write(path)
        .map_err(|_| FsError::AccessDenied)
}

/// Write a string to a file (convenience function)
pub fn write_string(path: &str, content: &str) -> FsResult<()> {
    match File::open_write(path) {
        Ok(file) => {
            match file.write_string(content) {
                Ok(_) => Ok(()),
                Err(_) => Err(FsError::IoError),
            }
        }
        Err(_) => Err(FsError::AccessDenied),
    }
}

/// Read entire file contents as a string (convenience function)
pub fn read_file_to_string(path: &str) -> FsResult<String> {
    match File::open_read(path) {
        Ok(file) => {
            match file.read_to_string() {
                Ok(content) => Ok(content),
                Err(_) => Err(FsError::IoError),
            }
        }
        Err(_) => Err(FsError::FileNotFound),
    }
}