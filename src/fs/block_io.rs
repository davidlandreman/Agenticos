//! Checked byte and filesystem-block I/O over sector-addressed devices.

use crate::drivers::block::BlockDevice;
use crate::fs::filesystem::FilesystemError;

pub struct BlockIo<'a> {
    device: &'a dyn BlockDevice,
    fs_block_size: u32,
}

impl<'a> BlockIo<'a> {
    pub fn new(device: &'a dyn BlockDevice, fs_block_size: u32) -> Result<Self, FilesystemError> {
        let sector = device.block_size();
        if sector == 0
            || fs_block_size < sector
            || !fs_block_size.is_power_of_two()
            || fs_block_size % sector != 0
        {
            return Err(FilesystemError::InvalidFilesystem);
        }
        Ok(Self {
            device,
            fs_block_size,
        })
    }

    pub fn fs_block_count(&self) -> u64 {
        self.device.capacity() / self.fs_block_size as u64
    }

    pub fn read_block(&self, block: u64, out: &mut [u8]) -> Result<(), FilesystemError> {
        if out.len() != self.fs_block_size as usize || block >= self.fs_block_count() {
            return Err(FilesystemError::BufferTooSmall);
        }
        let sectors = self.fs_block_size / self.device.block_size();
        let first = block
            .checked_mul(sectors as u64)
            .ok_or(FilesystemError::InvalidPath)?;
        self.device
            .read_blocks(first, sectors, out)
            .map_err(|_| FilesystemError::IoError)
    }

    pub fn write_block(&self, block: u64, data: &[u8]) -> Result<(), FilesystemError> {
        if self.device.is_read_only() {
            return Err(FilesystemError::ReadOnly);
        }
        if data.len() != self.fs_block_size as usize || block >= self.fs_block_count() {
            return Err(FilesystemError::BufferTooSmall);
        }
        let sectors = self.fs_block_size / self.device.block_size();
        let first = block
            .checked_mul(sectors as u64)
            .ok_or(FilesystemError::InvalidPath)?;
        self.device
            .write_blocks(first, sectors, data)
            .map_err(|_| FilesystemError::IoError)
    }

    pub fn read_bytes(&self, offset: u64, out: &mut [u8]) -> Result<(), FilesystemError> {
        if out.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(out.len() as u64)
            .ok_or(FilesystemError::InvalidPath)?;
        if end > self.device.capacity() {
            return Err(FilesystemError::IoError);
        }
        let sector_size = self.device.block_size() as usize;
        if sector_size > 4096 {
            return Err(FilesystemError::UnsupportedOperation);
        }
        let mut sector_buf = [0u8; 4096];
        let mut done = 0usize;
        while done < out.len() {
            let absolute = offset + done as u64;
            let sector = absolute / sector_size as u64;
            let within = absolute as usize % sector_size;
            let count = core::cmp::min(sector_size - within, out.len() - done);
            self.device
                .read_blocks(sector, 1, &mut sector_buf[..sector_size])
                .map_err(|_| FilesystemError::IoError)?;
            out[done..done + count].copy_from_slice(&sector_buf[within..within + count]);
            done += count;
        }
        Ok(())
    }

    pub fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<(), FilesystemError> {
        if data.is_empty() {
            return Ok(());
        }
        if self.device.is_read_only() {
            return Err(FilesystemError::ReadOnly);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(FilesystemError::InvalidPath)?;
        if end > self.device.capacity() {
            return Err(FilesystemError::IoError);
        }
        let sector_size = self.device.block_size() as usize;
        if sector_size > 4096 {
            return Err(FilesystemError::UnsupportedOperation);
        }
        let mut sector_buf = [0u8; 4096];
        let mut done = 0usize;
        while done < data.len() {
            let absolute = offset + done as u64;
            let sector = absolute / sector_size as u64;
            let within = absolute as usize % sector_size;
            let count = core::cmp::min(sector_size - within, data.len() - done);
            if within != 0 || count != sector_size {
                self.device
                    .read_blocks(sector, 1, &mut sector_buf[..sector_size])
                    .map_err(|_| FilesystemError::IoError)?;
            }
            sector_buf[within..within + count].copy_from_slice(&data[done..done + count]);
            self.device
                .write_blocks(sector, 1, &sector_buf[..sector_size])
                .map_err(|_| FilesystemError::IoError)?;
            done += count;
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<(), FilesystemError> {
        self.device.flush().map_err(|_| FilesystemError::IoError)
    }
}
