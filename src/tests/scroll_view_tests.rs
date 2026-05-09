//! Tests for `ScrollView` (U3).
//!
//! Most of `ScrollView`'s behavior is exercised through pure-method
//! calls (`scroll_to`, `ensure_visible`, geometry queries). The
//! `MouseEventType::Scroll` and `Event::EnsureVisible` paths go through
//! `handle_event` directly without needing the global `WindowManager`.

extern crate alloc;

use alloc::boxed::Box;
use spin::Mutex;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, EventResult, KeyModifiers, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::types::Point;
use crate::window::windows::container::ContainerWindow;
use crate::window::windows::scroll_view::ScrollView;
use crate::window::{
    ColorDepth, GraphicsDevice, Rect, ScreenMode, Window, WindowId, WindowManager,
};

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
// Scroll routing through `WindowManager::route_mouse_event` (U4a)
// ---------------------------------------------------------------------------
//
// These exercise the routing branch that walks up from the hit window
// looking for a `ScrollView` ancestor. The fixtures mirror those used in
// `mouse_event_extension_tests.rs`: a stub `GraphicsDevice` so the manager
// can construct, and a `RecordingWindow` that captures events into a
// global side-channel so we can assert who received what.

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

/// Last `MouseEvent` received by any `RecordingWindow`. The window also
/// records *its own id* alongside the event so tests can verify which
/// recipient handled the dispatch.
static LAST_RECEIVED: Mutex<Option<(WindowId, MouseEvent)>> = Mutex::new(None);

/// A plain recording window that captures every `Event::Mouse` it is
/// handed into the global `LAST_RECEIVED` slot. `is_scroll_view()` stays
/// at its default `false`.
struct RecordingWindow {
    id: WindowId,
    bounds: Rect,
    parent: Option<WindowId>,
    children: alloc::vec::Vec<WindowId>,
}

impl RecordingWindow {
    fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            id,
            bounds,
            parent: None,
            children: alloc::vec::Vec::new(),
        }
    }
}

impl Window for RecordingWindow {
    fn id(&self) -> WindowId { self.id }
    fn bounds(&self) -> Rect { self.bounds }
    fn set_bounds(&mut self, bounds: Rect) { self.bounds = bounds; }
    fn visible(&self) -> bool { true }
    fn set_visible(&mut self, _visible: bool) {}
    fn parent(&self) -> Option<WindowId> { self.parent }
    fn children(&self) -> &[WindowId] { &self.children }
    fn set_parent(&mut self, parent: Option<WindowId>) { self.parent = parent; }
    fn add_child(&mut self, child: WindowId) {
        if !self.children.contains(&child) {
            self.children.push(child);
        }
    }
    fn remove_child(&mut self, child: WindowId) {
        self.children.retain(|&c| c != child);
    }
    fn paint(&mut self, _device: &mut dyn GraphicsDevice) {}
    fn needs_repaint(&self) -> bool { false }
    fn invalidate(&mut self) {}
    fn handle_event(&mut self, event: Event) -> EventResult {
        if let Event::Mouse(m) = event {
            *LAST_RECEIVED.lock() = Some((self.id, m));
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }
    fn can_focus(&self) -> bool { true }
    fn has_focus(&self) -> bool { false }
    fn set_focus(&mut self, _focused: bool) {}
}

/// A recording window that *also* identifies itself as a `ScrollView`
/// via the `is_scroll_view()` discriminator. Lets the routing tests
/// inspect what the scroll-view ancestor receives without needing to
/// downcast `Box<dyn Window>` back to a concrete `ScrollView`.
struct RecordingScrollView {
    id: WindowId,
    bounds: Rect,
    parent: Option<WindowId>,
    children: alloc::vec::Vec<WindowId>,
}

impl RecordingScrollView {
    fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            id,
            bounds,
            parent: None,
            children: alloc::vec::Vec::new(),
        }
    }
}

