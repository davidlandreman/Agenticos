//! Tests for the U14 `ProgressBar` widget.
//!
//! All cases are pure-method or smoke-painted; nothing here needs the
//! global `WindowManager`. The `GraphicsDevice` stub mirrors the one
//! used in `scroll_view_tests.rs` so the paint smoke test stays
//! independent of the live framebuffer.

extern crate alloc;

use alloc::string::String;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, EventResult, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent,
    MouseEventType,
};
use crate::window::types::Point;
use crate::window::windows::progress_bar::ProgressBar;
use crate::window::{ColorDepth, GraphicsDevice, Rect, Window};

// ---------------------------------------------------------------------------
// Stub GraphicsDevice for paint smoke tests
// ---------------------------------------------------------------------------

struct StubDevice;

impl GraphicsDevice for StubDevice {
    fn width(&self) -> usize {
        1280
    }
    fn height(&self) -> usize {
        720
    }
    fn color_depth(&self) -> ColorDepth {
        ColorDepth::Bit32
    }
    fn clear(&mut self, _color: Color) {}
    fn draw_pixel(&mut self, _x: i32, _y: i32, _color: Color) {}
    fn read_pixel(&self, _x: i32, _y: i32) -> Color {
        Color::BLACK
    }
    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn fill_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn set_clip_rect(&mut self, _rect: Option<Rect>) {}
    fn flush(&mut self) {}
}

// ---------------------------------------------------------------------------
// Helpers — float comparison and synthetic events
// ---------------------------------------------------------------------------

fn approx_eq(a: f32, b: f32) -> bool {
    let diff = if a > b { a - b } else { b - a };
    diff < 1e-4
}

fn mouse_event(event_type: MouseEventType) -> Event {
    Event::Mouse(MouseEvent {
        event_type,
        position: Point::new(10, 10),
        global_position: Point::new(10, 10),
        buttons: MouseButtons {
            left: matches!(event_type, MouseEventType::ButtonDown),
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
    })
}

fn key_event(code: KeyCode) -> Event {
    Event::Keyboard(KeyboardEvent {
        key_code: code,
        pressed: true,
        modifiers: KeyModifiers::default(),
    })
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

/// `set_progress(50, 100)` produces fraction 0.5.
fn test_set_progress_half_fraction() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(50, 100);
    assert_eq!(pb.current(), 50);
    assert_eq!(pb.total(), 100);
    assert!(approx_eq(pb.fraction(), 0.5), "got {}", pb.fraction());
}

/// `set_progress(0, 100)` produces fraction 0.0.
fn test_set_progress_zero_fraction() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(0, 100);
    assert!(approx_eq(pb.fraction(), 0.0), "got {}", pb.fraction());
}

/// `set_progress(100, 100)` produces fraction 1.0.
fn test_set_progress_full_fraction() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(100, 100);
    assert!(approx_eq(pb.fraction(), 1.0), "got {}", pb.fraction());
}

/// `set_label(Some(...))` stores the label; `set_label(None)` clears it.
/// We can't directly observe the stored label without a getter, so we
/// verify that the second call invalidates the widget on transition by
/// painting and re-painting through a stub device. That part is exercised
/// by the smoke test below; here we just verify the API doesn't panic
/// across both states.
fn test_set_label_round_trip() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_label(Some(String::from("47% — Copying file_42.txt")));
    pb.set_label(None);
    pb.set_label(Some(String::from("done")));
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

/// `set_progress(50, 0)` — divide-by-zero path — returns fraction 0.0
/// without panicking, and still paints cleanly.
fn test_set_progress_total_zero_no_panic() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(50, 0);
    assert!(approx_eq(pb.fraction(), 0.0), "got {}", pb.fraction());

    let mut device = StubDevice;
    pb.paint(&mut device); // must not panic
}

/// `set_progress(150, 100)` clamps fraction to 1.0 (no overflow).
fn test_set_progress_overshoot_clamps_to_one() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(150, 100);
    assert!(approx_eq(pb.fraction(), 1.0), "got {}", pb.fraction());
}

/// Extreme `u64::MAX` values still report a sane fraction and paint
/// without panicking — guards against multiplication overflow.
fn test_set_progress_u64_max_no_overflow() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(u64::MAX / 2, u64::MAX);
    let f = pb.fraction();
    // Should be approximately 0.5; precision loss is OK at this magnitude.
    assert!(f >= 0.49 && f <= 0.51, "expected ~0.5, got {}", f);

    let mut device = StubDevice;
    pb.paint(&mut device); // must not panic on huge inner_width math
}

/// `bounds.width = 0` — degenerate — paints without panic.
fn test_zero_width_bounds_paints_without_panic() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 0, 20));
    pb.set_progress(50, 100);
    let mut device = StubDevice;
    pb.paint(&mut device);
}

/// `bounds.width = 1, height = 1` — too small for both border and inner
/// fill — paints without panic.
fn test_one_pixel_bounds_paints_without_panic() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 1, 1));
    pb.set_progress(50, 100);
    let mut device = StubDevice;
    pb.paint(&mut device);
}

/// Painting with a label set, including non-ASCII characters, must not
/// panic and must not crash the centering math.
fn test_paint_with_label_smoke() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));
    pb.set_progress(47, 100);
    pb.set_label(Some(String::from("47% — Copying file_42.txt")));
    let mut device = StubDevice;
    pb.paint(&mut device);
}

// ---------------------------------------------------------------------------
// Event handling — must ignore everything
// ---------------------------------------------------------------------------

/// ProgressBar ignores all event variants — verify each major one
/// returns `EventResult::Ignored`.
fn test_ignores_all_events() {
    let mut pb = ProgressBar::new(Rect::new(0, 0, 200, 20));

    assert_eq!(
        pb.handle_event(mouse_event(MouseEventType::Move)),
        EventResult::Ignored,
    );
    assert_eq!(
        pb.handle_event(mouse_event(MouseEventType::ButtonDown)),
        EventResult::Ignored,
    );
    assert_eq!(
        pb.handle_event(mouse_event(MouseEventType::ButtonUp)),
        EventResult::Ignored,
    );
    assert_eq!(
        pb.handle_event(mouse_event(MouseEventType::Scroll {
            delta_x: 0,
            delta_y: 1,
        })),
        EventResult::Ignored,
    );
    assert_eq!(
        pb.handle_event(key_event(KeyCode::Enter)),
        EventResult::Ignored,
    );
    assert_eq!(
        pb.handle_event(Event::EnsureVisible(Rect::new(0, 0, 10, 10))),
        EventResult::Ignored,
    );
}

// ---------------------------------------------------------------------------
// Test registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_set_progress_half_fraction,
        &test_set_progress_zero_fraction,
        &test_set_progress_full_fraction,
        &test_set_label_round_trip,
        &test_set_progress_total_zero_no_panic,
        &test_set_progress_overshoot_clamps_to_one,
        &test_set_progress_u64_max_no_overflow,
        &test_zero_width_bounds_paints_without_panic,
        &test_one_pixel_bounds_paints_without_panic,
        &test_paint_with_label_smoke,
        &test_ignores_all_events,
    ]
}
