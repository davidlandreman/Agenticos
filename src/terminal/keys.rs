//! Keyboard event → escape-sequence byte encoding.
//!
//! When the terminal is in raw mode (ICANON off), every keystroke goes
//! to the slave as bytes. This module owns the mapping. Today's
//! `encode_keystroke_for_raw_mode` in `window::windows::terminal`
//! covers the common cases; this module expands coverage to the full
//! xterm subset that vi / vim / less / htop expect:
//!
//! - Function keys F1–F4: `ESC O P/Q/R/S`.
//! - Function keys F5–F12: `ESC [ 15..24 ~`.
//! - PgUp/PgDn/Insert: `ESC [ 5/6/2 ~`.
//! - Home/End: `ESC [ H/F` (xterm default) or `ESC [ 1 ~ / 4 ~` (vt220 — not emitted).
//! - Arrow keys with modifiers: `ESC [ 1 ; <mod> A/B/C/D` (CSI parameterized).
//!   Modifier code: 2=Shift, 5=Ctrl, 6=Ctrl+Shift, 3=Alt, 4=Alt+Shift,
//!   7=Alt+Ctrl, 8=Alt+Ctrl+Shift.
//! - Meta/Alt-prefixed characters: ESC followed by the character bytes.
//! - Ctrl + ASCII letter: 0x01..0x1A.
//! - Plain printables: UTF-8 bytes from `keycode_to_char`.

use alloc::vec::Vec;

use crate::window::event::{KeyCode, KeyModifiers};
use crate::window::keyboard::keycode_to_char;

/// Encode a keystroke into the bytes a Linux TTY would deliver to the
/// reader. Returns an empty vec for unmapped keys (modifier-only
/// keystrokes, etc.).
pub fn encode_keystroke(key: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
    // Ctrl + ASCII letter → control byte. Hits before the named-key
    // matches so Ctrl-M etc. produce the right byte instead of `\r`.
    // (Ctrl-Shift-letter is treated the same as Ctrl-letter in xterm.)
    if modifiers.ctrl && !modifiers.alt && !modifiers.meta {
        if let Some(ch) = keycode_to_char(key, KeyModifiers::default()) {
            if ch.is_ascii_alphabetic() {
                let lower = ch.to_ascii_lowercase() as u8;
                return alloc::vec![lower - b'a' + 1];
            }
        }
    }

    // Named keys with no modifiers (or with only shift, for printable
    // shift-combos handled below).
    let bytes = match key {
        KeyCode::Enter => alloc::vec![b'\r'],
        KeyCode::Backspace => alloc::vec![0x7F],
        KeyCode::Tab => {
            if modifiers.shift {
                // Back-tab — xterm CSI Z.
                alloc::vec![0x1B, b'[', b'Z']
            } else {
                alloc::vec![b'\t']
            }
        }
        KeyCode::Escape => alloc::vec![0x1B],

        // Arrows: bare = ESC[<letter>; with modifiers = ESC[1;<mod><letter>.
        KeyCode::Up => arrow_seq(b'A', modifiers),
        KeyCode::Down => arrow_seq(b'B', modifiers),
        KeyCode::Right => arrow_seq(b'C', modifiers),
        KeyCode::Left => arrow_seq(b'D', modifiers),
        KeyCode::Home => arrow_seq(b'H', modifiers),
        KeyCode::End => arrow_seq(b'F', modifiers),

        KeyCode::PageUp => tilde_seq(5, modifiers),
        KeyCode::PageDown => tilde_seq(6, modifiers),
        KeyCode::Insert => tilde_seq(2, modifiers),
        KeyCode::Delete => tilde_seq(3, modifiers),

        // F1–F4 use SS3 form `ESC O P/Q/R/S` (xterm/vt100 convention).
        KeyCode::F1 => fn_ss3(b'P', modifiers),
        KeyCode::F2 => fn_ss3(b'Q', modifiers),
        KeyCode::F3 => fn_ss3(b'R', modifiers),
        KeyCode::F4 => fn_ss3(b'S', modifiers),
        // F5–F12 use CSI form `ESC [ <n> ~`. n: 15,17,18,19,20,21,23,24
        // (note the gaps: 16 and 22 are skipped per xterm).
        KeyCode::F5 => tilde_seq(15, modifiers),
        KeyCode::F6 => tilde_seq(17, modifiers),
        KeyCode::F7 => tilde_seq(18, modifiers),
        KeyCode::F8 => tilde_seq(19, modifiers),
        KeyCode::F9 => tilde_seq(20, modifiers),
        KeyCode::F10 => tilde_seq(21, modifiers),
        KeyCode::F11 => tilde_seq(23, modifiers),
        KeyCode::F12 => tilde_seq(24, modifiers),

        _ => {
            // Fall back to UTF-8 of the character mapping. Includes
            // shifted printables (uppercase, !, @, etc.).
            if let Some(ch) = keycode_to_char(key, modifiers) {
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            } else {
                return Vec::new();
            }
        }
    };

    if bytes.is_empty() {
        return bytes;
    }

    // Alt / Meta prefix — ESC followed by the encoded sequence.
    // xterm's "alt sends escape" convention. Avoid prefixing keys that
    // already start with ESC (control sequences) — those are already
    // CSI / SS3 and shouldn't get a second ESC.
    if (modifiers.alt || modifiers.meta) && bytes.first() != Some(&0x1B) {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        out.push(0x1B);
        out.extend_from_slice(&bytes);
        return out;
    }

    bytes
}

