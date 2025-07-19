use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    InvalidBootSector,
    InvalidFatType,
    InvalidCluster,
    EndOfChain,
    BadCluster,
    NotFound,
    DiskFull,
    ReadOnly,
    InvalidPath,
    BlockDeviceError,
    BufferTooSmall,
    InvalidDirectoryEntry,
    UnsupportedOperation,
}

impl fmt::Display for FatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FatError::InvalidBootSector => write!(f, "Invalid boot sector"),
            FatError::InvalidFatType => write!(f, "Invalid FAT type"),
            FatError::InvalidCluster => write!(f, "Invalid cluster"),
            FatError::EndOfChain => write!(f, "End of cluster chain"),
            FatError::BadCluster => write!(f, "Bad cluster"),
            FatError::NotFound => write!(f, "File or directory not found"),
            FatError::DiskFull => write!(f, "Disk full"),
            FatError::ReadOnly => write!(f, "Filesystem is read-only"),
            FatError::InvalidPath => write!(f, "Invalid path"),
            FatError::BlockDeviceError => write!(f, "Block device error"),
            FatError::BufferTooSmall => write!(f, "Buffer too small"),
            FatError::InvalidDirectoryEntry => write!(f, "Invalid directory entry"),
            FatError::UnsupportedOperation => write!(f, "Unsupported operation"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileAttributes(pub u8);

impl FileAttributes {
    pub const READ_ONLY: u8 = 0x01;
    pub const HIDDEN: u8 = 0x02;
    pub const SYSTEM: u8 = 0x04;
    pub const VOLUME_ID: u8 = 0x08;
    pub const DIRECTORY: u8 = 0x10;
    pub const ARCHIVE: u8 = 0x20;
    pub const LFN: u8 = Self::READ_ONLY | Self::HIDDEN | Self::SYSTEM | Self::VOLUME_ID;

    pub fn new(value: u8) -> Self {
        Self(value)
    }

    pub fn is_read_only(&self) -> bool {
        self.0 & Self::READ_ONLY != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.0 & Self::HIDDEN != 0
    }

    pub fn is_system(&self) -> bool {
        self.0 & Self::SYSTEM != 0
    }

    pub fn is_volume_id(&self) -> bool {
        self.0 & Self::VOLUME_ID != 0
    }

    pub fn is_directory(&self) -> bool {
        self.0 & Self::DIRECTORY != 0
    }

    pub fn is_archive(&self) -> bool {
        self.0 & Self::ARCHIVE != 0
    }

    pub fn is_lfn(&self) -> bool {
        self.0 & 0x0F == Self::LFN
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClusterId(pub u32);

impl ClusterId {
    pub const INVALID: Self = Self(0);
    pub const ROOT_FAT16: Self = Self(0);
    
    pub fn is_valid(&self, fat_type: FatType) -> bool {
        match fat_type {
            FatType::Fat12 => self.0 >= 2 && self.0 < 0xFF7,
            FatType::Fat16 => self.0 >= 2 && self.0 < 0xFFF7,
            FatType::Fat32 => self.0 >= 2 && self.0 < 0x0FFFFFF7,
        }
    }
    
    pub fn is_end_of_chain(&self, fat_type: FatType) -> bool {
        match fat_type {
            FatType::Fat12 => self.0 >= 0xFF8,
            FatType::Fat16 => self.0 >= 0xFFF8,
            FatType::Fat32 => self.0 >= 0x0FFFFFF8,
        }
    }
    
    pub fn is_bad(&self, fat_type: FatType) -> bool {
        match fat_type {
            FatType::Fat12 => self.0 == 0xFF7,
            FatType::Fat16 => self.0 == 0xFFF7,
            FatType::Fat32 => self.0 == 0x0FFFFFF7,
        }
    }
}