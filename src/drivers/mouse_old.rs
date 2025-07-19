use spin::Mutex;
use lazy_static::lazy_static;
use crate::debug_info;
use x86_64::instructions::port::Port;

const MOUSE_DATA_PORT: u16 = 0x60;
const MOUSE_STATUS_PORT: u16 = 0x64;
const MOUSE_COMMAND_PORT: u16 = 0x64;

const MOUSE_ENABLE_AUXILIARY: u8 = 0xA8;
const MOUSE_SET_DEFAULTS: u8 = 0xF6;
const MOUSE_ENABLE_DATA_REPORTING: u8 = 0xF4;
const MOUSE_SET_SAMPLE_RATE: u8 = 0xF3;
const MOUSE_GET_DEVICE_ID: u8 = 0xF2;
const COMMAND_WRITE_MOUSE: u8 = 0xD4;
const GET_COMPAQ_STATUS: u8 = 0x20;
const SET_COMPAQ_STATUS: u8 = 0x60;

lazy_static! {
    static ref MOUSE_STATE: Mutex<MouseState> = Mutex::new(MouseState::new());
}

#[derive(Debug, Clone, Copy)]
pub struct MouseState {
    x: i32,
    y: i32,
    buttons: u8,
    packet_index: u8,
    packet: [u8; 4],
    wheel_support: bool,
}

impl MouseState {
    const fn new() -> Self {
        Self {
            x: 400,  // Start in center of 800x600 screen
            y: 300,
            buttons: 0,
            packet_index: 0,
            packet: [0; 4],
            wheel_support: false,
        }
    }
}

pub fn init() {
    debug_info!("Initializing PS/2 mouse driver...");
    
    unsafe {
        // First, disable devices while we configure
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout on initial write");
            return;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(0xAD); // Disable keyboard
        
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout disabling mouse");
            return;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(0xA7); // Disable mouse
        
        // Flush output buffer
        let mut timeout = 10000;
        while timeout > 0 {
            let status = Port::<u8>::new(MOUSE_STATUS_PORT).read();
            if (status & 0x01) != 0 {
                // Data available, read and discard
                let _ = Port::<u8>::new(MOUSE_DATA_PORT).read();
            } else {
                break;
            }
            timeout -= 1;
        }
        
        // Enable auxiliary device (mouse)
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout waiting to write enable command");
            return;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(MOUSE_ENABLE_AUXILIARY);
        
        // Enable keyboard again
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout enabling keyboard");
            return;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(0xAE); // Enable keyboard
        
        // Get the current compaq status byte
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout waiting to write status command");
            return;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(GET_COMPAQ_STATUS);
        
        // Check controller status before trying to read
        let ctrl_status = Port::<u8>::new(MOUSE_STATUS_PORT).read();
        debug_info!("Controller status after GET_COMPAQ_STATUS: 0x{:02x}", ctrl_status);
        
        if !wait_for_read() {
            debug_info!("Mouse init failed: timeout waiting to read status");
            debug_info!("Final controller status: 0x{:02x}", Port::<u8>::new(MOUSE_STATUS_PORT).read());
            return;
        }
        let mut status = Port::<u8>::new(MOUSE_DATA_PORT).read();
        debug_info!("Compaq status byte: 0x{:02x}", status);
        
        // Enable mouse interrupt (IRQ12) by setting bit 1
        status |= 0x02;
        // Enable mouse by clearing bit 5
        status &= !0x20;
        
        // Write back the modified status
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout waiting to write modified status command");
            return;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(SET_COMPAQ_STATUS);
        if !wait_for_write() {
            debug_info!("Mouse init failed: timeout waiting to write modified status");
            return;
        }
        Port::<u8>::new(MOUSE_DATA_PORT).write(status);
        
        // Try to enable scroll wheel (IntelliMouse)
        // These commands may fail on some systems, so we don't check return values
        let _ = send_mouse_command(MOUSE_SET_SAMPLE_RATE);
        let _ = send_mouse_data(200);
        let _ = send_mouse_command(MOUSE_SET_SAMPLE_RATE);
        let _ = send_mouse_data(100);
        let _ = send_mouse_command(MOUSE_SET_SAMPLE_RATE);
        let _ = send_mouse_data(80);
        
        // Get device ID to check if scroll wheel is supported
        if send_mouse_command(MOUSE_GET_DEVICE_ID) {
            if let Some(device_id) = read_mouse_data() {
                if device_id == 3 {
                    debug_info!("Mouse with scroll wheel detected");
                    MOUSE_STATE.lock().wheel_support = true;
                } else {
                    debug_info!("Standard PS/2 mouse detected (ID: {})", device_id);
                }
            }
        }
        
        // Reset to defaults - try simplified approach for QEMU
        if !wait_for_write() {
            debug_info!("Warning: Timeout before mouse reset");
        } else {
            Port::<u8>::new(MOUSE_COMMAND_PORT).write(COMMAND_WRITE_MOUSE);
            if wait_for_write() {
                Port::<u8>::new(MOUSE_DATA_PORT).write(MOUSE_SET_DEFAULTS);
                // Don't wait for ACK on reset, it might take time
            }
        }
        
        // Small delay for reset to complete
        for _ in 0..100000 {
            core::hint::spin_loop();
        }
        
        // Enable data reporting - simplified
        if !wait_for_write() {
            debug_info!("Warning: Timeout before enabling data reporting");
        } else {
            Port::<u8>::new(MOUSE_COMMAND_PORT).write(COMMAND_WRITE_MOUSE);
            if wait_for_write() {
                Port::<u8>::new(MOUSE_DATA_PORT).write(MOUSE_ENABLE_DATA_REPORTING);
                // Wait for ACK
                if wait_for_read() {
                    let ack = Port::<u8>::new(MOUSE_DATA_PORT).read();
                    if ack == 0xFA {
                        debug_info!("Mouse data reporting enabled successfully");
                    } else {
                        debug_info!("Unexpected response from mouse: 0x{:02x}", ack);
                    }
                }
            }
        }
        
        debug_info!("PS/2 mouse initialization completed");
    }
}

