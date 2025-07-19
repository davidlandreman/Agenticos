//! IDE/ATA disk driver for x86_64
//! 
//! This driver implements PIO mode access to IDE/ATA hard drives.
//! It supports the primary and secondary IDE channels with master/slave drives.

use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::drivers::block::BlockDevice;

/// IDE/ATA register port offsets
#[derive(Debug, Clone, Copy)]
#[repr(u16)]
pub enum IdeRegister {
    Data = 0x00,
    ErrorFeatures = 0x01,    // Error when reading, Features when writing
    SectorCount = 0x02,
    LbaLow = 0x03,
    LbaMid = 0x04,
    LbaHigh = 0x05,
    DriveSelect = 0x06,
    StatusCommand = 0x07,    // Status when reading, Command when writing
}

/// IDE status register bits
#[repr(u8)]
pub enum IdeStatus {
    Busy = 0x80,
    Ready = 0x40,
    WriteFault = 0x20,
    SeekComplete = 0x10,
    DataRequest = 0x08,
    CorrectedData = 0x04,
    Index = 0x02,
    Error = 0x01,
}

/// IDE commands
#[repr(u8)]
pub enum IdeCommand {
    ReadPio = 0x20,
    ReadPioExt = 0x24,
    WritePio = 0x30,
    WritePioExt = 0x34,
    Identify = 0xEC,
    SetFeatures = 0xEF,
}

/// IDE channel (Primary or Secondary)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IdeChannel {
    Primary,
    Secondary,
}

impl IdeChannel {
    /// Get the base I/O port for this channel
    pub fn base_port(&self) -> u16 {
        match self {
            IdeChannel::Primary => 0x1F0,
            IdeChannel::Secondary => 0x170,
        }
    }

    /// Get the control I/O port for this channel
    pub fn control_port(&self) -> u16 {
        match self {
            IdeChannel::Primary => 0x3F6,
            IdeChannel::Secondary => 0x376,
        }
    }

    /// Get the IRQ number for this channel
    pub fn irq(&self) -> u8 {
        match self {
            IdeChannel::Primary => 14,
            IdeChannel::Secondary => 15,
        }
    }
}

/// IDE drive (Master or Slave)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IdeDrive {
    Master,
    Slave,
}

impl IdeDrive {
    /// Get the drive select value for LBA mode
    pub fn select_value(&self, lba: bool) -> u8 {
        let mut value = 0xA0; // Always set bit 5 and 7
        if lba {
            value |= 0x40; // Set LBA bit
        }
        if *self == IdeDrive::Slave {
            value |= 0x10; // Set slave bit
        }
        value
    }
}

/// Drive identification data (512 bytes)
#[repr(C)]
pub struct IdentifyData {
    pub config: u16,
    pub cylinders: u16,
    pub reserved1: u16,
    pub heads: u16,
    pub bytes_per_track: u16,
    pub bytes_per_sector: u16,
    pub sectors_per_track: u16,
    pub vendor_unique: [u16; 3],
    pub serial_number: [u8; 20],
    pub buffer_type: u16,
    pub buffer_size: u16,
    pub ecc_bytes: u16,
    pub firmware_revision: [u8; 8],
    pub model_number: [u8; 40],
    pub max_multi_sector: u8,
    pub vendor_unique2: u8,
    pub dword_io: u16,
    pub capabilities: u16,
    pub reserved2: u16,
    pub pio_timing: u16,
    pub dma_timing: u16,
    pub field_valid: u16,
    pub current_cylinders: u16,
    pub current_heads: u16,
    pub current_sectors_per_track: u16,
    pub current_capacity: u32,
    pub multi_sector_setting: u8,
    pub multi_sector_valid: u8,
    pub total_addressable_sectors: u32,
    pub single_word_dma: u16,
    pub multi_word_dma: u16,
    pub advanced_pio_modes: u8,
    pub reserved3: u8,
    pub min_mw_dma_cycle_time: u16,
    pub rec_mw_dma_cycle_time: u16,
    pub min_pio_cycle_time: u16,
    pub min_pio_cycle_time_iordy: u16,
    pub reserved4: [u16; 2],
    pub release_time_overlapped: u16,
    pub release_time_service: u16,
    pub major_revision: u16,
    pub minor_revision: u16,
    pub reserved5: [u16; 12],
    pub command_set_support: u64,
    pub command_sets_enabled: u64,
    pub ultra_dma_support: u8,
    pub ultra_dma_active: u8,
    pub reserved6: [u16; 37],
    pub lba48_addressable_sectors: u64,
    pub reserved7: [u16; 23],
    pub removable_media_status: u16,
    pub security_status: u16,
    pub vendor_specific: [u16; 31],
    pub cfa_power_mode: u16,
    pub reserved8: [u16; 15],
    pub current_media_serial: u16,
    pub reserved9: [u16; 49],
    pub integrity_word: u16,
}

