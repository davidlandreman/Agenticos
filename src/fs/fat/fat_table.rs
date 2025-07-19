use crate::drivers::block::BlockDevice;
use crate::fs::fat::types::{FatType, FatError, ClusterId};
use crate::fs::fat::boot_sector::BootSector;

pub struct FatTable<'a> {
    device: &'a dyn BlockDevice,
    fat_type: FatType,
    fat_start_sector: u32,
    sectors_per_fat: u32,
    bytes_per_sector: u16,
}

impl<'a> FatTable<'a> {
    pub fn new(device: &'a dyn BlockDevice, boot_sector: &BootSector, fat_type: FatType) -> Self {
        let sectors_per_fat = if boot_sector.bpb.sectors_per_fat_16 != 0 {
            boot_sector.bpb.sectors_per_fat_16 as u32
        } else {
            boot_sector.fat32_ext().sectors_per_fat_32
        };
        
        Self {
            device,
            fat_type,
            fat_start_sector: boot_sector.bpb.reserved_sectors as u32,
            sectors_per_fat,
            bytes_per_sector: boot_sector.bpb.bytes_per_sector,
        }
    }
    
    pub fn read_entry(&self, cluster: ClusterId) -> Result<ClusterId, FatError> {
        match self.fat_type {
            FatType::Fat12 => self.read_fat12_entry(cluster),
            FatType::Fat16 => self.read_fat16_entry(cluster),
            FatType::Fat32 => self.read_fat32_entry(cluster),
        }
    }
    
    fn read_fat12_entry(&self, cluster: ClusterId) -> Result<ClusterId, FatError> {
        let fat_offset = cluster.0 + (cluster.0 / 2);
        let fat_sector = self.fat_start_sector + (fat_offset / self.bytes_per_sector as u32);
        let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;
        
        let mut buffer = [0u8; 512];
        self.device.read_blocks(fat_sector as u64, 1, &mut buffer)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        let value = if cluster.0 & 1 == 1 {
            // Odd cluster
            if ent_offset == 511 {
                // Entry spans sectors
                let low = buffer[511];
                self.device.read_blocks(fat_sector as u64 + 1, 1, &mut buffer)
                    .map_err(|_| FatError::BlockDeviceError)?;
                let high = buffer[0];
                ((high as u16) << 8 | low as u16) >> 4
            } else {
                u16::from_le_bytes([buffer[ent_offset], buffer[ent_offset + 1]]) >> 4
            }
        } else {
            // Even cluster
            if ent_offset == 511 {
                // Entry spans sectors
                let low = buffer[511];
                self.device.read_blocks(fat_sector as u64 + 1, 1, &mut buffer)
                    .map_err(|_| FatError::BlockDeviceError)?;
                let high = buffer[0];
                (high as u16 & 0x0F) << 8 | low as u16
            } else {
                u16::from_le_bytes([buffer[ent_offset], buffer[ent_offset + 1]]) & 0x0FFF
            }
        };
        
        Ok(ClusterId(value as u32))
    }
    
    fn read_fat16_entry(&self, cluster: ClusterId) -> Result<ClusterId, FatError> {
        let fat_offset = cluster.0 * 2;
        let fat_sector = self.fat_start_sector + (fat_offset / self.bytes_per_sector as u32);
        let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;
        
        let mut buffer = [0u8; 512];
        self.device.read_blocks(fat_sector as u64, 1, &mut buffer)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        let value = u16::from_le_bytes([buffer[ent_offset], buffer[ent_offset + 1]]);
        Ok(ClusterId(value as u32))
    }
    
    fn read_fat32_entry(&self, cluster: ClusterId) -> Result<ClusterId, FatError> {
        let fat_offset = cluster.0 * 4;
        let fat_sector = self.fat_start_sector + (fat_offset / self.bytes_per_sector as u32);
        let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;
        
        let mut buffer = [0u8; 512];
        self.device.read_blocks(fat_sector as u64, 1, &mut buffer)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        let value = u32::from_le_bytes([
            buffer[ent_offset],
            buffer[ent_offset + 1],
            buffer[ent_offset + 2],
            buffer[ent_offset + 3],
        ]) & 0x0FFFFFFF; // Mask off upper 4 bits
        
        Ok(ClusterId(value))
    }
    
    pub fn follow_chain(&self, start_cluster: ClusterId, mut callback: impl FnMut(ClusterId) -> Result<(), FatError>) -> Result<(), FatError> {
        let mut current = start_cluster;
        
        loop {
            if !current.is_valid(self.fat_type) {
                return Err(FatError::InvalidCluster);
            }
            
            if current.is_bad(self.fat_type) {
                return Err(FatError::BadCluster);
            }
            
            callback(current)?;
            
            current = self.read_entry(current)?;
            
            if current.is_end_of_chain(self.fat_type) {
                break;
            }
        }
        
        Ok(())
    }
}