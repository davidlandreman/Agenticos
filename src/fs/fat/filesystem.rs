use crate::drivers::block::BlockDevice;
use crate::fs::fat::boot_sector::BootSector;
use crate::fs::fat::fat_table::FatTable;
use crate::fs::fat::directory::DirectoryIterator;
use crate::fs::fat::types::{FatType, FatError, ClusterId};
use crate::debug_info;
use alloc;

pub struct FatFilesystem<'a> {
    device: &'a dyn BlockDevice,
    fat_type: FatType,
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    first_data_sector: u32,
    root_dir_sectors: u32,
    root_cluster: ClusterId,
}

#[derive(Clone, Copy)]
pub struct FileHandle {
    pub name: [u8; 13],
    pub size: u32,
    pub first_cluster: ClusterId,
    pub is_directory: bool,
}

impl<'a> FatFilesystem<'a> {
    pub fn new(device: &'a dyn BlockDevice) -> Result<Self, FatError> {
        // Read boot sector
        let mut boot_sector_data = [0u8; 512];
        device.read_blocks(0, 1, &mut boot_sector_data)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
        let fat_type = boot_sector.fat_type()?;
        
        let bytes_per_sector = boot_sector.bpb.bytes_per_sector;
        let sectors_per_cluster = boot_sector.bpb.sectors_per_cluster;
        
        debug_info!("FAT filesystem detected: {:?}", fat_type);
        debug_info!("Bytes per sector: {}", bytes_per_sector);
        debug_info!("Sectors per cluster: {}", sectors_per_cluster);
        
        let root_cluster = match fat_type {
            FatType::Fat32 => ClusterId(boot_sector.fat32_ext().root_cluster),
            _ => ClusterId::ROOT_FAT16,
        };
        
        Ok(Self {
            device,
            fat_type,
            bytes_per_sector,
            sectors_per_cluster,
            first_data_sector: boot_sector.first_data_sector(),
            root_dir_sectors: boot_sector.root_dir_sectors(),
            root_cluster,
        })
    }
    
    pub fn fat_type(&self) -> FatType {
        self.fat_type
    }
    
    fn cluster_to_sector(&self, cluster: ClusterId) -> u32 {
        ((cluster.0 - 2) * self.sectors_per_cluster as u32) + self.first_data_sector
    }
    
    fn read_cluster(&self, cluster: ClusterId, buffer: &mut [u8]) -> Result<(), FatError> {
        let sector = self.cluster_to_sector(cluster);
        self.device.read_blocks(sector as u64, self.sectors_per_cluster as u32, buffer)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        Ok(())
    }
    
    pub fn read_file(&self, file: &FileHandle, buffer: &mut [u8]) -> Result<usize, FatError> {
        if file.is_directory {
            return Err(FatError::InvalidPath);
        }
        
        if buffer.len() < file.size as usize {
            return Err(FatError::BufferTooSmall);
        }
        
        // Handle empty files
        if file.size == 0 || file.first_cluster.0 == 0 {
            return Ok(0);
        }
        
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
        let mut bytes_read = 0;
        
        // Read boot sector to create FAT table
        let mut boot_sector_data = [0u8; 512];
        self.device.read_blocks(0, 1, &mut boot_sector_data)
            .map_err(|_| FatError::BlockDeviceError)?;
        let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
        
        let fat_table = FatTable::new(self.device, boot_sector, self.fat_type);
        
        // Allocate a temporary buffer for reading full clusters
        let mut cluster_buffer = alloc::vec![0u8; cluster_size];
        
        fat_table.follow_chain(file.first_cluster, |cluster| {
            let bytes_to_read = core::cmp::min(cluster_size, file.size as usize - bytes_read);
            
            if bytes_to_read > 0 {
                // Read the full cluster into our temporary buffer
                self.read_cluster(cluster, &mut cluster_buffer)?;
                
                // Copy only the bytes we need into the output buffer
                buffer[bytes_read..bytes_read + bytes_to_read]
                    .copy_from_slice(&cluster_buffer[..bytes_to_read]);
                    
                bytes_read += bytes_to_read;
            }
            
            Ok(())
        })?;
        
        Ok(file.size as usize)
    }
    