/// IDE disk device
pub struct IdeDisk {
    channel: IdeChannel,
    drive: IdeDrive,
    present: bool,
    supports_lba: bool,
    supports_lba48: bool,
    total_sectors: u64,
    model: [u8; 40],
    serial: [u8; 20],
}

impl IdeDisk {
    /// Create a new IDE disk instance
    pub fn new(channel: IdeChannel, drive: IdeDrive) -> Self {
        Self {
            channel,
            drive,
            present: false,
            supports_lba: false,
            supports_lba48: false,
            total_sectors: 0,
            model: [0; 40],
            serial: [0; 20],
        }
    }

    /// Check if the disk is present
    pub fn is_present(&self) -> bool {
        self.present
    }

    /// Get the disk capacity in sectors
    pub fn capacity(&self) -> u64 {
        self.total_sectors
    }

    /// Get the model string
    pub fn model_string(&self) -> &str {
        let len = self.model.iter().position(|&c| c == 0).unwrap_or(40);
        core::str::from_utf8(&self.model[..len]).unwrap_or("Unknown")
    }
}

/// IDE controller
pub struct IdeController {
    primary_master: Mutex<IdeDisk>,
    primary_slave: Mutex<IdeDisk>,
    secondary_master: Mutex<IdeDisk>,
    secondary_slave: Mutex<IdeDisk>,
    initialized: AtomicBool,
}

impl IdeController {
    /// Create a new IDE controller instance
    /// Create a new IDE controller instance with uninitialized disks
    pub const fn new() -> Self {
        Self {
            primary_master: Mutex::new(IdeDisk {
                channel: IdeChannel::Primary,
                drive: IdeDrive::Master,
                present: false,
                supports_lba: false,
                supports_lba48: false,
                total_sectors: 0,
                model: [0; 40],
                serial: [0; 20],
            }),
            primary_slave: Mutex::new(IdeDisk {
                channel: IdeChannel::Primary,
                drive: IdeDrive::Slave,
                present: false,
                supports_lba: false,
                supports_lba48: false,
                total_sectors: 0,
                model: [0; 40],
                serial: [0; 20],
            }),
            secondary_master: Mutex::new(IdeDisk {
                channel: IdeChannel::Secondary,
                drive: IdeDrive::Master,
                present: false,
                supports_lba: false,
                supports_lba48: false,
                total_sectors: 0,
                model: [0; 40],
                serial: [0; 20],
            }),
            secondary_slave: Mutex::new(IdeDisk {
                channel: IdeChannel::Secondary,
                drive: IdeDrive::Slave,
                present: false,
                supports_lba: false,
                supports_lba48: false,
                total_sectors: 0,
                model: [0; 40],
                serial: [0; 20],
            }),
            initialized: AtomicBool::new(false),
        }
    }

    /// Get a mutable reference to a specific disk
    fn get_disk_mut(&self, channel: IdeChannel, drive: IdeDrive) -> &Mutex<IdeDisk> {
        match (channel, drive) {
            (IdeChannel::Primary, IdeDrive::Master) => &self.primary_master,
            (IdeChannel::Primary, IdeDrive::Slave) => &self.primary_slave,
            (IdeChannel::Secondary, IdeDrive::Master) => &self.secondary_master,
            (IdeChannel::Secondary, IdeDrive::Slave) => &self.secondary_slave,
        }
    }
    
