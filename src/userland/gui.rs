//! Per-process ring-3 GUI ownership and event queues.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::userland::lifecycle::{Ring3BlockReason, KERNEL_PID, PROCESS_TABLE};
use crate::window::event::{Event, KeyCode, MouseEventType};
use crate::window::WindowId;

pub const GUI_ABI_VERSION: u32 = 1;
pub const GUI_EVENT_QUEUE_CAPACITY: usize = 128;
pub const GUI_NONBLOCK: u64 = 1;
pub const GUI_PIXEL_FORMAT_XRGB8888: u32 = 1;

pub const GUI_EVENT_KEY: u32 = 1;
pub const GUI_EVENT_MOUSE: u32 = 2;
pub const GUI_EVENT_RESIZE: u32 = 3;
pub const GUI_EVENT_CLOSE: u32 = 4;
pub const GUI_EVENT_FOCUS_CHANGE: u32 = 5;
pub const GUI_EVENT_THEME_CHANGED: u32 = 6;
pub const GUI_EVENT_SETTINGS_CHANGED: u32 = 7;

pub const GUI_MOUSE_MOVE: u32 = 0;
pub const GUI_MOUSE_DOWN: u32 = 1;
pub const GUI_MOUSE_UP: u32 = 2;
pub const GUI_MOUSE_SCROLL: u32 = 3;

/// Version-1 GUI event. Its fixed layout is mirrored by `userland/runtime`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuiEvent {
    pub kind: u32,
    pub window: u32,
    pub payload: [u32; 6],
}

const _: [(); 32] = [(); core::mem::size_of::<GuiEvent>()];

#[derive(Debug, Clone, Copy)]
pub struct GuiWindowRecord {
    pub frame_id: WindowId,
    pub surface_id: WindowId,
}

struct GuiProcessState {
    next_handle: u32,
    windows: BTreeMap<u32, GuiWindowRecord>,
    events: VecDeque<GuiEvent>,
}

impl GuiProcessState {
    const fn new() -> Self {
        Self {
            next_handle: 1,
            windows: BTreeMap::new(),
            events: VecDeque::new(),
        }
    }
}

static GUI_STATES: InterruptMutex<BTreeMap<u32, GuiProcessState>> =
    InterruptMutex::new(BTreeMap::new());

/// Owned `(pid, window_count, queued_events)` rows for
/// `/proc/agenticos/gui`. One short critical section; no per-window
/// detail leaves this module.
pub fn ownership_snapshot() -> alloc::vec::Vec<(u32, usize, usize)> {
    let states = GUI_STATES.lock();
    states
        .iter()
        .map(|(&pid, state)| (pid, state.windows.len(), state.events.len()))
        .collect()
}

pub fn allocate_handle(pid: u32) -> Result<u32, i64> {
    if pid == KERNEL_PID {
        return Err(crate::userland::abi::EPERM);
    }
    let mut states = GUI_STATES.lock();
    let state = states.entry(pid).or_insert_with(GuiProcessState::new);
    let start = state.next_handle.max(1);
    let mut handle = start;
    loop {
        if !state.windows.contains_key(&handle) {
            state.next_handle = handle.wrapping_add(1).max(1);
            return Ok(handle);
        }
        handle = handle.wrapping_add(1).max(1);
        if handle == start {
            return Err(crate::userland::abi::EMFILE);
        }
    }
}

pub fn register_window(pid: u32, handle: u32, record: GuiWindowRecord) -> Result<(), i64> {
    let mut states = GUI_STATES.lock();
    let state = states.entry(pid).or_insert_with(GuiProcessState::new);
    if state.windows.insert(handle, record).is_some() {
        return Err(crate::userland::abi::EEXIST);
    }
    Ok(())
}

pub fn window_record(pid: u32, handle: u32) -> Option<GuiWindowRecord> {
    GUI_STATES
        .lock()
        .get(&pid)
        .and_then(|state| state.windows.get(&handle).copied())
}