/// Build a "CSI <letter>" arrow / Home / End sequence, parameterizing
/// the modifiers when any are set. xterm modifier code:
///   bit 1 = Shift, bit 2 = Alt, bit 4 = Ctrl
///   value = 1 + (bits) → 2 Shift, 3 Alt, 5 Ctrl, 6 Ctrl+Shift, …
fn arrow_seq(letter: u8, modifiers: KeyModifiers) -> Vec<u8> {
    let m = modifier_code(modifiers);
    if m == 1 {
        // No modifier — bare CSI letter.
        alloc::vec![0x1B, b'[', letter]
    } else {
        // CSI 1 ; <m> <letter>
        let mut out = alloc::vec![0x1B, b'[', b'1', b';'];
        push_decimal(&mut out, m);
        out.push(letter);
        out
    }
}

/// Build a "CSI <n> ~" sequence (Insert, Delete, PgUp/Dn, F5–F12).
fn tilde_seq(n: u16, modifiers: KeyModifiers) -> Vec<u8> {
    let m = modifier_code(modifiers);
    let mut out = alloc::vec![0x1B, b'['];
    push_decimal(&mut out, n);
    if m != 1 {
        out.push(b';');
        push_decimal(&mut out, m);
    }
    out.push(b'~');
    out
}

/// Build a "SS3 <letter>" sequence (F1–F4 bare) or "CSI 1 ; <m>
/// <letter>" with modifiers.
fn fn_ss3(letter: u8, modifiers: KeyModifiers) -> Vec<u8> {
    let m = modifier_code(modifiers);
    if m == 1 {
        alloc::vec![0x1B, b'O', letter]
    } else {
        let mut out = alloc::vec![0x1B, b'[', b'1', b';'];
        push_decimal(&mut out, m);
        out.push(letter);
        out
    }
}

fn modifier_code(modifiers: KeyModifiers) -> u16 {
    let mut bits: u16 = 0;
    if modifiers.shift {
        bits |= 1;
    }
    if modifiers.alt {
        bits |= 2;
    }
    if modifiers.ctrl {
        bits |= 4;
    }
    1 + bits
}

fn push_decimal(buf: &mut Vec<u8>, mut v: u16) {
    if v == 0 {
        buf.push(b'0');
        return;
    }
    let mut tmp = [0u8; 5];
    let mut n = 0;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        buf.push(tmp[n - 1 - i]);
    }
}

/// Wrap a paste in bracketed-paste markers — `ESC [ 200 ~ … ESC [ 201
/// ~`. Used when the screen has `?2004h` set.
pub fn wrap_bracketed_paste(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(b"\x1B[200~");
    out.extend_from_slice(data);
    out.extend_from_slice(b"\x1B[201~");
    out
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &tests::test_plain_enter_backspace_tab,
        &tests::test_ctrl_letter,
        &tests::test_arrow_bare,
        &tests::test_arrow_shift,
        &tests::test_arrow_ctrl,
        &tests::test_arrow_ctrl_shift,
        &tests::test_home_end,
        &tests::test_pageup_pagedown,
        &tests::test_insert_delete,
        &tests::test_f1_f4_ss3,
        &tests::test_f5_f12_csi_tilde,
        &tests::test_alt_letter_prefix,
        &tests::test_shift_tab_backtab,
        &tests::test_bracketed_paste,
    ]
}

#[cfg(feature = "test")]
mod tests {
    use super::*;

    fn no_mods() -> KeyModifiers {
        KeyModifiers::default()
    }

