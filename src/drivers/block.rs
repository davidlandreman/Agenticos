//! Block device abstraction for storage devices

/// Trait for block devices (storage devices that read/write in blocks)
pub trait BlockDevice {
    /// Read blocks from the device
    /// 
    /// # Arguments
    /// * `block` - Starting block number (LBA)
    /// * `count` - Number of blocks to read
    /// * `buffer` - Buffer to read data into (must be at least count * block_size bytes)
    fn read_blocks(&self, block: u64, count: u32, buffer: &mut [u8]) -> Result<(), &'static str>;

    /// Write blocks to the device
    /// 
    /// # Arguments
    /// * `block` - Starting block number (LBA)
    /// * `count` - Number of blocks to write
    /// * `buffer` - Buffer containing data to write (must be at least count * block_size bytes)
    fn write_blocks(&self, block: u64, count: u32, buffer: &[u8]) -> Result<(), &'static str>;

    /// Get the block size in bytes (typically 512 for hard drives)
    fn block_size(&self) -> u32;

    /// Get the total number of blocks on the device
    fn total_blocks(&self) -> u64;

    /// Get the total capacity in bytes
    fn capacity(&self) -> u64 {
        self.total_blocks() * self.block_size() as u64
    }

    /// Check if the device is read-only
    fn is_read_only(&self) -> bool {
        false
    }

    /// Get a human-readable name for the device
    fn name(&self) -> &str;

    /// Flush any pending writes to the device
    fn flush(&self) -> Result<(), &'static str> {
        Ok(()) // Default implementation does nothing
    }
}

/// Error type for block device operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDeviceError {
    /// The requested block is out of range
    InvalidBlock,
    /// The buffer is too small for the operation
    BufferTooSmall,
    /// The device is not present or not ready
    DeviceNotReady,
    /// An I/O error occurred
    IoError,
    /// The device is read-only
    ReadOnly,
    /// Operation not supported by this device
    NotSupported,
}