pub fn take_window(pid: u32, handle: u32) -> Option<GuiWindowRecord> {
    GUI_STATES
        .lock()
        .get_mut(&pid)
        .and_then(|state| state.windows.remove(&handle))
}

pub fn enqueue_event(pid: u32, event: GuiEvent) {
    {
        let mut states = GUI_STATES.lock();
        let state = states.entry(pid).or_insert_with(GuiProcessState::new);
        let is_move = event.kind == GUI_EVENT_MOUSE && event.payload[3] == GUI_MOUSE_MOVE;
        if is_move {
            if let Some(last) = state.events.back_mut() {
                if last.kind == GUI_EVENT_MOUSE
                    && last.window == event.window
                    && last.payload[3] == GUI_MOUSE_MOVE
                {
                    *last = event;
                } else {
                    push_bounded(state, event);
                }
            } else {
                state.events.push_back(event);
            }
        } else {
            push_bounded(state, event);
        }
    }
    wake_ring3_blocked_on_gui_event(pid);
}

/// Notify every GUI-owning process of a process-global theme transition.
/// A queued older transition is replaced so rapid changes converge without
/// consuming event-queue capacity.
pub fn broadcast_theme_changed(
    effective: crate::window::theme::ThemeKind,
    requested: crate::window::theme::ThemeRequest,
) {
    let event = GuiEvent {
        kind: GUI_EVENT_THEME_CHANGED,
        window: 0,
        payload: [
            match effective {
                crate::window::theme::ThemeKind::Classic => 1,
                crate::window::theme::ThemeKind::Aero => 2,
            },
            match requested {
                crate::window::theme::ThemeRequest::Auto => 0,
                crate::window::theme::ThemeRequest::Classic => 1,
                crate::window::theme::ThemeRequest::Aero => 2,
            },
            0,
            0,
            0,
            0,
        ],
    };
    broadcast_global_event(event);
}

/// Notify GUI-owning processes that non-theme system settings changed.
/// Control Center uses this to refresh other open instances after a live
/// wallpaper update.
pub fn broadcast_settings_changed() {
    broadcast_global_event(GuiEvent {
        kind: GUI_EVENT_SETTINGS_CHANGED,
        window: 0,
        payload: [0; 6],
    });
}

fn broadcast_global_event(event: GuiEvent) {
    let wake: Vec<u32> = {
        let mut states = GUI_STATES.lock();
        states
            .iter_mut()
            .filter_map(|(&pid, state)| {
                if state.windows.is_empty() {
                    return None;
                }
                if let Some(queued) = state
                    .events
                    .iter_mut()
                    .find(|queued| queued.kind == event.kind && queued.window == 0)
                {
                    *queued = event;
                } else {
                    push_bounded(state, event);
                }
                Some(pid)
            })
            .collect()
    };
    for pid in wake {
        wake_ring3_blocked_on_gui_event(pid);
    }
}

fn push_bounded(state: &mut GuiProcessState, event: GuiEvent) {
    if state.events.len() == GUI_EVENT_QUEUE_CAPACITY {
        let _ = state.events.pop_front();
        crate::debug_warn!("ring3 GUI event queue full; dropping oldest event");
    }
    state.events.push_back(event);
}

pub fn pop_event(pid: u32) -> Option<GuiEvent> {
    GUI_STATES
        .lock()
        .get_mut(&pid)
        .and_then(|state| state.events.pop_front())
}

fn wake_ring3_blocked_on_gui_event(pid: u32) {
    if pid == KERNEL_PID {
        return;
    }
    let should_wake = {
        let mut table = PROCESS_TABLE.lock();
        if matches!(
            table.ring3_blocked.get(&pid),
            Some(Ring3BlockReason::WaitingForGuiEvent)
        ) {
            table.ring3_blocked.remove(&pid);
            true
        } else {
            false
        }
    };
    if should_wake {
        crate::userland::lifecycle::mark_ring3_ready(pid);
    }
}