fn wait_for_read() -> bool {
    unsafe {
        let mut status_port = Port::<u8>::new(MOUSE_STATUS_PORT);
        let mut timeout = 100000;
        while (status_port.read() & 0x01) == 0 {
            if timeout == 0 {
                return false;
            }
            timeout -= 1;
            core::hint::spin_loop();
        }
        true
    }
}

fn wait_for_write() -> bool {
    unsafe {
        let mut status_port = Port::<u8>::new(MOUSE_STATUS_PORT);
        let mut timeout = 100000;
        while (status_port.read() & 0x02) != 0 {
            if timeout == 0 {
                return false;
            }
            timeout -= 1;
            core::hint::spin_loop();
        }
        true
    }
}

fn send_mouse_command(command: u8) -> bool {
    unsafe {
        if !wait_for_write() {
            return false;
        }
        Port::<u8>::new(MOUSE_COMMAND_PORT).write(COMMAND_WRITE_MOUSE);
        if !wait_for_write() {
            return false;
        }
        Port::<u8>::new(MOUSE_DATA_PORT).write(command);
        
        // Wait for ACK
        if !wait_for_read() {
            return false;
        }
        let ack = Port::<u8>::new(MOUSE_DATA_PORT).read();
        ack == 0xFA // ACK byte
    }
}

fn send_mouse_data(data: u8) -> bool {
    unsafe {
        if !wait_for_write() {
            return false;
        }
        Port::<u8>::new(MOUSE_DATA_PORT).write(data);
        
        // Wait for ACK
        if !wait_for_read() {
            return false;
        }
        let ack = Port::<u8>::new(MOUSE_DATA_PORT).read();
        ack == 0xFA // ACK byte
    }
}

fn read_mouse_data() -> Option<u8> {
    unsafe {
        if !wait_for_read() {
            return None;
        }
        Some(Port::<u8>::new(MOUSE_DATA_PORT).read())
    }
}

pub fn handle_mouse_interrupt(data: u8) {
    let mut state = MOUSE_STATE.lock();
    
    let packet_index = state.packet_index as usize;
    state.packet[packet_index] = data;
    state.packet_index += 1;
    
    let expected_packets = if state.wheel_support { 4 } else { 3 };
    
    if state.packet_index >= expected_packets {
        process_mouse_packet(&mut state);
        state.packet_index = 0;
    }
}

fn process_mouse_packet(state: &mut MouseState) {
    let flags = state.packet[0];
    
    // Check if packet is valid
    if (flags & 0x08) == 0 {
        // Invalid packet, reset
        state.packet_index = 0;
        return;
    }
    
    // Extract button states
    state.buttons = flags & 0x07; // Left, Right, Middle buttons
    
    // Extract movement data
    let mut x_movement = state.packet[1] as i16;
    let mut y_movement = state.packet[2] as i16;
    
    // Sign extend if negative
    if (flags & 0x10) != 0 {
        x_movement |= 0xFF00u16 as i16;
    }
    if (flags & 0x20) != 0 {
        y_movement |= 0xFF00u16 as i16;
    }
    
    // Update position
    state.x = (state.x + x_movement as i32).clamp(0, 799);
    state.y = (state.y - y_movement as i32).clamp(0, 599); // Y is inverted
    
    // Handle scroll wheel if supported
    let wheel = if state.wheel_support {
        state.packet[3] as i8
    } else {
        0
    };
    
    debug_info!("Mouse: x={}, y={}, buttons={:03b}, wheel={}", 
        state.x, state.y, state.buttons, wheel);
}

pub fn get_position() -> (i32, i32) {
    let state = MOUSE_STATE.lock();
    (state.x, state.y)
}

pub fn get_buttons() -> u8 {
    MOUSE_STATE.lock().buttons
}

pub fn is_left_pressed() -> bool {
    (get_buttons() & 0x01) != 0
}

pub fn is_right_pressed() -> bool {
    (get_buttons() & 0x02) != 0
}

pub fn is_middle_pressed() -> bool {
    (get_buttons() & 0x04) != 0
}