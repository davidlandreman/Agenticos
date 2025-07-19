use spin::Mutex;
use lazy_static::lazy_static;
use crate::debug_info;
use crate::debug_trace;
use crate::debug_debug;
use x86_64::instructions::port::Port;

const MOUSE_DATA_PORT: u16 = 0x60;
const MOUSE_STATUS_PORT: u16 = 0x64;
const MOUSE_COMMAND_PORT: u16 = 0x64;

// PS/2 Controller Commands
const ENABLE_AUX_DEVICE: u8 = 0xA8;
const DISABLE_AUX_DEVICE: u8 = 0xA7;
const TEST_AUX_PORT: u8 = 0xA9;
const ENABLE_KEYBOARD: u8 = 0xAE;
const DISABLE_KEYBOARD: u8 = 0xAD;

// Mouse Commands (sent via 0xD4 prefix)
const WRITE_TO_AUX: u8 = 0xD4;
const MOUSE_RESET: u8 = 0xFF;
const MOUSE_ENABLE: u8 = 0xF4;
const MOUSE_SET_DEFAULTS: u8 = 0xF6;

// Expected responses
const ACK: u8 = 0xFA;
const MOUSE_ID: u8 = 0x00;

lazy_static! {
    static ref MOUSE_DATA: Mutex<MouseData> = Mutex::new(MouseData::new());
}

#[derive(Debug)]
struct MouseData {
    x: i32,
    y: i32,
    buttons: u8,
    packet_bytes: [u8; 3],
    packet_index: usize,
}

impl MouseData {
    const fn new() -> Self {
        Self {
            x: 400,
            y: 300,
            buttons: 0,
            packet_bytes: [0; 3],
            packet_index: 0,
        }
    }
}

pub fn init() {
    debug_info!("Starting simplified PS/2 mouse initialization...");
    
    // Skip PS/2 controller configuration here - it's already done in kernel init
    // This prevents conflicts with keyboard initialization
    
    // Wait for controller to be ready
    wait_controller();
    
    // Skip enabling auxiliary device - already done in PS/2 controller init
    // The mouse may already be sending data at this point
    
    // Clear any pending data before reset
    let mut cleared_count = 0;
    while read_data_timeout().is_some() {
        cleared_count += 1;
    }
    if cleared_count > 0 {
        debug_info!("Cleared {} pending bytes from mouse", cleared_count);
    }
    
    // Reset mouse - this may fail if mouse is already active
    debug_info!("Attempting to reset mouse...");
    if !send_mouse_command(MOUSE_RESET) {
        debug_info!("Mouse reset command failed - mouse may already be active");
        // Try to just enable data reporting
        if send_mouse_command(MOUSE_ENABLE) {
            debug_info!("Mouse data reporting enabled (without reset)");
            debug_info!("PS/2 mouse initialization completed (already active)");
            return;
        } else {
            debug_info!("Failed to enable mouse data reporting");
            return;
        }
    }
        
        // Wait for reset to complete and read response
        // Mouse sends 0xFA (ACK) then 0xAA (self-test passed) then 0x00 (mouse ID)
        let mut got_bat = false;
        
        for _ in 0..3 {
            if let Some(byte) = read_data_timeout_long() {
                match byte {
                    0xFA => {
                        debug_info!("Got ACK from mouse");
                    }
                    0xAA => {
                        debug_info!("Mouse self-test passed");
                        got_bat = true;
                    }
                    0x00 => {
                        debug_info!("Got mouse ID");
                    }
                    _ => debug_info!("Unexpected byte during reset: 0x{:02x}", byte),
                }
            }
        }
        
        if !got_bat {
            debug_info!("Warning: Mouse didn't complete self-test");
        }
        
        // Set defaults
        if send_mouse_command(MOUSE_SET_DEFAULTS) {
            debug_info!("Mouse defaults set");
        }
        
        // Enable data reporting
        if send_mouse_command(MOUSE_ENABLE) {
            debug_info!("Mouse data reporting enabled");
        } else {
            debug_info!("Failed to enable mouse data reporting");
            return;
        }
        
        debug_info!("PS/2 mouse initialization completed successfully");
}