    /// Get disk information (public interface)
    pub fn get_disk_info(&self, channel: IdeChannel, drive: IdeDrive) -> Option<([u8; 40], u64)> {
        let disk = self.get_disk_mut(channel, drive).lock();
        if disk.is_present() {
            Some((disk.model, disk.total_sectors))
        } else {
            None
        }
    }
}

/// Global IDE controller instance
pub static IDE_CONTROLLER: IdeController = IdeController::new();

/// Helper function to read from an IDE register
unsafe fn ide_read_register(channel: IdeChannel, register: IdeRegister) -> u8 {
    let port = channel.base_port() + register as u16;
    let mut port: PortReadOnly<u8> = PortReadOnly::new(port);
    port.read()
}

/// Helper function to write to an IDE register
unsafe fn ide_write_register(channel: IdeChannel, register: IdeRegister, value: u8) {
    let port = channel.base_port() + register as u16;
    let mut port: PortWriteOnly<u8> = PortWriteOnly::new(port);
    port.write(value);
}

/// Helper function to read from the alternate status register
unsafe fn ide_read_alt_status(channel: IdeChannel) -> u8 {
    let port = channel.control_port();
    let mut port: PortReadOnly<u8> = PortReadOnly::new(port);
    port.read()
}

/// Helper function to write to the device control register
unsafe fn ide_write_device_control(channel: IdeChannel, value: u8) {
    let port = channel.control_port();
    let mut port: PortWriteOnly<u8> = PortWriteOnly::new(port);
    port.write(value);
}

/// Wait for the drive to be ready (not busy)
unsafe fn wait_ready(channel: IdeChannel) -> Result<(), &'static str> {
    for _ in 0..1000 {
        let status = ide_read_register(channel, IdeRegister::StatusCommand);
        if status & IdeStatus::Busy as u8 == 0 {
            return Ok(());
        }
        // Small delay
        for _ in 0..100 {
            core::hint::spin_loop();
        }
    }
    Err("IDE drive timeout waiting for ready")
}

/// Wait for data request ready
unsafe fn wait_drq(channel: IdeChannel) -> Result<(), &'static str> {
    for _ in 0..1000 {
        let status = ide_read_register(channel, IdeRegister::StatusCommand);
        if status & IdeStatus::DataRequest as u8 != 0 {
            return Ok(());
        }
        if status & IdeStatus::Error as u8 != 0 {
            return Err("IDE drive error");
        }
        // Small delay
        for _ in 0..100 {
            core::hint::spin_loop();
        }
    }
    Err("IDE drive timeout waiting for data request")
}

/// Read data from the IDE data port
unsafe fn ide_read_data(channel: IdeChannel, buffer: &mut [u16]) {
    let port = channel.base_port() + IdeRegister::Data as u16;
    let mut port: PortReadOnly<u16> = PortReadOnly::new(port);
    
    for word in buffer.iter_mut() {
        *word = port.read();
    }
}

/// Write data to the IDE data port
unsafe fn ide_write_data(channel: IdeChannel, buffer: &[u16]) {
    let port = channel.base_port() + IdeRegister::Data as u16;
    let mut port: PortWriteOnly<u16> = PortWriteOnly::new(port);
    
    for &word in buffer.iter() {
        port.write(word);
    }
}

impl IdeController {
    /// Initialize the IDE controller and detect drives
    pub fn initialize(&self) {
        if self.initialized.load(Ordering::Relaxed) {
            return;
        }

        unsafe {
            // Disable interrupts during initialization
            ide_write_device_control(IdeChannel::Primary, 0x02);
            ide_write_device_control(IdeChannel::Secondary, 0x02);

            // Detect drives on both channels
            self.detect_drive(IdeChannel::Primary, IdeDrive::Master);
            self.detect_drive(IdeChannel::Primary, IdeDrive::Slave);
            self.detect_drive(IdeChannel::Secondary, IdeDrive::Master);
            self.detect_drive(IdeChannel::Secondary, IdeDrive::Slave);

            // Re-enable interrupts
            ide_write_device_control(IdeChannel::Primary, 0x00);
            ide_write_device_control(IdeChannel::Secondary, 0x00);
        }

        self.initialized.store(true, Ordering::Relaxed);
    }

