
use crate::fs::vfs::{VirtualFilesystem, get_vfs, vfs_open, vfs_read_dir, vfs_stat};
use crate::fs::filesystem::{FileHandle as FsFileHandle, FileMode, DirectoryEntry, FilesystemError};
use crate::drivers::block::BlockDevice;
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
        read_with(path, |handle, fs| {
            let mut buffer = [0u8; N];
            let bytes_read = fs.read(handle, &mut buffer)
                .map_err(|_| FsError::IoError)?;
            Ok((buffer, bytes_read))
        })
    }

    pub fn read_to_string<const N: usize>(path: &str) -> FsResult<(&'static str, usize)> {
        static mut BUFFER: [u8; 4096] = [0u8; 4096];
        
        read_with(path, |handle, fs| {
            let bytes_read = unsafe {
                fs.read(handle, &mut BUFFER[..N.min(4096)])
                    .map_err(|_| FsError::IoError)?
            };
            
            unsafe {
                let s = core::str::from_utf8(&BUFFER[..bytes_read])
                    .map_err(|_| FsError::IoError)?;
                Ok((core::mem::transmute(s), bytes_read))
            }
        })
    }

    pub fn write(_path: &str, _contents: &[u8]) -> FsResult<()> {
        Err(FsError::NotImplemented)
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

pub fn read_to_string<const N: usize>(path: &str) -> FsResult<(&'static str, usize)> {
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

/// Read the entire contents of a file into a static buffer
/// Returns the buffer and the number of bytes read
pub fn read_entire_file(path: &str) -> FsResult<(&'static [u8], usize)> {
    const BUFFER_SIZE: usize = 8192;
    static mut FILE_BUFFER: [u8; BUFFER_SIZE] = [0u8; BUFFER_SIZE];
    
    read_with(path, |handle, fs| {
        let mut total_read = 0;
        unsafe {
            loop {
                let bytes_read = fs.read(handle, &mut FILE_BUFFER[total_read..])
                    .map_err(|_| FsError::IoError)?;
                if bytes_read == 0 {
                    break;
                }
                total_read += bytes_read;
                if total_read >= BUFFER_SIZE {
                    break;
                }
            }
            Ok((&FILE_BUFFER[..total_read], total_read))
        }
    })
}

/// Process a file line by line using a callback
pub fn for_each_line<F>(path: &str, mut f: F) -> FsResult<()>
where
    F: FnMut(&str) -> FsResult<()>,
{
    read_with(path, |handle, fs| {
        let mut buffer = [0u8; 512];
        let mut line_buffer = [0u8; 256];
        let mut line_pos = 0;
        let mut buffer_pos = 0;
        let mut buffer_len = 0;
        
        loop {
            // Refill buffer if needed
            if buffer_pos >= buffer_len {
                buffer_len = fs.read(handle, &mut buffer)
                    .map_err(|_| FsError::IoError)?;
                buffer_pos = 0;
                
                if buffer_len == 0 {
                    // Process final line if any
                    if line_pos > 0 {
                        let line = core::str::from_utf8(&line_buffer[..line_pos])
                            .map_err(|_| FsError::IoError)?;
                        f(line)?;
                    }
                    break;
                }
            }
            
            // Process bytes from buffer
            while buffer_pos < buffer_len && line_pos < line_buffer.len() {
                let byte = buffer[buffer_pos];
                buffer_pos += 1;
                
                if byte == b'\n' {
                    let line = core::str::from_utf8(&line_buffer[..line_pos])
                        .map_err(|_| FsError::IoError)?;
                    f(line)?;
                    line_pos = 0;
                } else if byte != b'\r' {
                    line_buffer[line_pos] = byte;
                    line_pos += 1;
                }
            }
            
            if line_pos >= line_buffer.len() {
                return Err(FsError::BufferTooSmall);
            }
        }
        
        Ok(())
    })
}