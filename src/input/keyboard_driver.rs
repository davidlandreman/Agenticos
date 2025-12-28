//! Keyboard driver with proper PS/2 Scancode Set 2 state machine.
//!
//! This module handles the complexity of PS/2 keyboard protocol:
//! - Multi-byte sequences (0xE0 extended keys, 0xF0 break codes)
//! - Proper modifier key tracking (press AND release)
//! - Conversion from scancodes to KeyCode events

use crate::window::event::{KeyCode, KeyModifiers, KeyboardEvent};

/// Keyboard state machine for processing PS/2 Scancode Set 2.
///
/// PS/2 Scancode Set 2 uses:
/// - 0xF0 prefix for break codes (key release)
/// - 0xE0 prefix for extended keys (arrows, right ctrl, etc.)
/// - 0xE1 prefix for Pause/Break (rare, not fully implemented)
#[derive(Debug)]
pub struct KeyboardDriver {
    /// Current modifier key states (tracks left/right separately)
    modifiers: ModifierState,
    /// Expecting break code after 0xF0 prefix
    expecting_break: bool,
    /// Expecting extended code after 0xE0 prefix
    expecting_extended: bool,
}

/// Internal modifier state tracking left/right keys separately.
#[derive(Debug, Default, Clone, Copy)]
struct ModifierState {
    left_shift: bool,
    right_shift: bool,
    left_ctrl: bool,
    right_ctrl: bool,
    left_alt: bool,
    right_alt: bool,
}

impl ModifierState {
    /// Convert to the public KeyModifiers type.
    fn to_key_modifiers(&self) -> KeyModifiers {
        KeyModifiers {
            shift: self.left_shift || self.right_shift,
            ctrl: self.left_ctrl || self.right_ctrl,
            alt: self.left_alt || self.right_alt,
            meta: false, // No meta/windows key support yet
        }
    }
}

impl Default for KeyboardDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardDriver {
    /// Create a new keyboard driver with default state.
    pub const fn new() -> Self {
        Self {
            modifiers: ModifierState {
                left_shift: false,
                right_shift: false,
                left_ctrl: false,
                right_ctrl: false,
                left_alt: false,
                right_alt: false,
            },
            expecting_break: false,
            expecting_extended: false,
        }
    }

    /// Process a raw scancode and optionally produce a KeyboardEvent.
    ///
    /// Returns `Some(event)` when a complete key press/release is detected,
    /// or `None` for prefix bytes (0xE0, 0xF0) that don't produce events yet.
    pub fn process_scancode(&mut self, scancode: u8) -> Option<KeyboardEvent> {
        // Handle special prefix bytes
        match scancode {
            0xF0 => {
                // Break code prefix - next scancode is a key release
                self.expecting_break = true;
                return None;
            }
            0xE0 => {
                // Extended key prefix - next scancode is extended key
                self.expecting_extended = true;
                return None;
            }
            0xE1 => {
                // Pause/Break uses E1 prefix (rare, ignore for now)
                // Full sequence is E1 14 77 E1 F0 14 F0 77
                return None;
            }
            0xFA => {
                // ACK from keyboard (after commands) - ignore
                return None;
            }
            0xAA => {
                // Self-test passed (after reset) - ignore
                return None;
            }
            _ => {}
        }

        // Capture and reset state flags
        let is_release = self.expecting_break;
        let is_extended = self.expecting_extended;
        self.expecting_break = false;
        self.expecting_extended = false;

        // Update modifier state FIRST (this is the critical fix!)
        // Modifiers must be updated before we generate the event
        self.update_modifier(scancode, is_release, is_extended);

        // Convert scancode to KeyCode
        let key_code = if is_extended {
            scancode_extended_to_keycode(scancode)
        } else {
            scancode_to_keycode(scancode)
        };

        // Generate event if we have a valid key code
        key_code.map(|code| KeyboardEvent {
            key_code: code,
            pressed: !is_release,
            modifiers: self.modifiers.to_key_modifiers(),
        })
    }

