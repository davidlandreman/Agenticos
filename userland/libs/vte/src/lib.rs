//! Terminal emulator core — ring-3 port of the kernel `src/terminal/`.
//!
//! The kernel keeps the PTY (fd pair, termios, winsize, line discipline); this
//! crate holds the *emulator*: the DEC/ANSI state-machine parser (`vte`), the
//! character grid + scrollback + alt-screen (`screen`), the caret snapshot
//! (`caret`), the 256-color palette (`colors`), key→escape encoding (`keys`),
//! and compile-time terminal constants (`config`).
//!
//! Ported from the kernel modules with two substitutions: the kernel's
//! `graphics::color::Color` becomes the crate-local [`color::Color`], and the
//! kernel's `window::event` key types become the crate-local [`input`] module,
//! which also decodes the GUI-event wire representation
//! (`GUI_EVENT_KEY` payload[0] = `encode_key_code`).

#![no_std]

extern crate alloc;

pub mod caret;
pub mod color;
pub mod colors;
pub mod config;
pub mod input;
pub mod keys;
pub mod screen;
pub mod vte;

pub use caret::Caret;
pub use colors::ColorSpec;
pub use input::{decode_key_code, KeyCode, KeyModifiers};
pub use screen::{Cell, CursorShape, Screen};
pub use vte::{Perform, Vte};