/// Drain GUI state before taking the window-manager lock, keeping lock order
/// one-way even when cleanup follows a fault.
pub fn cleanup_process(pid: u32) {
    crate::userland::gui_gl::cleanup_process(pid);
    let records: Vec<GuiWindowRecord> = GUI_STATES
        .lock()
        .remove(&pid)
        .map(|state| state.windows.into_values().collect())
        .unwrap_or_default();
    if records.is_empty() {
        return;
    }
    let _ = crate::window::with_window_manager(|wm| {
        for record in records {
            wm.destroy_window(record.frame_id);
        }
    });
}

pub fn encode_window_event(handle: u32, event: &Event) -> Option<GuiEvent> {
    let mut encoded = GuiEvent {
        window: handle,
        ..GuiEvent::default()
    };
    match event {
        Event::Keyboard(key) => {
            encoded.kind = GUI_EVENT_KEY;
            encoded.payload[0] = encode_key_code(key.key_code);
            encoded.payload[1] = key_char(key.key_code, key.modifiers.shift) as u32;
            encoded.payload[2] = modifier_bits(key.modifiers);
            encoded.payload[3] = u32::from(key.pressed);
        }
        Event::Mouse(mouse) => {
            encoded.kind = GUI_EVENT_MOUSE;
            encoded.payload[0] = mouse.position.x as u32;
            encoded.payload[1] = mouse.position.y as u32;
            encoded.payload[2] = u32::from(mouse.buttons.left)
                | (u32::from(mouse.buttons.right) << 1)
                | (u32::from(mouse.buttons.middle) << 2)
                | (modifier_bits(mouse.modifiers) << 8);
            match mouse.event_type {
                MouseEventType::Move => encoded.payload[3] = GUI_MOUSE_MOVE,
                MouseEventType::ButtonDown => {
                    encoded.payload[3] = GUI_MOUSE_DOWN;
                    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
                    encoded.payload[4] = ticks as u32;
                    encoded.payload[5] = (ticks >> 32) as u32;
                }
                MouseEventType::ButtonUp => {
                    encoded.payload[3] = GUI_MOUSE_UP;
                    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
                    encoded.payload[4] = ticks as u32;
                    encoded.payload[5] = (ticks >> 32) as u32;
                }
                MouseEventType::Scroll { delta_x, delta_y } => {
                    encoded.payload[3] = GUI_MOUSE_SCROLL;
                    encoded.payload[4] = delta_x as u32;
                    encoded.payload[5] = delta_y as u32;
                }
            }
        }
        Event::Resize(resize) => {
            encoded.kind = GUI_EVENT_RESIZE;
            encoded.payload[0] = resize.width;
            encoded.payload[1] = resize.height;
        }
        Event::Close(close) => {
            let _frame_id = close.window;
            encoded.kind = GUI_EVENT_CLOSE;
        }
        Event::Focus(focus) => {
            encoded.kind = GUI_EVENT_FOCUS_CHANGE;
            encoded.payload[0] = u32::from(focus.gained);
        }
        Event::Move(_) | Event::EnsureVisible(_) => return None,
    }
    Some(encoded)
}

fn modifier_bits(modifiers: crate::window::event::KeyModifiers) -> u32 {
    u32::from(modifiers.shift)
        | (u32::from(modifiers.ctrl) << 1)
        | (u32::from(modifiers.alt) << 2)
        | (u32::from(modifiers.meta) << 3)
}

