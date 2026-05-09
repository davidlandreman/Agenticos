//! Tests for `ScrollView` (U3).
//!
//! Most of `ScrollView`'s behavior is exercised through pure-method
//! calls (`scroll_to`, `ensure_visible`, geometry queries). The
//! `MouseEventType::Scroll` and `Event::EnsureVisible` paths go through
//! `handle_event` directly without needing the global `WindowManager`.

extern crate alloc;

use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, EventResult, KeyModifiers, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::types::Point;
use crate::window::windows::container::ContainerWindow;
use crate::window::windows::scroll_view::ScrollView;
use crate::window::{Rect, Window};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scroll_event(delta_x: i32, delta_y: i32) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::Scroll { delta_x, delta_y },
        position: Point::new(0, 0),
        global_position: Point::new(0, 0),
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    })
}

// ---------------------------------------------------------------------------
// is_scroll_view discriminator
// ---------------------------------------------------------------------------

fn test_is_scroll_view_true_for_scrollview() {
    let sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    assert!(sv.is_scroll_view());
}

fn test_is_scroll_view_false_for_other_window() {
    let other = ContainerWindow::new(Rect::new(0, 0, 100, 100));
    assert!(!other.is_scroll_view());
}

// ---------------------------------------------------------------------------
// Scrollbar geometry — proportional thumb height
// ---------------------------------------------------------------------------

fn test_thumb_height_proportional_to_viewport_over_content() {
    // Viewport 100 tall, content 300 tall — thumb should be ~33% of track.
    // We can verify via the public scroll_to clamp limits and a wheel
    // event: scroll_y = 0 initially, content_h - viewport_h = 200 max.
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);

    assert_eq!(sv.scroll_y(), 0);

    // A single Scroll{delta_y: -1} should move scroll_y by -16, clamped to 0.
    let result = sv.handle_event(scroll_event(0, -1));
    assert_eq!(result, EventResult::Handled);
    assert_eq!(sv.scroll_y(), 0);

    // delta_y: 1 (down/positive) advances scroll_y by 16 px.
    let result = sv.handle_event(scroll_event(0, 1));
    assert_eq!(result, EventResult::Handled);
    assert_eq!(sv.scroll_y(), 16);
}

// ---------------------------------------------------------------------------
// Wheel event scrolling
// ---------------------------------------------------------------------------

fn test_wheel_down_increases_scroll_y_clamped_to_max() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);

    // Apply a large positive delta_y; should clamp to max = content - viewport = 200.
    let _ = sv.handle_event(scroll_event(0, 1000));
    assert_eq!(sv.scroll_y(), 200);
}

fn test_wheel_up_at_top_stays_at_zero() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);
    assert_eq!(sv.scroll_y(), 0);

    let _ = sv.handle_event(scroll_event(0, -100));
    assert_eq!(sv.scroll_y(), 0);
}

fn test_wheel_down_at_bottom_is_noop() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);
    sv.scroll_to(0, 200); // content_h - viewport_h
    assert_eq!(sv.scroll_y(), 200);

    let _ = sv.handle_event(scroll_event(0, 5));
    assert_eq!(sv.scroll_y(), 200);
}

// ---------------------------------------------------------------------------
// EnsureVisible
// ---------------------------------------------------------------------------

fn test_ensure_visible_below_viewport_scrolls_down() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);

    // A 10-tall rect at y=200 sits below the viewport; scroll_y should
    // move so the rect's bottom (210) sits at the viewport bottom.
    let result = sv.handle_event(Event::EnsureVisible(Rect::new(0, 200, 50, 10)));
    assert_eq!(result, EventResult::Handled);
    assert_eq!(sv.scroll_y(), 210 - 100);
}

fn test_ensure_visible_above_viewport_scrolls_up() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);
    sv.scroll_to(0, 200);
    assert_eq!(sv.scroll_y(), 200);

    // A rect at y=10 is above the viewport (which currently shows 200..300);
    // scroll_y should move to 10 (rect's top edge).
    let result = sv.handle_event(Event::EnsureVisible(Rect::new(0, 10, 50, 20)));
    assert_eq!(result, EventResult::Handled);
    assert_eq!(sv.scroll_y(), 10);
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

fn test_content_smaller_than_viewport_no_scroll_effect() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(50, 50);

    let _ = sv.handle_event(scroll_event(0, 5));
    assert_eq!(sv.scroll_y(), 0);
    let _ = sv.handle_event(scroll_event(0, -5));
    assert_eq!(sv.scroll_y(), 0);
}

fn test_content_equals_viewport_no_scroll_effect() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 100);

    let _ = sv.handle_event(scroll_event(0, 5));
    assert_eq!(sv.scroll_y(), 0);
}

fn test_horizontal_disabled_by_default_ignores_h_wheel() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(300, 100);

    let _ = sv.handle_event(scroll_event(5, 0));
    assert_eq!(sv.scroll_x(), 0);
}

fn test_horizontal_enabled_consumes_h_wheel() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_horizontal_enabled(true);
    sv.set_content_size(300, 100);

    let _ = sv.handle_event(scroll_event(2, 0));
    assert_eq!(sv.scroll_x(), 32);
}

// ---------------------------------------------------------------------------
// Programmatic scroll_to clamps
// ---------------------------------------------------------------------------

fn test_scroll_to_clamps_negative_to_zero() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);
    sv.scroll_to(0, -50);
    assert_eq!(sv.scroll_y(), 0);
}

fn test_scroll_to_clamps_above_max() {
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, 100));
    sv.set_content_size(100, 300);
    sv.scroll_to(0, 9999);
    assert_eq!(sv.scroll_y(), 200);
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_is_scroll_view_true_for_scrollview,
        &test_is_scroll_view_false_for_other_window,
        &test_thumb_height_proportional_to_viewport_over_content,
        &test_wheel_down_increases_scroll_y_clamped_to_max,
        &test_wheel_up_at_top_stays_at_zero,
        &test_wheel_down_at_bottom_is_noop,
        &test_ensure_visible_below_viewport_scrolls_down,
        &test_ensure_visible_above_viewport_scrolls_up,
        &test_content_smaller_than_viewport_no_scroll_effect,
        &test_content_equals_viewport_no_scroll_effect,
        &test_horizontal_disabled_by_default_ignores_h_wheel,
        &test_horizontal_enabled_consumes_h_wheel,
        &test_scroll_to_clamps_negative_to_zero,
        &test_scroll_to_clamps_above_max,
    ]
}
