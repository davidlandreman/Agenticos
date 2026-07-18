//! Keyboard scancode to KeyCode conversion for window system

use super::event::{KeyCode, KeyModifiers};

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
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn update_modifiers(&mut self, scancode: u8) {
        // For scancode set 2, we need to handle modifiers differently
        // For now, we'll track make codes only (not handling break codes yet)
        match scancode {
            0x12 | 0x59 => self.modifiers.shift = true, // Left/Right Shift pressed
            0x14 => self.modifiers.ctrl = true,         // Left Ctrl pressed
            0x11 => self.modifiers.alt = true,          // Left Alt pressed
            // TODO: Handle break codes (0xF0 prefix) to clear modifiers
            _ => {}
        }
    }
}