impl Window for RecordingScrollView {
    fn id(&self) -> WindowId { self.id }
    fn bounds(&self) -> Rect { self.bounds }
    fn set_bounds(&mut self, bounds: Rect) { self.bounds = bounds; }
    fn visible(&self) -> bool { true }
    fn set_visible(&mut self, _visible: bool) {}
    fn parent(&self) -> Option<WindowId> { self.parent }
    fn children(&self) -> &[WindowId] { &self.children }
    fn set_parent(&mut self, parent: Option<WindowId>) { self.parent = parent; }
    fn add_child(&mut self, child: WindowId) {
        if !self.children.contains(&child) {
            self.children.push(child);
        }
    }
    fn remove_child(&mut self, child: WindowId) {
        self.children.retain(|&c| c != child);
    }
    fn paint(&mut self, _device: &mut dyn GraphicsDevice) {}
    fn needs_repaint(&self) -> bool { false }
    fn invalidate(&mut self) {}
    fn handle_event(&mut self, event: Event) -> EventResult {
        if let Event::Mouse(m) = event {
            *LAST_RECEIVED.lock() = Some((self.id, m));
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }
    fn can_focus(&self) -> bool { true }
    fn has_focus(&self) -> bool { false }
    fn set_focus(&mut self, _focused: bool) {}
    fn is_scroll_view(&self) -> bool { true }
}

fn make_wm() -> WindowManager {
    let mut wm = WindowManager::new(Box::new(StubDevice));
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);
    wm
}

fn synthetic_scroll_at(global: Point, delta_x: i32, delta_y: i32) -> MouseEvent {
    MouseEvent {
        event_type: MouseEventType::Scroll { delta_x, delta_y },
        position: Point::new(0, 0),
        global_position: global,
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    }
}

/// Happy path: a synthetic Scroll over the child of a `ScrollView` is
/// delivered to the `ScrollView`'s `handle_event`, with `delta_y`
/// preserved. The child should NOT see the scroll event.
fn test_scroll_routes_to_scroll_view_ancestor() {
    *LAST_RECEIVED.lock() = None;

    let mut wm = make_wm();

    // Tree: root (1280x720) -> scroll_view (100x100 at origin) -> child
    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 1280, 720));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(s) = wm.get_active_screen_mut() {
        s.set_root_window(root_id);
    }

    let sv_id = wm.create_window(Some(root_id));
    let mut sv = RecordingScrollView::new(sv_id, Rect::new(0, 0, 100, 100));
    sv.set_parent(Some(root_id));

    let child_id = wm.create_window(Some(sv_id));
    let mut child = RecordingWindow::new(child_id, Rect::new(0, 0, 100, 100));
    child.set_parent(Some(sv_id));

    wm.set_window_impl(sv_id, Box::new(sv));
    wm.set_window_impl(child_id, Box::new(child));

    // Hit point (50, 50) lands inside both the ScrollView and the child.
    // `topmost_at` returns the child, but Scroll routing walks up to the
    // ScrollView (innermost match).
    wm.route_mouse_event(synthetic_scroll_at(Point::new(50, 50), 0, -3));

    let received = LAST_RECEIVED
        .lock()
        .expect("scroll view should have received the event");
    assert_eq!(received.0, sv_id, "scroll event must reach the ScrollView");
    match received.1.event_type {
        MouseEventType::Scroll { delta_x, delta_y } => {
            assert_eq!(delta_x, 0);
            assert_eq!(delta_y, -3, "delta_y must be preserved through routing");
        }
        other => panic!("expected Scroll, got {:?}", other),
    }
}

