//! Event system for the window manager

use alloc::boxed::Box;
use super::types::{WindowId, Point};

/// Key codes for keyboard events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    // Letters
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    // Numbers
    Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9,
    // Special keys
    Escape, Enter, Space, Tab, Backspace, Delete,
    Left, Right, Up, Down,
    // Modifiers
    LeftShift, RightShift, LeftCtrl, RightCtrl, LeftAlt, RightAlt,
    // Function keys
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    // Punctuation
    Comma, Period, Slash, Semicolon, Quote, 
    LeftBracket, RightBracket, Backslash,
    Minus, Equals, Backtick,
    // Other
    Unknown,
}

/// Result of handling an event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// Event was processed and should not propagate further
    Handled,
    /// Event was not relevant to this window
    Ignored,
    /// Event should propagate to parent window
    Propagate,
}

/// Events that can be sent to windows
#[derive(Debug, Clone)]
pub enum Event {
    /// Keyboard input event
    Keyboard(KeyboardEvent),
    /// Mouse input event
    Mouse(MouseEvent),
    /// Window resize event
    Resize(ResizeEvent),
    /// Window move event
    Move(MoveEvent),
    /// Window close request
    Close(CloseEvent),
    /// Focus change event
    Focus(FocusEvent),
}

/// Keyboard event data
#[derive(Debug, Clone, Copy)]
pub struct KeyboardEvent {
    /// The key code that was pressed or released
    pub key_code: KeyCode,
    /// Whether the key was pressed (true) or released (false)
    pub pressed: bool,
    /// Modifier keys state
    pub modifiers: KeyModifiers,
}

/// Keyboard modifier keys state
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Mouse event data
#[derive(Debug, Clone, Copy)]
pub struct MouseEvent {
    /// Type of mouse event
    pub event_type: MouseEventType,
    /// Mouse position relative to the window
    pub position: Point,
    /// Global mouse position
    pub global_position: Point,
    /// Mouse button state
    pub buttons: MouseButtons,
}

/// Types of mouse events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    Move,
    ButtonDown,
    ButtonUp,
    Scroll,
}

/// Mouse button state
#[derive(Debug, Clone, Copy, Default)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

/// Window resize event
#[derive(Debug, Clone, Copy)]
pub struct ResizeEvent {
    /// New width
    pub width: u32,
    /// New height
    pub height: u32,
}

/// Window move event
#[derive(Debug, Clone, Copy)]
pub struct MoveEvent {
    /// New X position
    pub x: i32,
    /// New Y position
    pub y: i32,
}

/// Window close event
#[derive(Debug, Clone, Copy)]
pub struct CloseEvent {
    /// Window requesting to close
    pub window: WindowId,
}

/// Focus change event
#[derive(Debug, Clone, Copy)]
pub struct FocusEvent {
    /// Whether the window gained (true) or lost (false) focus
    pub gained: bool,
}