    fn ctrl() -> KeyModifiers {
        KeyModifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn shift() -> KeyModifiers {
        KeyModifiers {
            shift: true,
            ..Default::default()
        }
    }

    fn alt() -> KeyModifiers {
        KeyModifiers {
            alt: true,
            ..Default::default()
        }
    }

    pub(super) fn test_plain_enter_backspace_tab() {
        assert_eq!(encode_keystroke(KeyCode::Enter, no_mods()), b"\r".to_vec());
        assert_eq!(encode_keystroke(KeyCode::Backspace, no_mods()), alloc::vec![0x7F]);
        assert_eq!(encode_keystroke(KeyCode::Tab, no_mods()), b"\t".to_vec());
        assert_eq!(encode_keystroke(KeyCode::Escape, no_mods()), alloc::vec![0x1B]);
    }

    pub(super) fn test_ctrl_letter() {
        assert_eq!(encode_keystroke(KeyCode::A, ctrl()), alloc::vec![0x01]);
        assert_eq!(encode_keystroke(KeyCode::C, ctrl()), alloc::vec![0x03]);
        assert_eq!(encode_keystroke(KeyCode::Z, ctrl()), alloc::vec![0x1A]);
    }

    pub(super) fn test_arrow_bare() {
        assert_eq!(encode_keystroke(KeyCode::Up, no_mods()), b"\x1b[A".to_vec());
        assert_eq!(encode_keystroke(KeyCode::Down, no_mods()), b"\x1b[B".to_vec());
        assert_eq!(encode_keystroke(KeyCode::Right, no_mods()), b"\x1b[C".to_vec());
        assert_eq!(encode_keystroke(KeyCode::Left, no_mods()), b"\x1b[D".to_vec());
    }

    pub(super) fn test_arrow_shift() {
        // Shift+Up = ESC[1;2A
        assert_eq!(encode_keystroke(KeyCode::Up, shift()), b"\x1b[1;2A".to_vec());
    }

    pub(super) fn test_arrow_ctrl() {
        // Ctrl+Right = ESC[1;5C
        assert_eq!(encode_keystroke(KeyCode::Right, ctrl()), b"\x1b[1;5C".to_vec());
    }

    pub(super) fn test_arrow_ctrl_shift() {
        // Ctrl+Shift+Left = ESC[1;6D
        let m = KeyModifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(encode_keystroke(KeyCode::Left, m), b"\x1b[1;6D".to_vec());
    }

    pub(super) fn test_home_end() {
        assert_eq!(encode_keystroke(KeyCode::Home, no_mods()), b"\x1b[H".to_vec());
        assert_eq!(encode_keystroke(KeyCode::End, no_mods()), b"\x1b[F".to_vec());
    }

    pub(super) fn test_pageup_pagedown() {
        assert_eq!(encode_keystroke(KeyCode::PageUp, no_mods()), b"\x1b[5~".to_vec());
        assert_eq!(encode_keystroke(KeyCode::PageDown, no_mods()), b"\x1b[6~".to_vec());
    }

    pub(super) fn test_insert_delete() {
        assert_eq!(encode_keystroke(KeyCode::Insert, no_mods()), b"\x1b[2~".to_vec());
        assert_eq!(encode_keystroke(KeyCode::Delete, no_mods()), b"\x1b[3~".to_vec());
    }

    pub(super) fn test_f1_f4_ss3() {
        assert_eq!(encode_keystroke(KeyCode::F1, no_mods()), b"\x1bOP".to_vec());
        assert_eq!(encode_keystroke(KeyCode::F2, no_mods()), b"\x1bOQ".to_vec());
        assert_eq!(encode_keystroke(KeyCode::F3, no_mods()), b"\x1bOR".to_vec());
        assert_eq!(encode_keystroke(KeyCode::F4, no_mods()), b"\x1bOS".to_vec());
    }

    pub(super) fn test_f5_f12_csi_tilde() {
        assert_eq!(encode_keystroke(KeyCode::F5, no_mods()), b"\x1b[15~".to_vec());
        assert_eq!(encode_keystroke(KeyCode::F6, no_mods()), b"\x1b[17~".to_vec());
        assert_eq!(encode_keystroke(KeyCode::F12, no_mods()), b"\x1b[24~".to_vec());
    }

    pub(super) fn test_alt_letter_prefix() {
        // Alt+a → ESC a
        let bytes = encode_keystroke(KeyCode::A, alt());
        assert_eq!(bytes, alloc::vec![0x1B, b'a']);
    }

    pub(super) fn test_shift_tab_backtab() {
        assert_eq!(encode_keystroke(KeyCode::Tab, shift()), b"\x1b[Z".to_vec());
    }

    pub(super) fn test_bracketed_paste() {
        let w = wrap_bracketed_paste(b"hello");
        assert_eq!(w, b"\x1b[200~hello\x1b[201~".to_vec());
    }
}