    /// Detect and identify a specific drive
    unsafe fn detect_drive(&self, channel: IdeChannel, drive: IdeDrive) {
        // Select the drive
        ide_write_register(channel, IdeRegister::DriveSelect, drive.select_value(false));
        
        // Small delay for drive selection
        for _ in 0..400 {
            core::hint::spin_loop();
        }

        // Send identify command
        ide_write_register(channel, IdeRegister::StatusCommand, IdeCommand::Identify as u8);
        
        // Check if drive exists
        let status = ide_read_register(channel, IdeRegister::StatusCommand);
        if status == 0 || status == 0xFF {
            // No drive present
            return;
        }

        // Wait for drive to be ready
        if wait_ready(channel).is_err() {
            return;
        }

        // Check for IDENTIFY command completion
        let status = ide_read_register(channel, IdeRegister::StatusCommand);
        if status & IdeStatus::Error as u8 != 0 {
            // Drive might be ATAPI, skip for now
            return;
        }

        // Wait for data ready
        if wait_drq(channel).is_err() {
            return;
        }

        // Read identification data
        let mut identify_buffer = [0u16; 256];
        ide_read_data(channel, &mut identify_buffer);

        // Parse identification data
        let mut disk = self.get_disk_mut(channel, drive).lock();
        disk.present = true;

        // Check for LBA support (bit 9 of capabilities)
        disk.supports_lba = identify_buffer[49] & (1 << 9) != 0;

        // Check for LBA48 support (bit 10 of command set support)
        disk.supports_lba48 = identify_buffer[83] & (1 << 10) != 0;

        // Get total sectors
        if disk.supports_lba48 && identify_buffer[83] & (1 << 10) != 0 {
            // LBA48 addressable sectors (words 100-103)
            disk.total_sectors = (identify_buffer[103] as u64) << 48
                | (identify_buffer[102] as u64) << 32
                | (identify_buffer[101] as u64) << 16
                | identify_buffer[100] as u64;
        } else if disk.supports_lba {
            // LBA28 addressable sectors (words 60-61)
            disk.total_sectors = ((identify_buffer[61] as u64) << 16) | identify_buffer[60] as u64;
        } else {
            // CHS addressable sectors
            let cylinders = identify_buffer[1] as u64;
            let heads = identify_buffer[3] as u64;
            let sectors = identify_buffer[6] as u64;
            disk.total_sectors = cylinders * heads * sectors;
        }

        // Copy model string (words 27-46, needs byte swapping)
        for i in 0..20 {
            let word = identify_buffer[27 + i];
            disk.model[i * 2] = (word >> 8) as u8;
            disk.model[i * 2 + 1] = (word & 0xFF) as u8;
        }

        // Copy serial number (words 10-19, needs byte swapping)
        for i in 0..10 {
            let word = identify_buffer[10 + i];
            disk.serial[i * 2] = (word >> 8) as u8;
            disk.serial[i * 2 + 1] = (word & 0xFF) as u8;
        }

        // Log the detected drive
        crate::debug_info!("IDE: Detected {} {} - Model: {}, Sectors: {}, LBA: {}, LBA48: {}",
            match channel { IdeChannel::Primary => "Primary", IdeChannel::Secondary => "Secondary" },
            match drive { IdeDrive::Master => "Master", IdeDrive::Slave => "Slave" },
            disk.model_string().trim(),
            disk.total_sectors,
            disk.supports_lba,
            disk.supports_lba48
        );
    }

    /// Read sectors from a disk using PIO mode
    pub fn read_sectors(&self, channel: IdeChannel, drive: IdeDrive, lba: u64, count: u8, buffer: &mut [u8]) -> Result<(), &'static str> {
        if count == 0 || count > 128 {
            return Err("Invalid sector count");
        }

        if buffer.len() < (count as usize * 512) {
            return Err("Buffer too small");
        }

        let disk = self.get_disk_mut(channel, drive).lock();
        if !disk.is_present() {
            return Err("Disk not present");
        }

        if lba >= disk.total_sectors {
            return Err("LBA out of range");
        }

        let use_lba48 = disk.supports_lba48 && lba > 0x0FFFFFFF;
        drop(disk); // Release lock before I/O operations