fn wait_controller() {
    unsafe {
        let mut port = Port::<u8>::new(MOUSE_STATUS_PORT);
        for _ in 0..100000 {
            let status = port.read();
            if (status & 0x02) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }
}

fn send_controller_command(cmd: u8) {
    unsafe {
        wait_controller();
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(cmd);
    }
}

fn send_mouse_command(cmd: u8) -> bool {
    unsafe {
        // Send "write to auxiliary device" command
        wait_controller();
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(WRITE_TO_AUX);
        
        // Send the actual mouse command
        wait_controller();
        Port::<u8>::new(MOUSE_DATA_PORT).write(cmd);
        
        // Wait for ACK
        if let Some(response) = read_data_timeout() {
            if response == ACK {
                return true;
            } else {
                debug_info!("Mouse command 0x{:02x} got response 0x{:02x} instead of ACK", cmd, response);
            }
        } else {
            debug_info!("Mouse command 0x{:02x} timed out waiting for ACK", cmd);
        }
        false
    }
}

fn read_data_timeout() -> Option<u8> {
    unsafe {
        let mut status_port = Port::<u8>::new(MOUSE_STATUS_PORT);
        let mut data_port = Port::<u8>::new(MOUSE_DATA_PORT);
        
        for _ in 0..100000 {
            let status = status_port.read();
            if (status & 0x01) != 0 {
                return Some(data_port.read());
            }
            core::hint::spin_loop();
        }
        None
    }
}

fn read_data_timeout_long() -> Option<u8> {
    unsafe {
        let mut status_port = Port::<u8>::new(MOUSE_STATUS_PORT);
        let mut data_port = Port::<u8>::new(MOUSE_DATA_PORT);
        
        for _ in 0..1000000 {
            let status = status_port.read();
            if (status & 0x01) != 0 {
                return Some(data_port.read());
            }
            core::hint::spin_loop();
        }
        None
    }
}

pub fn handle_interrupt(data: u8) {
    let mut mouse_data = MOUSE_DATA.lock();
    
    // If we're expecting the first byte of a packet, validate it
    if mouse_data.packet_index == 0 {
        // Check if this could be a valid first byte (bit 3 must be set)
        if (data & 0x08) == 0 {
            debug_trace!("Invalid first byte 0x{:02x}, skipping", data);
            return;
        }
    }
    
    let packet_index = mouse_data.packet_index;
    mouse_data.packet_bytes[packet_index] = data;
    mouse_data.packet_index += 1;
    
    debug_trace!("Mouse packet byte {} of 3: 0x{:02x}", mouse_data.packet_index, data);
    
    if mouse_data.packet_index >= 3 {
        debug_trace!("Processing complete mouse packet: 0x{:02x} 0x{:02x} 0x{:02x}", 
            mouse_data.packet_bytes[0], 
            mouse_data.packet_bytes[1], 
            mouse_data.packet_bytes[2]);
        process_packet(&mut mouse_data);
        mouse_data.packet_index = 0;
    }
}

fn process_packet(data: &mut MouseData) {
    let byte1 = data.packet_bytes[0];
    let byte2 = data.packet_bytes[1];
    let byte3 = data.packet_bytes[2];
    
    // Extract button states
    let old_buttons = data.buttons;
    data.buttons = byte1 & 0x07;
    
    // Log button changes
    if data.buttons != old_buttons {
        let left = if data.buttons & 0x01 != 0 { "pressed" } else { "released" };
        let right = if data.buttons & 0x02 != 0 { "pressed" } else { "released" };
        let middle = if data.buttons & 0x04 != 0 { "pressed" } else { "released" };
        debug_info!("Mouse buttons changed: left={}, right={}, middle={}", left, right, middle);
    }
    
    // Extract X and Y movement (with sign extension)
    let x_delta = if (byte1 & 0x10) != 0 {
        // Negative X
        byte2 as i16 | 0xFF00u16 as i16
    } else {
        byte2 as i16
    };
    
    let y_delta = if (byte1 & 0x20) != 0 {
        // Negative Y
        byte3 as i16 | 0xFF00u16 as i16
    } else {
        byte3 as i16
    };
    
    // Update position (Y is inverted in PS/2)
    let old_x = data.x;
    let old_y = data.y;
    data.x = (data.x + x_delta as i32).clamp(0, 1279);
    data.y = (data.y - y_delta as i32).clamp(0, 719);
    
    // Only log if position actually changed
    if data.x != old_x || data.y != old_y {
        debug_trace!("Mouse moved: ({}, {}) -> ({}, {}), buttons={:03b}", 
            old_x, old_y, data.x, data.y, data.buttons);
    }
}

pub fn get_state() -> (i32, i32, u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let data = MOUSE_DATA.lock();
        (data.x, data.y, data.buttons)
    })
}