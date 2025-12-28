//! PCI Bus Driver
//!
//! Provides PCI configuration space access and device enumeration.
//! Uses legacy I/O ports 0xCF8 (address) and 0xCFC (data).

use x86_64::instructions::port::Port;
use alloc::vec::Vec;
use crate::debug_info;

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// PCI device identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

impl PciDevice {
    /// Read a 32-bit value from PCI configuration space
    pub fn read_config(&self, offset: u8) -> u32 {
        pci_config_read(self.bus, self.device, self.function, offset)
    }

    /// Write a 32-bit value to PCI configuration space
    pub fn write_config(&self, offset: u8, value: u32) {
        pci_config_write(self.bus, self.device, self.function, offset, value);
    }

    /// Read a Base Address Register (BAR)
    pub fn read_bar(&self, bar_index: u8) -> Option<Bar> {
        if bar_index > 5 {
            return None;
        }

        let offset = 0x10 + (bar_index * 4);
        let bar_value = self.read_config(offset);

        if bar_value == 0 {
            return None;
        }

        if bar_value & 0x01 != 0 {
            // I/O Space BAR
            Some(Bar::Io {
                port: (bar_value & 0xFFFC) as u16,
            })
        } else {
            // Memory Space BAR
            let prefetchable = (bar_value & 0x08) != 0;
            let bar_type = (bar_value >> 1) & 0x03;

            let address = match bar_type {
                0 => (bar_value & 0xFFFFFFF0) as u64,
                2 => {
                    // 64-bit BAR
                    let high = self.read_config(offset + 4) as u64;
                    ((high << 32) | (bar_value & 0xFFFFFFF0) as u64)
                }
                _ => return None,
            };

            // Determine size by writing all 1s and reading back
            self.write_config(offset, 0xFFFFFFFF);
            let size_mask = self.read_config(offset);
            self.write_config(offset, bar_value); // Restore original value

            let size = if size_mask != 0 {
                (!(size_mask & 0xFFFFFFF0)).wrapping_add(1) as u64
            } else {
                0
            };

            Some(Bar::Memory {
                address,
                size,
                prefetchable,
                is_64bit: bar_type == 2,
            })
        }
    }

    /// Enable bus mastering for this device
    pub fn enable_bus_master(&self) {
        let command = self.read_config(0x04);
        self.write_config(0x04, command | 0x04);
    }

    /// Enable memory space access for this device
    pub fn enable_memory_space(&self) {
        let command = self.read_config(0x04);
        self.write_config(0x04, command | 0x02);
    }

    /// Enable I/O space access for this device
    pub fn enable_io_space(&self) {
        let command = self.read_config(0x04);
        self.write_config(0x04, command | 0x01);
    }
}

/// PCI Base Address Register types
#[derive(Debug, Clone, Copy)]
pub enum Bar {
    Memory {
        address: u64,
        size: u64,
        prefetchable: bool,
        is_64bit: bool,
    },
    Io {
        port: u16,
    },
}

/// Build a PCI configuration address
fn pci_config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
        | 0x80000000 // Enable bit
}

/// Read from PCI configuration space
pub fn pci_config_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = pci_config_address(bus, device, function, offset);

    unsafe {
        let mut address_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

        address_port.write(address);
        data_port.read()
    }
}

/// Write to PCI configuration space
pub fn pci_config_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = pci_config_address(bus, device, function, offset);

    unsafe {
        let mut address_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

        address_port.write(address);
        data_port.write(value);
    }
}

/// Check if a device exists at the given bus/device/function
fn device_exists(bus: u8, device: u8, function: u8) -> bool {
    let vendor_id = pci_config_read(bus, device, function, 0) & 0xFFFF;
    vendor_id != 0xFFFF
}

/// Read device info from PCI configuration space
fn read_device_info(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    if !device_exists(bus, device, function) {
        return None;
    }

    let id_reg = pci_config_read(bus, device, function, 0x00);
    let vendor_id = (id_reg & 0xFFFF) as u16;
    let device_id = ((id_reg >> 16) & 0xFFFF) as u16;

    let class_reg = pci_config_read(bus, device, function, 0x08);
    let class_code = ((class_reg >> 24) & 0xFF) as u8;
    let subclass = ((class_reg >> 16) & 0xFF) as u8;
    let prog_if = ((class_reg >> 8) & 0xFF) as u8;

    let header_reg = pci_config_read(bus, device, function, 0x0C);
    let header_type = ((header_reg >> 16) & 0xFF) as u8;

    let int_reg = pci_config_read(bus, device, function, 0x3C);
    let interrupt_line = (int_reg & 0xFF) as u8;
    let interrupt_pin = ((int_reg >> 8) & 0xFF) as u8;

    Some(PciDevice {
        bus,
        device,
        function,
        vendor_id,
        device_id,
        class_code,
        subclass,
        prog_if,
        header_type,
        interrupt_line,
        interrupt_pin,
    })
}

/// Scan all PCI buses and return a list of devices
pub fn enumerate_devices() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for bus in 0..=255u8 {
        for device in 0..32u8 {
            // Check function 0
            if let Some(dev) = read_device_info(bus, device, 0) {
                debug_info!(
                    "PCI {:02x}:{:02x}.{}: {:04x}:{:04x} class={:02x}:{:02x}",
                    bus, device, 0,
                    dev.vendor_id, dev.device_id,
                    dev.class_code, dev.subclass
                );

                let is_multifunction = (dev.header_type & 0x80) != 0;
                devices.push(dev);

                // Check other functions if multifunction device
                if is_multifunction {
                    for function in 1..8u8 {
                        if let Some(dev) = read_device_info(bus, device, function) {
                            debug_info!(
                                "PCI {:02x}:{:02x}.{}: {:04x}:{:04x} class={:02x}:{:02x}",
                                bus, device, function,
                                dev.vendor_id, dev.device_id,
                                dev.class_code, dev.subclass
                            );
                            devices.push(dev);
                        }
                    }
                }
            }
        }
    }

    devices
}

/// Find all devices with the given vendor and device ID
pub fn find_devices(vendor_id: u16, device_id: u16) -> Vec<PciDevice> {
    enumerate_devices()
        .into_iter()
        .filter(|d| d.vendor_id == vendor_id && d.device_id == device_id)
        .collect()
}

/// Find all devices with the given class code
pub fn find_devices_by_class(class_code: u8, subclass: u8) -> Vec<PciDevice> {
    enumerate_devices()
        .into_iter()
        .filter(|d| d.class_code == class_code && d.subclass == subclass)
        .collect()
}

// VirtIO vendor ID
pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

// VirtIO device IDs (transitional)
pub const VIRTIO_DEVICE_INPUT: u16 = 0x1052;

/// Find VirtIO input devices
pub fn find_virtio_input_devices() -> Vec<PciDevice> {
    enumerate_devices()
        .into_iter()
        .filter(|d| d.vendor_id == VIRTIO_VENDOR_ID && d.device_id == VIRTIO_DEVICE_INPUT)
        .collect()
}
