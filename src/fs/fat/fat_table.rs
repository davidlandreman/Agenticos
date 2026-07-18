use crate::drivers::block::BlockDevice;
use crate::fs::fat::types::{FatType, FatError, ClusterId};
use crate::fs::fat::boot_sector::BootSector;

pub struct FatTable<'a> {
    device: &'a dyn BlockDevice,
    fat_type: FatType,
    fat_start_sector: u32,
    sectors_per_fat: u32,
    bytes_per_sector: u16,
    num_fats: u8,
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
            num_fats: boot_sector.bpb.num_fats,
        }
    }


    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn num_fats(&self) -> u8 {
        self.num_fats
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn sectors_per_fat(&self) -> u32 {
        self.sectors_per_fat
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn bytes_per_sector(&self) -> u16 {
        self.bytes_per_sector
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
    
    // ---------- Write side (Phase C U8) ----------

    /// Write `value` to FAT entry `cluster`, mirrored across every FAT
    /// copy (`num_fats`, typically 2). Read-modify-write at the sector
    /// level — FAT12 packs two entries per 3 bytes, FAT16 is 2-byte
    /// aligned, FAT32 is 4-byte aligned (high nibble preserved).
    ///
    /// Cluster 0 and 1 are FAT metadata (media descriptor copy + the
    /// "clean" bit on FAT16/32) and the caller is responsible for
    /// preserving their semantics — `write_clean_bit` is the
    /// supported path for the dirty-bit work.
    pub fn write_entry(&self, cluster: ClusterId, value: ClusterId) -> Result<(), FatError> {
        match self.fat_type {
            FatType::Fat12 => self.write_fat12_entry(cluster, value),
            FatType::Fat16 => self.write_fat16_entry(cluster, value),
            FatType::Fat32 => self.write_fat32_entry(cluster, value),
        }
    }

    fn write_to_all_fats(&self, sector_offset_in_fat: u32, buffer: &[u8; 512]) -> Result<(), FatError> {
        for fat_idx in 0..self.num_fats as u32 {
            let absolute_sector = self.fat_start_sector + fat_idx * self.sectors_per_fat + sector_offset_in_fat;
            self.device.write_blocks(absolute_sector as u64, 1, buffer)
                .map_err(|_| FatError::BlockDeviceError)?;
        }
        Ok(())
    }

    fn write_fat12_entry(&self, cluster: ClusterId, value: ClusterId) -> Result<(), FatError> {
        let fat_offset = cluster.0 + (cluster.0 / 2);
        let sector_in_fat = fat_offset / self.bytes_per_sector as u32;
        let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;

        // Read sector(s); FAT12 entries can straddle a sector boundary.
        let mut buffer = [0u8; 512];
        let absolute_sector = self.fat_start_sector + sector_in_fat;
        self.device.read_blocks(absolute_sector as u64, 1, &mut buffer)
            .map_err(|_| FatError::BlockDeviceError)?;

        let v = (value.0 & 0x0FFF) as u16;
        if cluster.0 & 1 == 1 {
            // Odd cluster: lives in high 12 bits of the 16-bit pair.
            if ent_offset == 511 {
                let low = (v << 4) as u8 & 0xF0; // low nibble of pair
                let low_existing = buffer[511] & 0x0F;
                buffer[511] = low_existing | low;
                self.write_to_all_fats(sector_in_fat, &buffer)?;
                // Spill into next sector.
                let mut next = [0u8; 512];
                let next_abs = absolute_sector + 1;
                self.device.read_blocks(next_abs as u64, 1, &mut next)
                    .map_err(|_| FatError::BlockDeviceError)?;
                next[0] = (v >> 4) as u8;
                self.write_to_all_fats(sector_in_fat + 1, &next)?;
            } else {
                // High nibble of byte[ent] preserved (low nibble of even neighbor);
                // overwrite high nibble with v's low 4 bits, plus byte[ent+1] with v's high 8 bits.
                let low_neighbor = buffer[ent_offset] & 0x0F;
                buffer[ent_offset] = low_neighbor | ((v << 4) as u8 & 0xF0);
                buffer[ent_offset + 1] = (v >> 4) as u8;
                self.write_to_all_fats(sector_in_fat, &buffer)?;
            }
        } else {
            // Even cluster: lives in low 12 bits of the 16-bit pair.
            if ent_offset == 511 {
                buffer[511] = (v & 0xFF) as u8;
                self.write_to_all_fats(sector_in_fat, &buffer)?;
                let mut next = [0u8; 512];
                let next_abs = absolute_sector + 1;
                self.device.read_blocks(next_abs as u64, 1, &mut next)
                    .map_err(|_| FatError::BlockDeviceError)?;
                let high_neighbor = next[0] & 0xF0;
                next[0] = high_neighbor | ((v >> 8) as u8 & 0x0F);
                self.write_to_all_fats(sector_in_fat + 1, &next)?;
            } else {
                buffer[ent_offset] = (v & 0xFF) as u8;
                let high_neighbor = buffer[ent_offset + 1] & 0xF0;
                buffer[ent_offset + 1] = high_neighbor | ((v >> 8) as u8 & 0x0F);
                self.write_to_all_fats(sector_in_fat, &buffer)?;
            }
        }
        Ok(())
    }

    fn write_fat16_entry(&self, cluster: ClusterId, value: ClusterId) -> Result<(), FatError> {
        let fat_offset = cluster.0 * 2;
        let sector_in_fat = fat_offset / self.bytes_per_sector as u32;
        let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;

        let mut buffer = [0u8; 512];
        let absolute_sector = self.fat_start_sector + sector_in_fat;
        self.device.read_blocks(absolute_sector as u64, 1, &mut buffer)
            .map_err(|_| FatError::BlockDeviceError)?;

        let v = value.0 as u16;
        buffer[ent_offset] = (v & 0xFF) as u8;
        buffer[ent_offset + 1] = (v >> 8) as u8;
        self.write_to_all_fats(sector_in_fat, &buffer)?;
        Ok(())
    }

    fn write_fat32_entry(&self, cluster: ClusterId, value: ClusterId) -> Result<(), FatError> {
        let fat_offset = cluster.0 * 4;
        let sector_in_fat = fat_offset / self.bytes_per_sector as u32;
        let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;

        let mut buffer = [0u8; 512];
        let absolute_sector = self.fat_start_sector + sector_in_fat;
        self.device.read_blocks(absolute_sector as u64, 1, &mut buffer)
            .map_err(|_| FatError::BlockDeviceError)?;

        // FAT32 entries are 28-bit; preserve the high 4 bits (reserved).
        let existing_high = u32::from_le_bytes([
            buffer[ent_offset],
            buffer[ent_offset + 1],
            buffer[ent_offset + 2],
            buffer[ent_offset + 3],
        ]) & 0xF0000000;
        let new_val = existing_high | (value.0 & 0x0FFFFFFF);
        let bytes = new_val.to_le_bytes();
        buffer[ent_offset..ent_offset + 4].copy_from_slice(&bytes);
        self.write_to_all_fats(sector_in_fat, &buffer)?;
        Ok(())
    }

    /// Read FAT[1] and return the "clean" bit. True means clean
    /// (previous unmount was clean); false means dirty (unclean
    /// shutdown). FAT12 has no such bit — returns `true` (assume clean).
    ///
    /// Reference: Microsoft FAT32 specification, §3.6 "Determining FAT
    /// File System Type" — bit 15 of FAT[1] on FAT16, bit 27 of FAT[1]
    /// on FAT32.
    pub fn read_clean_bit(&self) -> Result<bool, FatError> {
        match self.fat_type {
            FatType::Fat12 => Ok(true), // no dirty bit on FAT12
            FatType::Fat16 => {
                let entry = self.read_fat16_entry(ClusterId(1))?;
                Ok(entry.0 & 0x8000 != 0)
            }
            FatType::Fat32 => {
                let entry = self.read_fat32_entry(ClusterId(1))?;
                Ok(entry.0 & 0x08000000 != 0)
            }
        }
    }

    /// Write the clean bit of FAT[1]. `clean = true` means "previous
    /// shutdown was clean / no writes pending"; `false` means dirty.
    /// Set FALSE on mount-for-write; restore TRUE on sync. No-op on
    /// FAT12.
    pub fn write_clean_bit(&self, clean: bool) -> Result<(), FatError> {
        match self.fat_type {
            FatType::Fat12 => Ok(()),
            FatType::Fat16 => {
                let current = self.read_fat16_entry(ClusterId(1))?;
                let new = if clean {
                    ClusterId(current.0 | 0x8000)
                } else {
                    ClusterId(current.0 & !0x8000)
                };
                self.write_fat16_entry(ClusterId(1), new)
            }
            FatType::Fat32 => {
                // Preserve the upper 4 bits AND the other low 24 bits;
                // only flip bit 27. read_fat32_entry masks off the
                // upper 4 reserved bits, so we have to re-fetch the
                // raw word here.
                let fat_offset = 1u32 * 4;
                let sector_in_fat = fat_offset / self.bytes_per_sector as u32;
                let ent_offset = (fat_offset % self.bytes_per_sector as u32) as usize;
                let mut buffer = [0u8; 512];
                let absolute_sector = self.fat_start_sector + sector_in_fat;
                self.device.read_blocks(absolute_sector as u64, 1, &mut buffer)
                    .map_err(|_| FatError::BlockDeviceError)?;
                let mut raw = u32::from_le_bytes([
                    buffer[ent_offset],
                    buffer[ent_offset + 1],
                    buffer[ent_offset + 2],
                    buffer[ent_offset + 3],
                ]);
                if clean {
                    raw |= 0x08000000;
                } else {
                    raw &= !0x08000000;
                }
                let bytes = raw.to_le_bytes();
                buffer[ent_offset..ent_offset + 4].copy_from_slice(&bytes);
                self.write_to_all_fats(sector_in_fat, &buffer)
            }
        }
    }

    /// Scan the FAT for a free cluster starting at `hint`, wrapping
    /// at `max_cluster`. Returns `Err(DiskFull)` if no free cluster
    /// is found after a full pass. Caller marks the returned cluster
    /// allocated by `write_entry(cluster, EOC)`.
    pub fn find_free_cluster(&self, hint: ClusterId, max_cluster: u32) -> Result<ClusterId, FatError> {
        let start = hint.0.max(2);
        let total_to_scan = max_cluster.saturating_sub(2);
        let mut idx = 0u32;
        while idx <= total_to_scan {
            let candidate = 2 + ((start - 2 + idx) % (max_cluster - 2 + 1));
            let entry = self.read_entry(ClusterId(candidate))?;
            if entry.0 == 0 {
                return Ok(ClusterId(candidate));
            }
            idx += 1;
        }
        Err(FatError::DiskFull)
    }

    pub fn follow_chain(&self, start_cluster: ClusterId, mut callback: impl FnMut(ClusterId) -> Result<(), FatError>) -> Result<(), FatError> {
        let mut current = start_cluster;
        let mut visited: u32 = 0;

        loop {
            if !current.is_valid(self.fat_type) {
                crate::debug_warn!("[fat] follow_chain: invalid cluster {} after {} visits", current.0, visited);
                return Err(FatError::InvalidCluster);
            }

            if current.is_bad(self.fat_type) {
                crate::debug_warn!("[fat] follow_chain: bad cluster {} after {} visits", current.0, visited);
                return Err(FatError::BadCluster);
            }

            callback(current)?;
            visited += 1;
            if visited.is_multiple_of(32) {
                crate::debug_info!("[fat] follow_chain: {} clusters visited, current={}", visited, current.0);
            }

            current = self.read_entry(current)?;

            if current.is_end_of_chain(self.fat_type) {
                crate::debug_info!("[fat] follow_chain: end-of-chain after {} clusters", visited);
                break;
            }
        }

        Ok(())
    }
}