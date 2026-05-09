//! Tests for U16 — `MouseEvent.modifiers`, `MouseEventType::Scroll {dx,dy}`,
//! and `Event::EnsureVisible(Rect)`.
//!
//! These exercise *type shape* and routing — the new fields and variants
//! must construct, pattern-match, and survive a round-trip through
//! `WindowManager::route_mouse_event`.

extern crate alloc;

use alloc::boxed::Box;

use spin::Mutex;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, EventResult, KeyModifiers, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::types::Point;
use crate::window::{
    ColorDepth, GraphicsDevice, Rect, ScreenMode, Window, WindowId, WindowManager,
};

// ---------------------------------------------------------------------------
// Synthetic Scroll construction and pattern matching
// ---------------------------------------------------------------------------

fn test_scroll_variant_constructs_and_destructures() {
    let ev = MouseEventType::Scroll { delta_x: 0, delta_y: -3 };
    match ev {
        MouseEventType::Scroll { delta_x, delta_y } => {
            assert_eq!(delta_x, 0);
            assert_eq!(delta_y, -3);
        }
        _ => panic!("expected Scroll variant"),
    }
}

fn test_scroll_variant_extreme_deltas() {
    // Should construct and pattern-match without panic.
    let ev = MouseEventType::Scroll {
        delta_x: i32::MAX,
        delta_y: i32::MIN,
    };
    match ev {
        MouseEventType::Scroll { delta_x, delta_y } => {
            assert_eq!(delta_x, i32::MAX);
            assert_eq!(delta_y, i32::MIN);
        }
        _ => panic!("expected Scroll variant"),
    }
}

// ---------------------------------------------------------------------------
// MouseEvent.modifiers default and explicit construction
// ---------------------------------------------------------------------------

fn test_mouse_event_default_modifiers_all_false() {
    let ev = MouseEvent {
        event_type: MouseEventType::Move,
        position: Point::new(0, 0),
        global_position: Point::new(0, 0),
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    };
    assert!(!ev.modifiers.shift);
    assert!(!ev.modifiers.ctrl);
    assert!(!ev.modifiers.alt);
    assert!(!ev.modifiers.meta);
}

fn test_mouse_event_with_shift_modifier_preserved() {
    let ev = MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: Point::new(10, 10),
        global_position: Point::new(10, 10),
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
    };
    assert!(ev.modifiers.shift);
    assert!(!ev.modifiers.ctrl);
    assert!(!ev.modifiers.alt);
    assert!(!ev.modifiers.meta);
}

// ---------------------------------------------------------------------------
// Event::EnsureVisible variant exists and carries a Rect
// ---------------------------------------------------------------------------

fn test_ensure_visible_variant_constructs() {
    let r = Rect::new(5, 6, 7, 8);
    let ev = Event::EnsureVisible(r);
    match ev {
        Event::EnsureVisible(rect) => {
            assert_eq!(rect.x, 5);
            assert_eq!(rect.y, 6);
            assert_eq!(rect.width, 7);
            assert_eq!(rect.height, 8);
        }
        _ => panic!("expected EnsureVisible variant"),
    }
}

// ---------------------------------------------------------------------------
// Routing — modifiers survive WindowManager::route_mouse_event
// ---------------------------------------------------------------------------

/// Minimal `GraphicsDevice` stub — `route_mouse_event` doesn't paint, but
/// the `WindowManager` needs *some* device.
struct StubDevice;

impl GraphicsDevice for StubDevice {
    fn width(&self) -> usize { 1280 }
    fn height(&self) -> usize { 720 }
    fn color_depth(&self) -> ColorDepth { ColorDepth::Bit32 }
    fn clear(&mut self, _color: Color) {}
    fn draw_pixel(&mut self, _x: i32, _y: i32, _color: Color) {}
    fn read_pixel(&self, _x: i32, _y: i32) -> Color { Color::BLACK }
    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn fill_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn set_clip_rect(&mut self, _rect: Option<Rect>) {}
    fn flush(&mut self) {}
}

/// Side-channel slot the recording window writes into when it receives a
/// mouse event. Avoids needing trait downcasting to inspect a window the
/// `WindowManager` owns.
static LAST_MOUSE: Mutex<Option<MouseEvent>> = Mutex::new(None);

/// Window stub that records the most recent `MouseEvent` it received via
/// `handle_event` into the static `LAST_MOUSE` slot. Used to assert that
/// `route_mouse_event` preserves the new `modifiers` field through to the
/// target window.
struct RecordingWindow {
    base: crate::window::windows::base::WindowBase,
}

impl RecordingWindow {
    fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            base: crate::window::windows::base::WindowBase::new_with_id(id, bounds),
        }
    }
}

impl Window for RecordingWindow {
    fn base(&self) -> &crate::window::windows::base::WindowBase { &self.base }
    fn base_mut(&mut self) -> &mut crate::window::windows::base::WindowBase { &mut self.base }
    fn paint(&mut self, _device: &mut dyn GraphicsDevice) {}
    fn handle_event(&mut self, event: Event) -> EventResult {
        if let Event::Mouse(m) = event {
            *LAST_MOUSE.lock() = Some(m);
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }
    fn can_focus(&self) -> bool { true }
}

fn test_route_mouse_event_preserves_modifiers() {
    // Reset side-channel.
    *LAST_MOUSE.lock() = None;

    // Build a fresh WindowManager with a stub device and a single root
    // window covering the screen.
    let mut wm = WindowManager::new(Box::new(StubDevice));

    // Create a Gui screen with a root window.
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 1280, 720));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    // Synthesize a mouse event with shift held. `global_position` is what
    // `route_mouse_event` uses to hit-test.
    let synthetic = MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: Point::new(0, 0), // router overwrites with local coords
        global_position: Point::new(100, 100),
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
            meta: false,
        },
    };

    wm.route_mouse_event(synthetic);

    let guard = LAST_MOUSE.lock();
    let recorded = guard
        .as_ref()
        .expect("root window should have received the mouse event");

    assert!(
        recorded.modifiers.shift,
        "shift modifier must survive route_mouse_event"
    );
    assert!(!recorded.modifiers.ctrl);
    assert!(!recorded.modifiers.alt);
    assert!(!recorded.modifiers.meta);
    // Root is at (0, 0), so local == global.
    assert_eq!(recorded.position, Point::new(100, 100));
    assert_eq!(recorded.global_position, Point::new(100, 100));
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_scroll_variant_constructs_and_destructures,
        &test_scroll_variant_extreme_deltas,
        &test_mouse_event_default_modifiers_all_false,
        &test_mouse_event_with_shift_modifier_preserved,
        &test_ensure_visible_variant_constructs,
        &test_route_mouse_event_preserves_modifiers,
    ]
}
