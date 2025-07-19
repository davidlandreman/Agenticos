use crate::drivers::block::BlockDevice;
use core::fmt;

/// Partition type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    Empty,
    Fat12,
    Fat16,
    Fat32,
    Extended,
    LinuxSwap,
    LinuxNative,
    Ntfs,
    Unknown(u8),
}

impl PartitionType {
    pub fn from_mbr_type(type_id: u8) -> Self {
        match type_id {
            0x00 => PartitionType::Empty,
            0x01 => PartitionType::Fat12,
            0x04 | 0x06 | 0x0E => PartitionType::Fat16,
            0x0B | 0x0C => PartitionType::Fat32,
            0x05 | 0x0F => PartitionType::Extended,
            0x07 => PartitionType::Ntfs,
            0x82 => PartitionType::LinuxSwap,
            0x83 => PartitionType::LinuxNative,
            _ => PartitionType::Unknown(type_id),
        }
    }
}

/// Partition information
#[derive(Debug, Clone, Copy)]
pub struct Partition {
    pub partition_type: PartitionType,
    pub bootable: bool,
    pub start_lba: u64,
    pub size_sectors: u64,
}

/// Partition-aware block device that provides access to a specific partition
pub struct PartitionBlockDevice<'a> {
    device: &'a dyn BlockDevice,
    start_lba: u64,
    size_sectors: u64,
}

impl<'a> PartitionBlockDevice<'a> {
    pub fn new(device: &'a dyn BlockDevice, partition: &Partition) -> Self {
        Self {
            device,
            start_lba: partition.start_lba,
            size_sectors: partition.size_sectors,
        }
    }
}

impl<'a> BlockDevice for PartitionBlockDevice<'a> {
    fn read_blocks(&self, block: u64, count: u32, buffer: &mut [u8]) -> Result<(), &'static str> {
        // Check bounds
        if block + count as u64 > self.size_sectors {
            return Err("Read beyond partition boundary");
        }
        
        // Translate partition-relative block to device block
        let device_block = self.start_lba + block;
        self.device.read_blocks(device_block, count, buffer)
    }
    
    fn write_blocks(&self, block: u64, count: u32, buffer: &[u8]) -> Result<(), &'static str> {
        // Check bounds
        if block + count as u64 > self.size_sectors {
            return Err("Write beyond partition boundary");
        }
        
        // Translate partition-relative block to device block
        let device_block = self.start_lba + block;
        self.device.write_blocks(device_block, count, buffer)
    }
    
    fn block_size(&self) -> u32 {
        self.device.block_size()
    }
    
    fn total_blocks(&self) -> u64 {
        self.size_sectors
    }
    
    fn name(&self) -> &str {
        "Partition"
    }
}

/// MBR partition table entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MbrPartitionEntry {
    bootable: u8,           // 0x80 = bootable, 0x00 = not bootable
    start_chs: [u8; 3],     // CHS address of first sector
    partition_type: u8,     // Partition type
    end_chs: [u8; 3],       // CHS address of last sector
    start_lba: u32,         // LBA of first sector
    size_sectors: u32,      // Number of sectors
}

/// Read partition table from a block device
pub fn read_partitions(device: &dyn BlockDevice) -> Result<[Option<Partition>; 4], &'static str> {
    let mut buffer = [0u8; 512];
    let mut partitions = [None; 4];
    
    // Read MBR
    device.read_blocks(0, 1, &mut buffer)?;
    
    // Check MBR signature
    if buffer[510] != 0x55 || buffer[511] != 0xAA {
        return Err("Invalid MBR signature");
    }
    
    // Read partition entries
    for i in 0..4 {
        let offset = 0x1BE + (i * 16);
        let entry_bytes = &buffer[offset..offset + 16];
        
        // Safe way to read packed struct
        let entry = MbrPartitionEntry {
            bootable: entry_bytes[0],
            start_chs: [entry_bytes[1], entry_bytes[2], entry_bytes[3]],
            partition_type: entry_bytes[4],
            end_chs: [entry_bytes[5], entry_bytes[6], entry_bytes[7]],
            start_lba: u32::from_le_bytes([
                entry_bytes[8], entry_bytes[9], entry_bytes[10], entry_bytes[11]
            ]),
            size_sectors: u32::from_le_bytes([
                entry_bytes[12], entry_bytes[13], entry_bytes[14], entry_bytes[15]
            ]),
        };
        
        if entry.partition_type != 0 && entry.start_lba != 0 {
            partitions[i] = Some(Partition {
                partition_type: PartitionType::from_mbr_type(entry.partition_type),
                bootable: entry.bootable == 0x80,
                start_lba: entry.start_lba as u64,
                size_sectors: entry.size_sectors as u64,
            });
        }
    }
    
    Ok(partitions)
}