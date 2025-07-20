//! Keyboard scancode to KeyCode conversion for window system

use super::event::{KeyCode, KeyModifiers};

/// Convert PS/2 scancode set 2 to KeyCode
pub fn scancode_to_keycode(scancode: u8) -> Option<KeyCode> {
    // PS/2 Scancode Set 2 (default for PS/2 keyboards)
    // Note: This is different from Set 1 used by old XT keyboards
    
    match scancode {
        // Row 1 - Function keys
        0x76 => Some(KeyCode::Escape),      // ESC
        0x05 => Some(KeyCode::F1),
        0x06 => Some(KeyCode::F2),
        0x04 => Some(KeyCode::F3),
        0x0C => Some(KeyCode::F4),
        0x03 => Some(KeyCode::F5),
        0x0B => Some(KeyCode::F6),
        0x83 => Some(KeyCode::F7),
        0x0A => Some(KeyCode::F8),
        0x01 => Some(KeyCode::F9),
        0x09 => Some(KeyCode::F10),
        0x78 => Some(KeyCode::F11),
        0x07 => Some(KeyCode::F12),
        
        // Row 2 - Numbers
        0x0E => Some(KeyCode::Backtick),    // ` ~
        0x16 => Some(KeyCode::Key1),        // 1 !
        0x1E => Some(KeyCode::Key2),        // 2 @
        0x26 => Some(KeyCode::Key3),        // 3 #
        0x25 => Some(KeyCode::Key4),        // 4 $
        0x2E => Some(KeyCode::Key5),        // 5 %
        0x36 => Some(KeyCode::Key6),        // 6 ^
        0x3D => Some(KeyCode::Key7),        // 7 &
        0x3E => Some(KeyCode::Key8),        // 8 *
        0x46 => Some(KeyCode::Key9),        // 9 (
        0x45 => Some(KeyCode::Key0),        // 0 )
        0x4E => Some(KeyCode::Minus),       // - _
        0x55 => Some(KeyCode::Equals),      // = +
        0x66 => Some(KeyCode::Backspace),
        
        // Row 3 - QWERTY
        0x0D => Some(KeyCode::Tab),
        0x15 => Some(KeyCode::Q),
        0x1D => Some(KeyCode::W),
        0x24 => Some(KeyCode::E),
        0x2D => Some(KeyCode::R),
        0x2C => Some(KeyCode::T),
        0x35 => Some(KeyCode::Y),
        0x3C => Some(KeyCode::U),
        0x43 => Some(KeyCode::I),
        0x44 => Some(KeyCode::O),
        0x4D => Some(KeyCode::P),
        0x54 => Some(KeyCode::LeftBracket),  // [ {
        0x5B => Some(KeyCode::RightBracket), // ] }
        0x5D => Some(KeyCode::Backslash),    // \ |
        
        // Row 4 - ASDF
        0x58 => Some(KeyCode::Unknown), // Caps Lock
        0x1C => Some(KeyCode::A),
        0x1B => Some(KeyCode::S),
        0x23 => Some(KeyCode::D),
        0x2B => Some(KeyCode::F),
        0x34 => Some(KeyCode::G),
        0x33 => Some(KeyCode::H),
        0x3B => Some(KeyCode::J),
        0x42 => Some(KeyCode::K),
        0x4B => Some(KeyCode::L),
        0x4C => Some(KeyCode::Semicolon),    // ; :
        0x52 => Some(KeyCode::Quote),        // ' "
        0x5A => Some(KeyCode::Enter),
        
        // Row 5 - ZXCV
        0x12 => Some(KeyCode::LeftShift),
        0x1A => Some(KeyCode::Z),
        0x22 => Some(KeyCode::X),
        0x21 => Some(KeyCode::C),
        0x2A => Some(KeyCode::V),
        0x32 => Some(KeyCode::B),
        0x31 => Some(KeyCode::N),
        0x3A => Some(KeyCode::M),
        0x41 => Some(KeyCode::Comma),        // , <
        0x49 => Some(KeyCode::Period),       // . >
        0x4A => Some(KeyCode::Slash),        // / ?
        0x59 => Some(KeyCode::RightShift),
        
        // Row 6 - Control row
        0x14 => Some(KeyCode::LeftCtrl),
        0x11 => Some(KeyCode::LeftAlt),
        0x29 => Some(KeyCode::Space),
        
        // Extended keys (usually prefixed with 0xE0)
        // For now, we'll handle the single-byte versions
        0x75 => Some(KeyCode::Up),       // Arrow Up
        0x6B => Some(KeyCode::Left),     // Arrow Left
        0x74 => Some(KeyCode::Right),    // Arrow Right
        0x72 => Some(KeyCode::Down),     // Arrow Down
        
        0x71 => Some(KeyCode::Delete),   // Delete
        
        _ => None,
    }
}

/// Check if a scancode is a break code (key release)
/// In scancode set 2, break codes are prefixed with 0xF0
pub fn is_break_code(scancode: u8) -> bool {
    // This is a simplified check - in reality we'd need to track
    // if we received an 0xF0 prefix before this scancode
    false // For now, we're only handling make codes
}

