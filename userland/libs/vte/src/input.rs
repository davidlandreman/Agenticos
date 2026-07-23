//! Key types + GUI-event wire decoding, replacing the kernel's
//! `window::event` / `window::keyboard`.
//!
//! A ring-3 terminal receives keystrokes as `GUI_EVENT_KEY` records whose
//! `payload[0]` is the kernel's `encode_key_code` value (0 = Unknown, 1..=80
//! for the named keys, in the order below). [`decode_key_code`] is the inverse
//! of that mapping. `keys::encode_keystroke` then consumes the decoded
//! [`KeyCode`] + [`KeyModifiers`] exactly as the kernel `TerminalWindow` did.
//!
//! The [`KeyCode`] variant order here is load-bearing: it must match the
//! kernel's `src/userland/gui.rs::encode_key_code` numbering.

/// A logical key, mirroring the kernel `window::event::KeyCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    // Letters
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    // Numbers
    Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9,
    // Special keys
    Escape, Enter, Space, Tab, Backspace, Delete,
    Left, Right, Up, Down, Home, End, PageUp, PageDown, Insert,
    // Modifiers
    LeftShift, RightShift, LeftCtrl, RightCtrl, LeftAlt, RightAlt,
    // Function keys
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    // Punctuation
    Comma, Period, Slash, Semicolon, Quote, LeftBracket, RightBracket,
    Backslash, Minus, Equals, Backtick,
    // Other
    Unknown,
}

/// Keyboard modifier state, mirroring `window::event::KeyModifiers`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// The GUI_EVENT_KEY `payload[2]` modifier bit layout (see kernel `gui.rs`).
pub const KEY_MOD_SHIFT: u32 = 1;
pub const KEY_MOD_CTRL: u32 = 2;
pub const KEY_MOD_ALT: u32 = 4;

impl KeyModifiers {
    /// Decode the `payload[2]` modifier word delivered with a `GUI_EVENT_KEY`.
    pub const fn from_payload(bits: u32) -> Self {
        Self {
            shift: bits & KEY_MOD_SHIFT != 0,
            ctrl: bits & KEY_MOD_CTRL != 0,
            alt: bits & KEY_MOD_ALT != 0,
            meta: false,
        }
    }
}

/// Decode the `GUI_EVENT_KEY` `payload[0]` value into a [`KeyCode`]. Inverse of
/// the kernel `src/userland/gui.rs::encode_key_code`.
pub fn decode_key_code(code: u32) -> KeyCode {
    use KeyCode::*;
    match code {
        1 => A, 2 => B, 3 => C, 4 => D, 5 => E, 6 => F, 7 => G, 8 => H, 9 => I,
        10 => J, 11 => K, 12 => L, 13 => M, 14 => N, 15 => O, 16 => P, 17 => Q,
        18 => R, 19 => S, 20 => T, 21 => U, 22 => V, 23 => W, 24 => X, 25 => Y,
        26 => Z,
        27 => Key0, 28 => Key1, 29 => Key2, 30 => Key3, 31 => Key4, 32 => Key5,
        33 => Key6, 34 => Key7, 35 => Key8, 36 => Key9,
        37 => Escape, 38 => Enter, 39 => Space, 40 => Tab, 41 => Backspace,
        42 => Delete, 43 => Left, 44 => Right, 45 => Up, 46 => Down, 47 => Home,
        48 => End, 49 => PageUp, 50 => PageDown, 51 => Insert,
        52 => LeftShift, 53 => RightShift, 54 => LeftCtrl, 55 => RightCtrl,
        56 => LeftAlt, 57 => RightAlt,
        58 => F1, 59 => F2, 60 => F3, 61 => F4, 62 => F5, 63 => F6, 64 => F7,
        65 => F8, 66 => F9, 67 => F10, 68 => F11, 69 => F12,
        70 => Comma, 71 => Period, 72 => Slash, 73 => Semicolon, 74 => Quote,
        75 => LeftBracket, 76 => RightBracket, 77 => Backslash, 78 => Minus,
        79 => Equals, 80 => Backtick,
        _ => Unknown,
    }
}

/// Convert a [`KeyCode`] to the character it produces (if any). Ported verbatim
/// from the kernel `window::keyboard::keycode_to_char`.
pub fn keycode_to_char(key_code: KeyCode, modifiers: KeyModifiers) -> Option<char> {
    match key_code {
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

        KeyCode::Space => Some(' '),
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab => Some('\t'),

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

        _ => None,
    }
}