    pub fn find_file(&self, path: &str) -> Result<FileHandle, FatError> {
        // For now, only support root directory files
        // Handle both absolute paths (with /) and relative paths (without /)
        let filename = if path.starts_with('/') {
            &path[1..]
        } else {
            path
        };
        
        // Don't process empty filenames or subdirectory paths
        if filename.is_empty() || filename.contains('/') {
            return Err(FatError::NotFound);
        }
        
        let mut files = [FileHandle {
            name: [0; 13],
            size: 0,
            first_cluster: ClusterId(0),
            is_directory: false,
        }; 256];
        let file_count = self.list_root_array(&mut files, 256)?;
        for i in 0..file_count {
            let file = &files[i];
            let file_name_str = core::str::from_utf8(&file.name)
                .ok()
                .and_then(|s| s.split('\0').next())
                .unwrap_or("");
                
            if file_name_str.eq_ignore_ascii_case(filename) {
                return Ok(*file);
            }
        }
        
        Err(FatError::NotFound)
    }
}

// For now, we use a simpler approach with static arrays
impl FatFilesystem<'_> {
    pub fn list_root_array(&self, entries: &mut [FileHandle], max_entries: usize) -> Result<usize, FatError> {
        let mut count = 0;
        
        match self.fat_type {
            FatType::Fat12 | FatType::Fat16 => {
                let root_start_sector = self.first_data_sector - self.root_dir_sectors;
                let mut buffer = [0u8; 512];
                
                'outer: for i in 0..self.root_dir_sectors {
                    self.device.read_blocks((root_start_sector + i) as u64, 1, &mut buffer)
                        .map_err(|_| FatError::BlockDeviceError)?;
                        
                    for entry in DirectoryIterator::new(&buffer) {
                        if let Ok(dir_entry) = entry {
                            if count >= max_entries {
                                break 'outer;
                            }
                            
                            entries[count] = FileHandle {
                                name: dir_entry.format_name(),
                                size: dir_entry.file_size,
                                first_cluster: dir_entry.first_cluster(),
                                is_directory: dir_entry.attributes().is_directory(),
                            };
                            count += 1;
                        }
                    }
                }
            }
            FatType::Fat32 => {
                count = self.read_directory_array(self.root_cluster, entries, max_entries)?;
            }
        }
        
        Ok(count)
    }
    
    fn read_directory_array(&self, start_cluster: ClusterId, entries: &mut [FileHandle], max_entries: usize) -> Result<usize, FatError> {
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
        let mut buffer = [0u8; 8192]; // Fixed size buffer for cluster data
        let mut count = 0;
        
        // Read boot sector to create FAT table
        let mut boot_sector_data = [0u8; 512];
        self.device.read_blocks(0, 1, &mut boot_sector_data)
            .map_err(|_| FatError::BlockDeviceError)?;
        let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
        
        let fat_table = FatTable::new(self.device, boot_sector, self.fat_type);
        
        fat_table.follow_chain(start_cluster, |cluster| {
            if count >= max_entries {
                return Ok(());
            }
            
            self.read_cluster(cluster, &mut buffer[..cluster_size])?;
            
            for entry in DirectoryIterator::new(&buffer[..cluster_size]) {
                if let Ok(dir_entry) = entry {
                    if count >= max_entries {
                        break;
                    }
                    
                    entries[count] = FileHandle {
                        name: dir_entry.format_name(),
                        size: dir_entry.file_size,
                        first_cluster: dir_entry.first_cluster(),
                        is_directory: dir_entry.attributes().is_directory(),
                    };
                    count += 1;
                }
            }
            
            Ok(())
        })?;
        
        Ok(count)
    }
}