fn encode_key_code(key: KeyCode) -> u32 {
    use KeyCode::*;
    match key {
        Unknown => 0,
        A => 1,
        B => 2,
        C => 3,
        D => 4,
        E => 5,
        F => 6,
        G => 7,
        H => 8,
        I => 9,
        J => 10,
        K => 11,
        L => 12,
        M => 13,
        N => 14,
        O => 15,
        P => 16,
        Q => 17,
        R => 18,
        S => 19,
        T => 20,
        U => 21,
        V => 22,
        W => 23,
        X => 24,
        Y => 25,
        Z => 26,
        Key0 => 27,
        Key1 => 28,
        Key2 => 29,
        Key3 => 30,
        Key4 => 31,
        Key5 => 32,
        Key6 => 33,
        Key7 => 34,
        Key8 => 35,
        Key9 => 36,
        Escape => 37,
        Enter => 38,
        Space => 39,
        Tab => 40,
        Backspace => 41,
        Delete => 42,
        Left => 43,
        Right => 44,
        Up => 45,
        Down => 46,
        Home => 47,
        End => 48,
        PageUp => 49,
        PageDown => 50,
        Insert => 51,
        LeftShift => 52,
        RightShift => 53,
        LeftCtrl => 54,
        RightCtrl => 55,
        LeftAlt => 56,
        RightAlt => 57,
        F1 => 58,
        F2 => 59,
        F3 => 60,
        F4 => 61,
        F5 => 62,
        F6 => 63,
        F7 => 64,
        F8 => 65,
        F9 => 66,
        F10 => 67,
        F11 => 68,
        F12 => 69,
        Comma => 70,
        Period => 71,
        Slash => 72,
        Semicolon => 73,
        Quote => 74,
        LeftBracket => 75,
        RightBracket => 76,
        Backslash => 77,
        Minus => 78,
        Equals => 79,
        Backtick => 80,
    }
}

fn key_char(key: KeyCode, shift: bool) -> char {
    use KeyCode::*;
    match key {
        A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U | V
        | W | X | Y | Z => {
            let base = b'a' + (encode_key_code(key) - 1) as u8;
            if shift {
                (base - b'a' + b'A') as char
            } else {
                base as char
            }
        }
        Key0 => {
            if shift {
                ')'
            } else {
                '0'
            }
        }
        Key1 => {
            if shift {
                '!'
            } else {
                '1'
            }
        }
        Key2 => {
            if shift {
                '@'
            } else {
                '2'
            }
        }
        Key3 => {
            if shift {
                '#'
            } else {
                '3'
            }
        }
        Key4 => {
            if shift {
                '$'
            } else {
                '4'
            }
        }
        Key5 => {
            if shift {
                '%'
            } else {
                '5'
            }
        }
        Key6 => {
            if shift {
                '^'
            } else {
                '6'
            }
        }
        Key7 => {
            if shift {
                '&'
            } else {
                '7'
            }
        }
        Key8 => {
            if shift {
                '*'
            } else {
                '8'
            }
        }
        Key9 => {
            if shift {
                '('
            } else {
                '9'
            }
        }
        Space => ' ',
        Tab => '\t',
        Enter => '\n',
        Comma => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        Period => {
            if shift {
                '>'
            } else {
                '.'
            }
        }
        Slash => {
            if shift {
                '?'
            } else {
                '/'
            }
        }
        Semicolon => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        Quote => {
            if shift {
                '"'
            } else {
                '\''
            }
        }
        LeftBracket => {
            if shift {
                '{'
            } else {
                '['
            }
        }
        RightBracket => {
            if shift {
                '}'
            } else {
                ']'
            }
        }
        Backslash => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        Minus => {
            if shift {
                '_'
            } else {
                '-'
            }
        }
        Equals => {
            if shift {
                '+'
            } else {
                '='
            }
        }
        Backtick => {
            if shift {
                '~'
            } else {
                '`'
            }
        }
        _ => '\0',
    }
}

#[cfg(feature = "test")]
pub fn reset_for_test() {
    GUI_STATES.lock().clear();
    super::gui_gl::reset_for_test();
}

#[cfg(feature = "test")]
pub fn event_count_for_test(pid: u32) -> usize {
    GUI_STATES
        .lock()
        .get(&pid)
        .map(|state| state.events.len())
        .unwrap_or(0)
}

#[cfg(feature = "test")]
pub fn window_count_for_test(pid: u32) -> usize {
    GUI_STATES
        .lock()
        .get(&pid)
        .map(|state| state.windows.len())
        .unwrap_or(0)
}
