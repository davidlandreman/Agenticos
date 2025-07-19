use crate::fs::fat::types::{FatType, FatError};

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BiosParameterBlock {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub root_entries: u16,
    pub total_sectors_16: u16,
    pub media_descriptor: u8,
    pub sectors_per_fat_16: u16,
    pub sectors_per_track: u16,
    pub num_heads: u16,
    pub hidden_sectors: u32,
    pub total_sectors_32: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Fat16ExtBootRecord {
    pub drive_number: u8,
    pub reserved: u8,
    pub boot_signature: u8,
    pub volume_id: u32,
    pub volume_label: [u8; 11],
    pub filesystem_type: [u8; 8],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Fat32ExtBootRecord {
    pub sectors_per_fat_32: u32,
    pub ext_flags: u16,
    pub fs_version: u16,
    pub root_cluster: u32,
    pub fs_info_sector: u16,
    pub backup_boot_sector: u16,
    pub reserved: [u8; 12],
    pub drive_number: u8,
    pub reserved1: u8,
    pub boot_signature: u8,
    pub volume_id: u32,
    pub volume_label: [u8; 11],
    pub filesystem_type: [u8; 8],
}

#[repr(C, packed)]
pub struct BootSector {
    pub jump_boot: [u8; 3],
    pub oem_name: [u8; 8],
    pub bpb: BiosParameterBlock,
    pub fat_specific: [u8; 54], // Will be cast to Fat16ExtBootRecord or Fat32ExtBootRecord
    pub boot_code: [u8; 420],
    pub boot_signature: u16,
}

impl BootSector {
    pub const BOOT_SIGNATURE: u16 = 0xAA55;
    
    pub fn from_bytes(data: &[u8; 512]) -> Result<&Self, FatError> {
        let boot_sector = unsafe { &*(data.as_ptr() as *const Self) };
        
        if boot_sector.boot_signature != Self::BOOT_SIGNATURE {
            return Err(FatError::InvalidBootSector);
        }
        
        Ok(boot_sector)
    }
    
    pub fn fat_type(&self) -> Result<FatType, FatError> {
        // Validate basic parameters
        if self.bpb.bytes_per_sector == 0 || self.bpb.sectors_per_cluster == 0 {
            return Err(FatError::InvalidBootSector);
        }
        
        let root_dir_sectors = ((self.bpb.root_entries as u32 * 32) 
            + (self.bpb.bytes_per_sector as u32 - 1)) 
            / self.bpb.bytes_per_sector as u32;
            
        let fat_sectors = if self.bpb.sectors_per_fat_16 != 0 {
            self.bpb.sectors_per_fat_16 as u32
        } else {
            self.fat32_ext().sectors_per_fat_32
        };
        
        let total_sectors = if self.bpb.total_sectors_16 != 0 {
            self.bpb.total_sectors_16 as u32
        } else {
            self.bpb.total_sectors_32
        };
        
        let overhead_sectors = self.bpb.reserved_sectors as u32
            + (self.bpb.num_fats as u32 * fat_sectors)
            + root_dir_sectors;
            
        // Check for invalid FAT filesystem (overhead exceeds total sectors)
        if overhead_sectors >= total_sectors {
            return Err(FatError::InvalidBootSector);
        }
        
        let data_sectors = total_sectors - overhead_sectors;
                
        let count_of_clusters = data_sectors / self.bpb.sectors_per_cluster as u32;
        
        if count_of_clusters < 4085 {
            Ok(FatType::Fat12)
        } else if count_of_clusters < 65525 {
            Ok(FatType::Fat16)
        } else {
            Ok(FatType::Fat32)
        }
    }
    
    pub fn fat16_ext(&self) -> &Fat16ExtBootRecord {
        unsafe { &*(self.fat_specific.as_ptr() as *const Fat16ExtBootRecord) }
    }
    
    pub fn fat32_ext(&self) -> &Fat32ExtBootRecord {
        unsafe { &*(self.fat_specific.as_ptr() as *const Fat32ExtBootRecord) }
    }
    
    pub fn first_data_sector(&self) -> u32 {
        let root_dir_sectors = ((self.bpb.root_entries as u32 * 32) 
            + (self.bpb.bytes_per_sector as u32 - 1)) 
            / self.bpb.bytes_per_sector as u32;
            
        let fat_sectors = if self.bpb.sectors_per_fat_16 != 0 {
            self.bpb.sectors_per_fat_16 as u32
        } else {
            self.fat32_ext().sectors_per_fat_32
        };
        
        self.bpb.reserved_sectors as u32
            + (self.bpb.num_fats as u32 * fat_sectors)
            + root_dir_sectors
    }
    
    pub fn cluster_to_sector(&self, cluster: u32) -> u32 {
        ((cluster - 2) * self.bpb.sectors_per_cluster as u32) + self.first_data_sector()
    }
    
    pub fn root_dir_sectors(&self) -> u32 {
        ((self.bpb.root_entries as u32 * 32) 
            + (self.bpb.bytes_per_sector as u32 - 1)) 
            / self.bpb.bytes_per_sector as u32
    }
}