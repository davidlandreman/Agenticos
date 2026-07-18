//! Tests for the U5 `Window` trait default-method delegation.
//!
//! These tests pin the contract that pure plumbing methods (`id`,
//! `bounds`, `visible`, `parent`, `children`, `needs_repaint`,
//! `invalidate`, `set_bounds`, `set_bounds_no_invalidate`, ...) default
//! to a one-line delegation through `base()` / `base_mut()`. They also
//! pin the two known overrides:
//! - widgets that override `can_focus()` win over the default `false`.
//! - `FrameWindow` keeps its own `active` field for `set_focus` /
//!   `has_focus`, NOT the `WindowBase::has_focus` slot.

extern crate alloc;

use crate::lib::test_utils::Testable;
use crate::window::windows::base::WindowBase;
use crate::window::windows::{ContainerWindow, FrameWindow};
use crate::window::{Rect, Window, WindowId};

/// Minimal `Window`-implementing widget used to exercise the default
/// trait methods. It owns nothing but a `WindowBase`, so every default
/// method should round-trip through that `WindowBase`.
struct PlainWidget {
    base: WindowBase,
}

impl PlainWidget {
    fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            base: WindowBase::new_with_id(id, bounds),
        }
    }
}

impl Window for PlainWidget {
    fn base(&self) -> &WindowBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }
    fn paint(&mut self, _device: &mut dyn crate::window::GraphicsDevice) {}
    fn handle_event(&mut self, _event: crate::window::Event) -> crate::window::EventResult {
        crate::window::EventResult::Ignored
    }
    // Deliberately overrides the default `false` so the override test
    // below can verify that overrides win.
    fn can_focus(&self) -> bool {
        true
    }
}

/// Helper: a fresh `WindowId`, kept stable inside one test by reusing
/// it for assertions.
fn fresh_id() -> WindowId {
    WindowId::new()
}

fn test_default_methods_route_through_base() {
    let id = fresh_id();
    let rect = Rect::new(10, 20, 100, 200);
    let widget = PlainWidget::new(id, rect);

    // Every read-only default method must mirror `WindowBase`.
    assert_eq!(widget.id(), id, "id() should delegate to base");
    assert_eq!(widget.bounds(), rect, "bounds() should delegate to base");
    assert!(
        widget.visible(),
        "visible() should delegate to base (default true)"
    );
    assert_eq!(widget.parent(), None, "parent() should delegate to base");
    assert!(
        widget.children().is_empty(),
        "children() should delegate to base"
    );
    // Fresh `WindowBase` defaults to needing a paint.
    assert!(
        widget.needs_repaint(),
        "needs_repaint() should delegate to base (true on construct)"
    );
    // Default `has_focus` is false.
    assert!(!widget.has_focus(), "has_focus() should delegate to base");
}

fn test_can_focus_override_beats_default() {
    let widget = PlainWidget::new(fresh_id(), Rect::new(0, 0, 1, 1));
    // `PlainWidget::can_focus` returns true even though the trait
    // default is false.
    assert!(
        widget.can_focus(),
        "widget override of can_focus() should win over default"
    );

    // A `Spacer` returns the trait default (false) since it does not
    // override `can_focus`.
    let spacer = crate::window::windows::Spacer::new(Rect::new(0, 0, 1, 1));
    assert!(!spacer.can_focus(), "default can_focus() must return false");
}

fn test_invalidate_default_sets_needs_repaint_on_base() {
    let mut widget = PlainWidget::new(fresh_id(), Rect::new(0, 0, 50, 50));
    // Force the dirty bit off, then invalidate via the default impl.
    widget.base_mut().clear_needs_repaint();
    assert!(!widget.needs_repaint());
    widget.invalidate();
    assert!(
        widget.needs_repaint(),
        "default invalidate() should flip WindowBase::needs_repaint"
    );
}