        unsafe {
            // Wait for drive to be ready
            wait_ready(channel)?;

            // Select drive and addressing mode
            if use_lba48 {
                // LBA48 mode
                ide_write_register(channel, IdeRegister::DriveSelect, 
                    drive.select_value(true) | ((lba >> 24) & 0x0F) as u8);
                
                // Send high-order bytes
                ide_write_register(channel, IdeRegister::SectorCount, 0);
                ide_write_register(channel, IdeRegister::LbaLow, (lba >> 24) as u8);
                ide_write_register(channel, IdeRegister::LbaMid, (lba >> 32) as u8);
                ide_write_register(channel, IdeRegister::LbaHigh, (lba >> 40) as u8);
                
                // Send low-order bytes
                ide_write_register(channel, IdeRegister::SectorCount, count);
                ide_write_register(channel, IdeRegister::LbaLow, lba as u8);
                ide_write_register(channel, IdeRegister::LbaMid, (lba >> 8) as u8);
                ide_write_register(channel, IdeRegister::LbaHigh, (lba >> 16) as u8);
                
                // Send command
                ide_write_register(channel, IdeRegister::StatusCommand, IdeCommand::ReadPioExt as u8);
            } else {
                // LBA28 mode
                ide_write_register(channel, IdeRegister::DriveSelect,
                    drive.select_value(true) | ((lba >> 24) & 0x0F) as u8);
                
                ide_write_register(channel, IdeRegister::SectorCount, count);
                ide_write_register(channel, IdeRegister::LbaLow, lba as u8);
                ide_write_register(channel, IdeRegister::LbaMid, (lba >> 8) as u8);
                ide_write_register(channel, IdeRegister::LbaHigh, (lba >> 16) as u8);
                
                // Send command
                ide_write_register(channel, IdeRegister::StatusCommand, IdeCommand::ReadPio as u8);
            }

            // Read sectors
            let mut word_buffer = [0u16; 256];
            for sector in 0..count {
                // Wait for data ready
                wait_drq(channel)?;

                // Read 256 words (512 bytes)
                ide_read_data(channel, &mut word_buffer);

                // Copy to output buffer
                let offset = sector as usize * 512;
                for (i, &word) in word_buffer.iter().enumerate() {
                    buffer[offset + i * 2] = word as u8;
                    buffer[offset + i * 2 + 1] = (word >> 8) as u8;
                }
            }

            // Wait for completion
            wait_ready(channel)?;
        }

        Ok(())
    }

    /// Write sectors to a disk using PIO mode
    pub fn write_sectors(&self, channel: IdeChannel, drive: IdeDrive, lba: u64, count: u8, buffer: &[u8]) -> Result<(), &'static str> {
        if count == 0 || count > 128 {
            return Err("Invalid sector count");
        }

        if buffer.len() < (count as usize * 512) {
            return Err("Buffer too small");
        }

        let disk = self.get_disk_mut(channel, drive).lock();
        if !disk.is_present() {
            return Err("Disk not present");
        }

        if lba >= disk.total_sectors {
            return Err("LBA out of range");
        }

        let use_lba48 = disk.supports_lba48 && lba > 0x0FFFFFFF;
        drop(disk); // Release lock before I/O operations

        unsafe {
            // Wait for drive to be ready
            wait_ready(channel)?;

            // Select drive and addressing mode
            if use_lba48 {
                // LBA48 mode
                ide_write_register(channel, IdeRegister::DriveSelect,
                    drive.select_value(true) | ((lba >> 24) & 0x0F) as u8);
                
                // Send high-order bytes
                ide_write_register(channel, IdeRegister::SectorCount, 0);
                ide_write_register(channel, IdeRegister::LbaLow, (lba >> 24) as u8);
                ide_write_register(channel, IdeRegister::LbaMid, (lba >> 32) as u8);
                ide_write_register(channel, IdeRegister::LbaHigh, (lba >> 40) as u8);
                
                // Send low-order bytes
                ide_write_register(channel, IdeRegister::SectorCount, count);
                ide_write_register(channel, IdeRegister::LbaLow, lba as u8);
                ide_write_register(channel, IdeRegister::LbaMid, (lba >> 8) as u8);
                ide_write_register(channel, IdeRegister::LbaHigh, (lba >> 16) as u8);
                
                // Send command
                ide_write_register(channel, IdeRegister::StatusCommand, IdeCommand::WritePioExt as u8);
            } else {
                // LBA28 mode
                ide_write_register(channel, IdeRegister::DriveSelect,
                    drive.select_value(true) | ((lba >> 24) & 0x0F) as u8);
                
                ide_write_register(channel, IdeRegister::SectorCount, count);
                ide_write_register(channel, IdeRegister::LbaLow, lba as u8);
                ide_write_register(channel, IdeRegister::LbaMid, (lba >> 8) as u8);
                ide_write_register(channel, IdeRegister::LbaHigh, (lba >> 16) as u8);
                
                // Send command
                ide_write_register(channel, IdeRegister::StatusCommand, IdeCommand::WritePio as u8);
            }

            // Write sectors
            let mut word_buffer = [0u16; 256];
            for sector in 0..count {
                // Wait for data ready
                wait_drq(channel)?;

                // Prepare 256 words (512 bytes) from buffer
                let offset = sector as usize * 512;
                for i in 0..256 {
                    word_buffer[i] = buffer[offset + i * 2] as u16
                        | ((buffer[offset + i * 2 + 1] as u16) << 8);
                }

                // Write data
                ide_write_data(channel, &word_buffer);
            }

            // Wait for completion
            wait_ready(channel)?;

            // Cache flush (if supported)
            ide_write_register(channel, IdeRegister::StatusCommand, 0xE7);
            wait_ready(channel)?;
        }

        Ok(())
    }
}

