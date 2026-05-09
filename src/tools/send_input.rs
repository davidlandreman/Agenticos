//! `send_input` tool — synthesize keyboard and mouse events into the
//! window-system event pipeline. Validate-then-inject: malformed batches
//! reject before any event lands; downstream consumers cannot tell synthetic
//! events apart from hardware-driven ones (R6, AE5).

use alloc::format;
use alloc::vec::Vec;

use serde::Deserialize;
use serde_json::json;

use crate::tools::{Tool, ToolError, ToolResult};
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::types::Point;

/// Cap to bound dispatcher work per call. Larger batches must be chunked by
/// the caller.
const MAX_BATCH: usize = 256;

#[derive(Deserialize)]
struct SendInputArgs {
    #[serde(default)]
    keyboard: Vec<KeyboardEntry>,
    #[serde(default)]
    mouse: Vec<MouseEntry>,
}

#[derive(Deserialize)]
struct KeyboardEntry {
    key_code: alloc::string::String,
    pressed: bool,
    #[serde(default)]
    modifiers: ModifierEntry,
}

#[derive(Deserialize, Default)]
struct ModifierEntry {
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    meta: bool,
}

#[derive(Deserialize)]
struct MouseEntry {
    event_type: alloc::string::String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    global_x: Option<i32>,
    #[serde(default)]
    global_y: Option<i32>,
    #[serde(default)]
    buttons: MouseButtonsEntry,
}

#[derive(Deserialize, Default)]
struct MouseButtonsEntry {
    #[serde(default)]
    left: bool,
    #[serde(default)]
    right: bool,
    #[serde(default)]
    middle: bool,
}

pub struct SendInput;

impl Tool for SendInput {
    fn name(&self) -> &'static str { "send_input" }

    fn description(&self) -> &'static str {
        "synthesize keyboard and/or mouse events into the window event pipeline"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{"keyboard":{"type":"array"},"mouse":{"type":"array"}}}"#
    }

    fn call(&self, args_json: &str) -> Result<ToolResult, ToolError> {
        let args: SendInputArgs = serde_json::from_str(args_json)
            .map_err(|e| ToolError::bad_args(format!("invalid args: {}", e)))?;

        let total = args.keyboard.len() + args.mouse.len();
        if total > MAX_BATCH {
            return Err(ToolError::bad_args(format!(
                "batch of {} exceeds max {}",
                total, MAX_BATCH
            )));
        }

        // Validate-then-inject: build the full Event list before any
        // downstream call. process_event mutates window state immediately;
        // partial injection on bad input has no rollback path.
        let mut events: Vec<Event> = Vec::with_capacity(total);
        for k in &args.keyboard {
            let key_code = parse_key_code(&k.key_code)
                .ok_or_else(|| ToolError::bad_args(format!("unknown key_code {:?}", k.key_code)))?;
            events.push(Event::Keyboard(KeyboardEvent {
                key_code,
                pressed: k.pressed,
                modifiers: KeyModifiers {
                    shift: k.modifiers.shift,
                    ctrl: k.modifiers.ctrl,
                    alt: k.modifiers.alt,
                    meta: k.modifiers.meta,
                },
            }));
        }
        for m in &args.mouse {
            let event_type = parse_mouse_event_type(&m.event_type)
                .ok_or_else(|| ToolError::bad_args(format!("unknown event_type {:?}", m.event_type)))?;
            events.push(Event::Mouse(MouseEvent {
                event_type,
                position: Point { x: m.x, y: m.y },
                global_position: Point {
                    x: m.global_x.unwrap_or(m.x),
                    y: m.global_y.unwrap_or(m.y),
                },
                buttons: MouseButtons {
                    left: m.buttons.left,
                    right: m.buttons.right,
                    middle: m.buttons.middle,
                },
            }));
        }

        let injected = events.len();
        for ev in events {
            crate::window::process_event(ev);
        }

        let body = json!({
            "injected": injected,
        });
        let json = serde_json::to_string(&body)
            .map_err(|e| ToolError::tool_failed(format!("serialize: {}", e)))?;
        Ok(ToolResult::json_only(json))
    }
}

fn parse_key_code(s: &str) -> Option<KeyCode> {
    Some(match s {
        "A" => KeyCode::A, "B" => KeyCode::B, "C" => KeyCode::C, "D" => KeyCode::D,
        "E" => KeyCode::E, "F" => KeyCode::F, "G" => KeyCode::G, "H" => KeyCode::H,
        "I" => KeyCode::I, "J" => KeyCode::J, "K" => KeyCode::K, "L" => KeyCode::L,
        "M" => KeyCode::M, "N" => KeyCode::N, "O" => KeyCode::O, "P" => KeyCode::P,
        "Q" => KeyCode::Q, "R" => KeyCode::R, "S" => KeyCode::S, "T" => KeyCode::T,
        "U" => KeyCode::U, "V" => KeyCode::V, "W" => KeyCode::W, "X" => KeyCode::X,
        "Y" => KeyCode::Y, "Z" => KeyCode::Z,
        "0" | "Key0" => KeyCode::Key0, "1" | "Key1" => KeyCode::Key1,
        "2" | "Key2" => KeyCode::Key2, "3" | "Key3" => KeyCode::Key3,
        "4" | "Key4" => KeyCode::Key4, "5" | "Key5" => KeyCode::Key5,
        "6" | "Key6" => KeyCode::Key6, "7" | "Key7" => KeyCode::Key7,
        "8" | "Key8" => KeyCode::Key8, "9" | "Key9" => KeyCode::Key9,
        "Escape" => KeyCode::Escape,
        "Enter" => KeyCode::Enter,
        "Space" => KeyCode::Space,
        "Tab" => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Insert" => KeyCode::Insert,
        "LeftShift" => KeyCode::LeftShift,
        "RightShift" => KeyCode::RightShift,
        "LeftCtrl" => KeyCode::LeftCtrl,
        "RightCtrl" => KeyCode::RightCtrl,
        "LeftAlt" => KeyCode::LeftAlt,
        "RightAlt" => KeyCode::RightAlt,
        "F1" => KeyCode::F1, "F2" => KeyCode::F2, "F3" => KeyCode::F3,
        "F4" => KeyCode::F4, "F5" => KeyCode::F5, "F6" => KeyCode::F6,
        "F7" => KeyCode::F7, "F8" => KeyCode::F8, "F9" => KeyCode::F9,
        "F10" => KeyCode::F10, "F11" => KeyCode::F11, "F12" => KeyCode::F12,
        "Comma" => KeyCode::Comma,
        "Period" => KeyCode::Period,
        "Slash" => KeyCode::Slash,
        "Semicolon" => KeyCode::Semicolon,
        "Quote" => KeyCode::Quote,
        "LeftBracket" => KeyCode::LeftBracket,
        "RightBracket" => KeyCode::RightBracket,
        "Backslash" => KeyCode::Backslash,
        "Minus" => KeyCode::Minus,
        "Equals" => KeyCode::Equals,
        "Backtick" => KeyCode::Backtick,
        _ => return None,
    })
}

fn parse_mouse_event_type(s: &str) -> Option<MouseEventType> {
    Some(match s {
        "Move" | "move" => MouseEventType::Move,
        "ButtonDown" | "button_down" => MouseEventType::ButtonDown,
        "ButtonUp" | "button_up" => MouseEventType::ButtonUp,
        "Scroll" | "scroll" => MouseEventType::Scroll,
        _ => return None,
    })
}