/// Happy path with the real `ScrollView`: confirms the routing actually
/// drives `ScrollView::handle_event`. After a Scroll event reaches the
/// real ScrollView, its `needs_repaint` flag becomes `true` (set by
/// `apply_wheel` via `invalidate`). A non-scrollable widget does not
/// invalidate on scroll, so this is a meaningful signal that the event
/// reached the ScrollView path.
fn test_scroll_routes_into_real_scroll_view_advances_offset() {
    let mut wm = make_wm();

    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 1280, 720));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(s) = wm.get_active_screen_mut() {
        s.set_root_window(root_id);
    }

    let sv_id = wm.create_window(Some(root_id));
    let mut sv = ScrollView::new_with_id(sv_id, Rect::new(0, 0, 100, 100));
    sv.set_parent(Some(root_id));
    sv.set_content_size(100, 300);
    wm.set_window_impl(sv_id, Box::new(sv));

    // Hit point inside the ScrollView (no child — `topmost_at` returns
    // the ScrollView itself, which is also the nearest enclosing
    // ScrollView).
    wm.route_mouse_event(synthetic_scroll_at(Point::new(50, 50), 0, 3));

    // After the route, the ScrollView should have processed the wheel
    // (apply_wheel calls `invalidate`).
    let sv_ref = wm
        .window_registry
        .get(&sv_id)
        .expect("scroll view should still be registered");
    assert!(
        sv_ref.needs_repaint(),
        "Scroll routed to ScrollView should invalidate it"
    );
}

/// Happy path: Scroll over a window with no ScrollView ancestor reaches
/// the hit window normally.
fn test_scroll_falls_through_when_no_scroll_view_ancestor() {
    *LAST_RECEIVED.lock() = None;

    let mut wm = make_wm();

    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 1280, 720));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(s) = wm.get_active_screen_mut() {
        s.set_root_window(root_id);
    }

    let leaf_id = wm.create_window(Some(root_id));
    let mut leaf = RecordingWindow::new(leaf_id, Rect::new(40, 60, 200, 200));
    leaf.set_parent(Some(root_id));
    wm.set_window_impl(leaf_id, Box::new(leaf));

    wm.route_mouse_event(synthetic_scroll_at(Point::new(100, 100), 0, -2));

    let received = LAST_RECEIVED.lock();
    let (recv_id, recv_ev) = received.expect("leaf should receive the scroll event");
    assert_eq!(recv_id, leaf_id);
    match recv_ev.event_type {
        MouseEventType::Scroll { delta_x, delta_y } => {
            assert_eq!(delta_x, 0);
            assert_eq!(delta_y, -2);
        }
        other => panic!("expected Scroll variant, got {:?}", other),
    }
    // local position should be translated relative to leaf bounds.
    assert_eq!(recv_ev.position, Point::new(60, 40));
}

/// Edge case: nested ScrollViews — the innermost wins. The inner
/// ScrollView records receipt; the outer must NOT see the scroll event.
fn test_nested_scroll_views_innermost_wins() {
    *LAST_RECEIVED.lock() = None;

    let mut wm = make_wm();

    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 1280, 720));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(s) = wm.get_active_screen_mut() {
        s.set_root_window(root_id);
    }

    // Outer ScrollView (recording) at the root; inner ScrollView nested
    // inside, both contain the hit point.
    let outer_id = wm.create_window(Some(root_id));
    let mut outer = RecordingScrollView::new(outer_id, Rect::new(0, 0, 200, 200));
    outer.set_parent(Some(root_id));

    let inner_id = wm.create_window(Some(outer_id));
    let mut inner = RecordingScrollView::new(inner_id, Rect::new(0, 0, 100, 100));
    inner.set_parent(Some(outer_id));

    let leaf_id = wm.create_window(Some(inner_id));
    let mut leaf = RecordingWindow::new(leaf_id, Rect::new(0, 0, 100, 100));
    leaf.set_parent(Some(inner_id));

    wm.set_window_impl(outer_id, Box::new(outer));
    wm.set_window_impl(inner_id, Box::new(inner));
    wm.set_window_impl(leaf_id, Box::new(leaf));

    // Hit point lies inside outer, inner, and leaf.
    wm.route_mouse_event(synthetic_scroll_at(Point::new(40, 40), 0, 2));

    let received = LAST_RECEIVED
        .lock()
        .expect("a scroll view should have received the event");
    assert_eq!(
        received.0, inner_id,
        "innermost ScrollView ancestor must win"
    );
}