fn test_set_bounds_default_invalidates() {
    let mut widget = PlainWidget::new(fresh_id(), Rect::new(0, 0, 50, 50));
    widget.base_mut().clear_needs_repaint();
    let new_bounds = Rect::new(5, 6, 70, 80);
    widget.set_bounds(new_bounds);
    assert_eq!(widget.bounds(), new_bounds);
    assert!(
        widget.needs_repaint(),
        "default set_bounds() should flip needs_repaint via WindowBase"
    );
}

fn test_set_bounds_no_invalidate_default_does_not_invalidate() {
    let mut widget = PlainWidget::new(fresh_id(), Rect::new(0, 0, 50, 50));
    widget.base_mut().clear_needs_repaint();
    let new_bounds = Rect::new(5, 6, 70, 80);
    widget.set_bounds_no_invalidate(new_bounds);
    assert_eq!(widget.bounds(), new_bounds);
    assert!(
        !widget.needs_repaint(),
        "set_bounds_no_invalidate() must NOT flip needs_repaint"
    );
}

fn test_visible_default_round_trip() {
    let mut widget = PlainWidget::new(fresh_id(), Rect::new(0, 0, 1, 1));
    assert!(widget.visible());
    widget.set_visible(false);
    assert!(!widget.visible());
    widget.set_visible(true);
    assert!(widget.visible());
}

fn test_parent_and_children_default_round_trip() {
    let mut widget = PlainWidget::new(fresh_id(), Rect::new(0, 0, 1, 1));
    let parent = fresh_id();
    let child = fresh_id();
    widget.set_parent(Some(parent));
    widget.add_child(child);
    assert_eq!(widget.parent(), Some(parent));
    assert_eq!(widget.children(), &[child]);
    widget.remove_child(child);
    assert!(widget.children().is_empty());
}

fn test_container_window_id_and_bounds_via_default() {
    // Sanity check that a real widget (which now relies on the default
    // delegations for everything except paint/handle_event/can_focus)
    // still round-trips correctly.
    let id = fresh_id();
    let rect = Rect::new(1, 2, 30, 40);
    let mut container = ContainerWindow::new_with_id(id, rect);
    assert_eq!(container.id(), id);
    assert_eq!(container.bounds(), rect);
    container.base_mut().clear_needs_repaint();
    assert!(!container.needs_repaint());
    container.invalidate();
    assert!(container.needs_repaint());
}

fn test_frame_window_keeps_custom_focus_path() {
    // FrameWindow must NOT delegate set_focus / has_focus to WindowBase.
    // Instead, it stores focus in its own `active` field (which drives
    // blue/grey title-bar coloring).
    let id = fresh_id();
    let mut frame = FrameWindow::new(id, "Test");

    // Initially inactive.
    assert!(!frame.has_focus(), "FrameWindow starts inactive");

    // Focus via the trait.
    frame.set_focus(true);
    assert!(
        frame.has_focus(),
        "FrameWindow::set_focus(true) should make has_focus() true"
    );

    // Critically: the underlying `WindowBase::has_focus` must NOT have
    // been touched by the override — its slot stays false. This pins
    // that the override does not delegate to the default `set_focus`,
    // which would have flipped that slot.
    assert!(
        !frame.base().has_focus(),
        "FrameWindow.set_focus must NOT touch WindowBase::has_focus"
    );

    frame.set_focus(false);
    assert!(!frame.has_focus());
    assert!(!frame.base().has_focus());
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_default_methods_route_through_base,
        &test_can_focus_override_beats_default,
        &test_invalidate_default_sets_needs_repaint_on_base,
        &test_set_bounds_default_invalidates,
        &test_set_bounds_no_invalidate_default_does_not_invalidate,
        &test_visible_default_round_trip,
        &test_parent_and_children_default_round_trip,
        &test_container_window_id_and_bounds_via_default,
        &test_frame_window_keeps_custom_focus_path,
    ]
}