/// Wrapper struct for IDE disk that implements BlockDevice trait
pub struct IdeBlockDevice {
    channel: IdeChannel,
    drive: IdeDrive,
}

impl IdeBlockDevice {
    /// Create a new IDE block device
    pub fn new(channel: IdeChannel, drive: IdeDrive) -> Self {
        Self { channel, drive }
    }
}

impl BlockDevice for IdeBlockDevice {
    fn read_blocks(&self, block: u64, count: u32, buffer: &mut [u8]) -> Result<(), &'static str> {
        // IDE read_sectors takes count as u8, so we need to split large reads
        let mut remaining = count;
        let mut current_lba = block;
        let mut buffer_offset = 0;

        while remaining > 0 {
            let sectors_to_read = core::cmp::min(remaining, 128) as u8;
            let end_offset = buffer_offset + (sectors_to_read as usize * 512);
            
            IDE_CONTROLLER.read_sectors(
                self.channel,
                self.drive,
                current_lba,
                sectors_to_read,
                &mut buffer[buffer_offset..end_offset]
            )?;

            remaining -= sectors_to_read as u32;
            current_lba += sectors_to_read as u64;
            buffer_offset = end_offset;
        }

        Ok(())
    }

    fn write_blocks(&self, block: u64, count: u32, buffer: &[u8]) -> Result<(), &'static str> {
        // IDE write_sectors takes count as u8, so we need to split large writes
        let mut remaining = count;
        let mut current_lba = block;
        let mut buffer_offset = 0;

        while remaining > 0 {
            let sectors_to_write = core::cmp::min(remaining, 128) as u8;
            let end_offset = buffer_offset + (sectors_to_write as usize * 512);
            
            IDE_CONTROLLER.write_sectors(
                self.channel,
                self.drive,
                current_lba,
                sectors_to_write,
                &buffer[buffer_offset..end_offset]
            )?;

            remaining -= sectors_to_write as u32;
            current_lba += sectors_to_write as u64;
            buffer_offset = end_offset;
        }

        Ok(())
    }

    fn block_size(&self) -> u32 {
        512 // Standard sector size for IDE/ATA drives
    }

    fn total_blocks(&self) -> u64 {
        let disk = IDE_CONTROLLER.get_disk_mut(self.channel, self.drive).lock();
        disk.total_sectors
    }

    fn name(&self) -> &str {
        match (self.channel, self.drive) {
            (IdeChannel::Primary, IdeDrive::Master) => "hda",
            (IdeChannel::Primary, IdeDrive::Slave) => "hdb",
            (IdeChannel::Secondary, IdeDrive::Master) => "hdc",
            (IdeChannel::Secondary, IdeDrive::Slave) => "hdd",
        }
    }
}