/// Convert KeyCode to a character (if possible)
pub fn keycode_to_char(key_code: KeyCode, modifiers: KeyModifiers) -> Option<char> {
    match key_code {
        // Letters
        KeyCode::A => Some(if modifiers.shift { 'A' } else { 'a' }),
        KeyCode::B => Some(if modifiers.shift { 'B' } else { 'b' }),
        KeyCode::C => Some(if modifiers.shift { 'C' } else { 'c' }),
        KeyCode::D => Some(if modifiers.shift { 'D' } else { 'd' }),
        KeyCode::E => Some(if modifiers.shift { 'E' } else { 'e' }),
        KeyCode::F => Some(if modifiers.shift { 'F' } else { 'f' }),
        KeyCode::G => Some(if modifiers.shift { 'G' } else { 'g' }),
        KeyCode::H => Some(if modifiers.shift { 'H' } else { 'h' }),
        KeyCode::I => Some(if modifiers.shift { 'I' } else { 'i' }),
        KeyCode::J => Some(if modifiers.shift { 'J' } else { 'j' }),
        KeyCode::K => Some(if modifiers.shift { 'K' } else { 'k' }),
        KeyCode::L => Some(if modifiers.shift { 'L' } else { 'l' }),
        KeyCode::M => Some(if modifiers.shift { 'M' } else { 'm' }),
        KeyCode::N => Some(if modifiers.shift { 'N' } else { 'n' }),
        KeyCode::O => Some(if modifiers.shift { 'O' } else { 'o' }),
        KeyCode::P => Some(if modifiers.shift { 'P' } else { 'p' }),
        KeyCode::Q => Some(if modifiers.shift { 'Q' } else { 'q' }),
        KeyCode::R => Some(if modifiers.shift { 'R' } else { 'r' }),
        KeyCode::S => Some(if modifiers.shift { 'S' } else { 's' }),
        KeyCode::T => Some(if modifiers.shift { 'T' } else { 't' }),
        KeyCode::U => Some(if modifiers.shift { 'U' } else { 'u' }),
        KeyCode::V => Some(if modifiers.shift { 'V' } else { 'v' }),
        KeyCode::W => Some(if modifiers.shift { 'W' } else { 'w' }),
        KeyCode::X => Some(if modifiers.shift { 'X' } else { 'x' }),
        KeyCode::Y => Some(if modifiers.shift { 'Y' } else { 'y' }),
        KeyCode::Z => Some(if modifiers.shift { 'Z' } else { 'z' }),
        
        // Numbers
        KeyCode::Key0 => Some(if modifiers.shift { ')' } else { '0' }),
        KeyCode::Key1 => Some(if modifiers.shift { '!' } else { '1' }),
        KeyCode::Key2 => Some(if modifiers.shift { '@' } else { '2' }),
        KeyCode::Key3 => Some(if modifiers.shift { '#' } else { '3' }),
        KeyCode::Key4 => Some(if modifiers.shift { '$' } else { '4' }),
        KeyCode::Key5 => Some(if modifiers.shift { '%' } else { '5' }),
        KeyCode::Key6 => Some(if modifiers.shift { '^' } else { '6' }),
        KeyCode::Key7 => Some(if modifiers.shift { '&' } else { '7' }),
        KeyCode::Key8 => Some(if modifiers.shift { '*' } else { '8' }),
        KeyCode::Key9 => Some(if modifiers.shift { '(' } else { '9' }),
        
        // Special characters
        KeyCode::Space => Some(' '),
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab => Some('\t'),
        
        // Punctuation
        KeyCode::Comma => Some(if modifiers.shift { '<' } else { ',' }),
        KeyCode::Period => Some(if modifiers.shift { '>' } else { '.' }),
        KeyCode::Slash => Some(if modifiers.shift { '?' } else { '/' }),
        KeyCode::Semicolon => Some(if modifiers.shift { ':' } else { ';' }),
        KeyCode::Quote => Some(if modifiers.shift { '"' } else { '\'' }),
        KeyCode::LeftBracket => Some(if modifiers.shift { '{' } else { '[' }),
        KeyCode::RightBracket => Some(if modifiers.shift { '}' } else { ']' }),
        KeyCode::Backslash => Some(if modifiers.shift { '|' } else { '\\' }),
        KeyCode::Minus => Some(if modifiers.shift { '_' } else { '-' }),
        KeyCode::Equals => Some(if modifiers.shift { '+' } else { '=' }),
        KeyCode::Backtick => Some(if modifiers.shift { '~' } else { '`' }),
        
        // Other keys don't produce characters
        _ => None,
    }
}

/// Keyboard state tracker for modifier keys
#[derive(Debug, Default)]
pub struct KeyboardState {
    pub modifiers: KeyModifiers,
}

impl KeyboardState {
    /// Update modifier state based on scancode
    pub fn update_modifiers(&mut self, scancode: u8) {
        // For scancode set 2, we need to handle modifiers differently
        // For now, we'll track make codes only (not handling break codes yet)
        match scancode {
            0x12 | 0x59 => self.modifiers.shift = true,  // Left/Right Shift pressed
            0x14 => self.modifiers.ctrl = true,          // Left Ctrl pressed
            0x11 => self.modifiers.alt = true,           // Left Alt pressed
            // TODO: Handle break codes (0xF0 prefix) to clear modifiers
            _ => {}
        }
    }
}