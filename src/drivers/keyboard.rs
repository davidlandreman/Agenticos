use spin::Mutex;
use lazy_static::lazy_static;
use crate::{debug_info, print};

const SCANCODE_QUEUE_SIZE: usize = 100;

lazy_static! {
    static ref SCANCODE_QUEUE: Mutex<ScancodeQueue> = Mutex::new(ScancodeQueue::new());
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
    if let Err(()) = SCANCODE_QUEUE.lock().push(scancode) {
        debug_info!("WARNING: Keyboard scancode queue full; dropping input");
    } else {
        process_scancode(scancode);
    }
}

fn process_scancode(scancode: u8) {
    let pressed = scancode < 0x80;
    let key_code = if pressed { scancode } else { scancode - 0x80 };
    
    if pressed {
        if let Some(character) = scancode_to_ascii(key_code) {
            print!("{}", character);
        }
    }
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

pub fn read_character() -> Option<char> {
    loop {
        if let Some(scancode) = SCANCODE_QUEUE.lock().pop() {
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