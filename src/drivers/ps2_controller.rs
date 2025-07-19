use x86_64::instructions::port::Port;
use crate::debug_info;

/// Initialize the PS/2 controller for keyboard and mouse support
pub fn init() {
    debug_info!("Initializing PS/2 controller for keyboard...");
    
    unsafe {
        // Disable devices while configuring
        wait_ps2_write();
        Port::<u8>::new(0x64).write(0xAD); // Disable keyboard
        wait_ps2_write();
        Port::<u8>::new(0x64).write(0xA7); // Disable mouse
        
        // Flush output buffer
        while (Port::<u8>::new(0x64).read() & 0x01) != 0 {
            let _ = Port::<u8>::new(0x60).read();
        }
        
        // Read controller configuration byte
        wait_ps2_write();
        Port::<u8>::new(0x64).write(0x20);
        wait_ps2_read();
        let config = Port::<u8>::new(0x60).read();
        debug_info!("PS/2 controller config: 0x{:02x}", config);
        
        // Enable keyboard interrupt (bit 0), mouse interrupt (bit 1), and clear translation (bit 6)
        let new_config = (config | 0x03) & !0x40;
        
        // Write back configuration
        wait_ps2_write();
        Port::<u8>::new(0x64).write(0x60);
        wait_ps2_write();
        Port::<u8>::new(0x60).write(new_config);
        debug_info!("PS/2 controller config updated to: 0x{:02x}", new_config);
        
        // Enable keyboard
        wait_ps2_write();
        Port::<u8>::new(0x64).write(0xAE);
        debug_info!("Keyboard enabled");
        
        // Reset keyboard
        wait_ps2_write();
        Port::<u8>::new(0x60).write(0xFF);
        wait_ps2_read();
        let reset_response = Port::<u8>::new(0x60).read();
        debug_info!("Keyboard reset response: 0x{:02x}", reset_response);
        
        // If we got ACK, wait for self-test result
        if reset_response == 0xFA {
            wait_ps2_read();
            let self_test = Port::<u8>::new(0x60).read();
            debug_info!("Keyboard self-test result: 0x{:02x}", self_test);
        }
    }
}

/// Wait for PS/2 controller to be ready for writing
fn wait_ps2_write() {
    unsafe {
        let mut status_port = Port::<u8>::new(0x64);
        for _ in 0..100000 {
            if (status_port.read() & 0x02) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }
}

/// Wait for PS/2 controller to have data ready for reading
fn wait_ps2_read() {
    unsafe {
        let mut status_port = Port::<u8>::new(0x64);
        for _ in 0..100000 {
            if (status_port.read() & 0x01) != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }
}