    /// Update modifier key state based on scancode.
    ///
    /// This handles BOTH press and release for all modifier keys.
    fn update_modifier(&mut self, scancode: u8, is_release: bool, is_extended: bool) {
        let pressed = !is_release;

        match (scancode, is_extended) {
            // Left Shift (0x12)
            (0x12, false) => self.modifiers.left_shift = pressed,
            // Right Shift (0x59)
            (0x59, false) => self.modifiers.right_shift = pressed,
            // Left Ctrl (0x14, not extended)
            (0x14, false) => self.modifiers.left_ctrl = pressed,
            // Right Ctrl (0x14 with E0 prefix)
            (0x14, true) => self.modifiers.right_ctrl = pressed,
            // Left Alt (0x11, not extended)
            (0x11, false) => self.modifiers.left_alt = pressed,
            // Right Alt / AltGr (0x11 with E0 prefix)
            (0x11, true) => self.modifiers.right_alt = pressed,
            _ => {}
        }
    }

    /// Get current modifier state (useful for mouse events that need modifier info).
    pub fn current_modifiers(&self) -> KeyModifiers {
        self.modifiers.to_key_modifiers()
    }

    /// Check if shift is pressed.
    pub fn is_shift_pressed(&self) -> bool {
        self.modifiers.left_shift || self.modifiers.right_shift
    }

    /// Check if ctrl is pressed.
    pub fn is_ctrl_pressed(&self) -> bool {
        self.modifiers.left_ctrl || self.modifiers.right_ctrl
    }

    /// Check if alt is pressed.
    pub fn is_alt_pressed(&self) -> bool {
        self.modifiers.left_alt || self.modifiers.right_alt
    }
}

/// Convert PS/2 Scancode Set 2 (non-extended) to KeyCode.
fn scancode_to_keycode(scancode: u8) -> Option<KeyCode> {
    match scancode {
        // Row 1 - Function keys
        0x76 => Some(KeyCode::Escape),
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
        0x0E => Some(KeyCode::Backtick),
        0x16 => Some(KeyCode::Key1),
        0x1E => Some(KeyCode::Key2),
        0x26 => Some(KeyCode::Key3),
        0x25 => Some(KeyCode::Key4),
        0x2E => Some(KeyCode::Key5),
        0x36 => Some(KeyCode::Key6),
        0x3D => Some(KeyCode::Key7),
        0x3E => Some(KeyCode::Key8),
        0x46 => Some(KeyCode::Key9),
        0x45 => Some(KeyCode::Key0),
        0x4E => Some(KeyCode::Minus),
        0x55 => Some(KeyCode::Equals),
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
        0x54 => Some(KeyCode::LeftBracket),
        0x5B => Some(KeyCode::RightBracket),
        0x5D => Some(KeyCode::Backslash),

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
        0x4C => Some(KeyCode::Semicolon),
        0x52 => Some(KeyCode::Quote),
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
        0x41 => Some(KeyCode::Comma),
        0x49 => Some(KeyCode::Period),
        0x4A => Some(KeyCode::Slash),
        0x59 => Some(KeyCode::RightShift),

        // Row 6 - Control row
        0x14 => Some(KeyCode::LeftCtrl),
        0x11 => Some(KeyCode::LeftAlt),
        0x29 => Some(KeyCode::Space),

        _ => None,
    }
}

/// Convert PS/2 Scancode Set 2 (extended, with 0xE0 prefix) to KeyCode.
fn scancode_extended_to_keycode(scancode: u8) -> Option<KeyCode> {
    match scancode {
        // Arrow keys (extended)
        0x75 => Some(KeyCode::Up),
        0x6B => Some(KeyCode::Left),
        0x74 => Some(KeyCode::Right),
        0x72 => Some(KeyCode::Down),

        // Navigation cluster (extended)
        0x71 => Some(KeyCode::Delete),
        // 0x70 => Some(KeyCode::Insert),
        // 0x6C => Some(KeyCode::Home),
        // 0x69 => Some(KeyCode::End),
        // 0x7D => Some(KeyCode::PageUp),
        // 0x7A => Some(KeyCode::PageDown),

        // Right-side modifiers (extended)
        0x14 => Some(KeyCode::RightCtrl),
        0x11 => Some(KeyCode::RightAlt),

        // Windows/GUI keys (extended)
        // 0x1F => Some(KeyCode::LeftMeta),
        // 0x27 => Some(KeyCode::RightMeta),
        // 0x2F => Some(KeyCode::Menu),

        _ => None,
    }
}

/// Convert KeyCode to a character (if applicable).
///
/// This is kept in the keyboard driver for convenience since it needs
/// modifier state to determine the output character.
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
