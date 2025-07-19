use spin::Mutex;
use lazy_static::lazy_static;
use crate::{debug_info, debug_trace, debug_error, debug_debug, print};

const SCANCODE_QUEUE_SIZE: usize = 100;

lazy_static! {
    static ref SCANCODE_QUEUE: Mutex<ScancodeQueue> = Mutex::new(ScancodeQueue::new());
    static ref KEYBOARD_STATE: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());
}

struct KeyboardState {
    is_break_code: bool,
    is_extended: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            is_break_code: false,
            is_extended: false,
        }
    }
}

struct ScancodeQueue {
    data: [u8; SCANCODE_QUEUE_SIZE],
    read_index: usize,
    write_index: usize,
}

impl ScancodeQueue {
    const fn new() -> Self {
        Self {
            data: [0; SCANCODE_QUEUE_SIZE],
            read_index: 0,
            write_index: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.read_index == self.write_index
    }

    fn push(&mut self, scancode: u8) -> Result<(), ()> {
        let next_write = (self.write_index + 1) % SCANCODE_QUEUE_SIZE;
        
        if next_write == self.read_index {
            return Err(());
        }
        
        self.data[self.write_index] = scancode;
        self.write_index = next_write;
        Ok(())
    }

    fn pop(&mut self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        
        let scancode = self.data[self.read_index];
        self.read_index = (self.read_index + 1) % SCANCODE_QUEUE_SIZE;
        Some(scancode)
    }
}

pub(crate) fn add_scancode(scancode: u8) {
    // Note: This is called from interrupt context
    // We should ONLY queue the scancode here, not process it
    // Processing (including printing) should happen outside interrupt context
    if let Err(()) = SCANCODE_QUEUE.lock().push(scancode) {
        debug_error!("WARNING: Keyboard scancode queue full; dropping input");
    }
}

fn process_scancode(scancode: u8) {
    // Note: This is called from interrupt context, but KEYBOARD_STATE is only accessed here
    // and in interrupt context, so we don't need without_interrupts
    let mut state = KEYBOARD_STATE.lock();
    
    // Handle special codes
    match scancode {
        0xF0 => {
            // Break code prefix in Set 2
            state.is_break_code = true;
            return;
        }
        0xE0 => {
            // Extended key prefix
            state.is_extended = true;
            return;
        }
        _ => {}
    }
    
    // If this is a break code (key release), don't process it
    if state.is_break_code {
        state.is_break_code = false;
        state.is_extended = false;
        return;
    }
    
    // Only process make codes (key press)
    if let Some(character) = scancode_set2_to_ascii(scancode) {
        debug_trace!("Keyboard: converted scancode 0x{:02X} to character '{}'", scancode, character);
        
        // Echo character immediately for real-time feedback
        crate::print!("{}", character);
        
        // Route input to active process stdin buffer
        crate::process::push_keyboard_input(character);
    } else {
        debug_debug!("Keyboard: scancode 0x{:02X} did not convert to character", scancode);
    }
    
    state.is_extended = false;
}

fn scancode_to_ascii(scancode: u8) -> Option<char> {
    let character = match scancode {
        0x01 => return None, // Escape
        0x02 => '1',
        0x03 => '2',
        0x04 => '3',
        0x05 => '4',
        0x06 => '5',
        0x07 => '6',
        0x08 => '7',
        0x09 => '8',
        0x0A => '9',
        0x0B => '0',
        0x0C => '-',
        0x0D => '=',
        0x0E => return None, // Backspace
        0x0F => '\t',
        0x10 => 'q',
        0x11 => 'w',
        0x12 => 'e',
        0x13 => 'r',
        0x14 => 't',
        0x15 => 'y',
        0x16 => 'u',
        0x17 => 'i',
        0x18 => 'o',
        0x19 => 'p',
        0x1A => '[',
        0x1B => ']',
        0x1C => '\n', // Enter
        0x1D => return None, // Left Control
        0x1E => 'a',
        0x1F => 's',
        0x20 => 'd',
        0x21 => 'f',
        0x22 => 'g',
        0x23 => 'h',
        0x24 => 'j',
        0x25 => 'k',
        0x26 => 'l',
        0x27 => ';',
        0x28 => '\'',
        0x29 => '`',
        0x2A => return None, // Left Shift
        0x2B => '\\',
        0x2C => 'z',
        0x2D => 'x',
        0x2E => 'c',
        0x2F => 'v',
        0x30 => 'b',
        0x31 => 'n',
        0x32 => 'm',
        0x33 => ',',
        0x34 => '.',
        0x35 => '/',
        0x36 => return None, // Right Shift
        0x37 => '*',         // Numpad *
        0x38 => return None, // Left Alt
        0x39 => ' ',         // Space
        _ => return None,
    };
    
    Some(character)
}

fn scancode_set2_to_ascii(scancode: u8) -> Option<char> {
    // PS/2 Scan Code Set 2 mappings
    let character = match scancode {
        0x0E => '`',
        0x16 => '1',
        0x1E => '2',
        0x26 => '3',
        0x25 => '4',
        0x2E => '5',
        0x36 => '6',
        0x3D => '7',
        0x3E => '8',
        0x46 => '9',
        0x45 => '0',
        0x4E => '-',
        0x55 => '=',
        0x66 => return None, // Backspace
        0x0D => '\t',
        0x15 => 'q',
        0x1D => 'w',
        0x24 => 'e',
        0x2D => 'r',
        0x2C => 't',
        0x35 => 'y',
        0x3C => 'u',
        0x43 => 'i',
        0x44 => 'o',
        0x4D => 'p',
        0x54 => '[',
        0x5B => ']',
        0x5A => '\n', // Enter
        0x14 => return None, // Left Control
        0x1C => 'a',
        0x1B => 's',
        0x23 => 'd',
        0x2B => 'f',
        0x34 => 'g',
        0x33 => 'h',
        0x3B => 'j',
        0x42 => 'k',
        0x4B => 'l',
        0x4C => ';',
        0x52 => '\'',
        0x5D => '\\',
        0x12 => return None, // Left Shift
        0x1A => 'z',
        0x22 => 'x',
        0x21 => 'c',
        0x2A => 'v',
        0x32 => 'b',
        0x31 => 'n',
        0x3A => 'm',
        0x41 => ',',
        0x49 => '.',
        0x4A => '/',
        0x59 => return None, // Right Shift
        0x11 => return None, // Left Alt
        0x29 => ' ',         // Space
        _ => return None,
    };
    
    Some(character)
}

/// Process any pending keyboard input
/// This should be called from the main loop, NOT from interrupt context
pub fn process_pending_input() {
    loop {
        let scancode = x86_64::instructions::interrupts::without_interrupts(|| {
            SCANCODE_QUEUE.lock().pop()
        });
        
        match scancode {
            Some(scancode) => {
                process_scancode(scancode);
            }
            None => break,
        }
    }
}

pub fn read_character() -> Option<char> {
    loop {
        let scancode = x86_64::instructions::interrupts::without_interrupts(|| {
            SCANCODE_QUEUE.lock().pop()
        });
        
        if let Some(scancode) = scancode {
            let pressed = scancode < 0x80;
            let key_code = if pressed { scancode } else { scancode - 0x80 };
            
            if pressed {
                if let Some(character) = scancode_to_ascii(key_code) {
                    return Some(character);
                }
            }
        } else {
            x86_64::instructions::hlt();
        }
    }
}