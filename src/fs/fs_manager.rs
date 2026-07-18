
use crate::fs::vfs::vfs_stat;
use crate::fs::filesystem::DirectoryEntry;
use core::fmt;

pub struct FileSystemManager;

#[derive(Debug)]
pub enum FsError {
    FileNotFound,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    BufferTooSmall,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    IoError,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    NotImplemented,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    InvalidPath,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    NotAFile,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    NotADirectory,
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::FileNotFound => write!(f, "File not found"),
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

    pub fn exists(path: &str) -> bool {
        vfs_stat(path).is_ok()
    }

    pub fn metadata(path: &str) -> FsResult<DirectoryEntry> {
        vfs_stat(path)
            .map_err(|_| FsError::FileNotFound)
    }



    }

#[expect(dead_code, reason = "intentional kernel API surface")]
pub struct Path<'a> {
    path: &'a str,
}

impl<'a> Path<'a> {





    }

#[expect(dead_code, reason = "intentional kernel API surface")]
pub struct PathComponents<'a> {
    path: &'a str,
    position: usize,
}

impl<'a> PathComponents<'a> {
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






pub fn exists(path: &str) -> bool {
    FileSystemManager::exists(path)
}

pub fn metadata(path: &str) -> FsResult<DirectoryEntry> {
    FileSystemManager::metadata(path)
}