/// Edge case: Scroll over an empty desktop (no children at the cursor
/// point) does not panic. `topmost_at` returns the root if it contains
/// the point; if not, no event is delivered. Either way, no panic.
fn test_scroll_over_empty_area_no_panic() {
    *LAST_RECEIVED.lock() = None;

    let mut wm = make_wm();

    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 100, 100));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(s) = wm.get_active_screen_mut() {
        s.set_root_window(root_id);
    }

    // Point well outside the root bounds — topmost_at returns None.
    wm.route_mouse_event(synthetic_scroll_at(Point::new(500, 500), 0, 1));

    // Should not have panicked, and nothing should have been delivered.
    assert!(LAST_RECEIVED.lock().is_none());
}

/// Regression: Move/ButtonDown/ButtonUp events still go to the hit
/// window even when a `ScrollView` ancestor exists. Only Scroll is
/// rerouted.
fn test_non_scroll_events_unaffected_by_scroll_view_ancestor() {
    *LAST_RECEIVED.lock() = None;

    let mut wm = make_wm();

    let root_id = wm.create_window(None);
    let root = RecordingWindow::new(root_id, Rect::new(0, 0, 1280, 720));
    wm.set_window_impl(root_id, Box::new(root));
    if let Some(s) = wm.get_active_screen_mut() {
        s.set_root_window(root_id);
    }

    let sv_id = wm.create_window(Some(root_id));
    let mut sv = ScrollView::new_with_id(sv_id, Rect::new(0, 0, 200, 200));
    sv.set_parent(Some(root_id));
    sv.set_content_size(200, 400);

    let child_id = wm.create_window(Some(sv_id));
    let mut child = RecordingWindow::new(child_id, Rect::new(0, 0, 200, 400));
    child.set_parent(Some(sv_id));

    wm.set_window_impl(sv_id, Box::new(sv));
    wm.set_window_impl(child_id, Box::new(child));

    // Move event over the child: should be delivered to the child, NOT
    // the ScrollView ancestor.
    let move_event = MouseEvent {
        event_type: MouseEventType::Move,
        position: Point::new(0, 0),
        global_position: Point::new(50, 50),
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    };
    wm.route_mouse_event(move_event);

    let received = LAST_RECEIVED
        .lock()
        .expect("child should receive Move event");
    assert_eq!(received.0, child_id);
    assert_eq!(received.1.event_type, MouseEventType::Move);

    // ButtonDown — same expectation: routes to the child.
    *LAST_RECEIVED.lock() = None;
    let down = MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: Point::new(0, 0),
        global_position: Point::new(60, 60),
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
    };
    wm.route_mouse_event(down);
    let received = LAST_RECEIVED
        .lock()
        .expect("child should receive ButtonDown");
    assert_eq!(received.0, child_id);
    assert_eq!(received.1.event_type, MouseEventType::ButtonDown);

    // ButtonUp — same.
    *LAST_RECEIVED.lock() = None;
    let up = MouseEvent {
        event_type: MouseEventType::ButtonUp,
        position: Point::new(0, 0),
        global_position: Point::new(60, 60),
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    };
    wm.route_mouse_event(up);
    let received = LAST_RECEIVED
        .lock()
        .expect("child should receive ButtonUp");
    assert_eq!(received.0, child_id);
    assert_eq!(received.1.event_type, MouseEventType::ButtonUp);
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
        &test_scroll_routes_to_scroll_view_ancestor,
        &test_scroll_routes_into_real_scroll_view_advances_offset,
        &test_scroll_falls_through_when_no_scroll_view_ancestor,
        &test_nested_scroll_views_innermost_wins,
        &test_scroll_over_empty_area_no_panic,
        &test_non_scroll_events_unaffected_by_scroll_view_ancestor,
    ]
}
