//! Tests for the U10 `Splitter` two-pane container.
//!
//! Layout math is exercised directly through the splitter's getters, so
//! the pure cases are independent of the `WindowManager`. The full
//! `set_bounds` → relayout → child `set_bounds` integration runs through
//! the live manager (already initialized by kernel boot before the test
//! runner fires) — same approach used by `layout_tests`.

extern crate alloc;

use alloc::boxed::Box;

use crate::lib::test_utils::Testable;
use crate::window::event::{Event, KeyModifiers, MouseButtons, MouseEvent, MouseEventType};
use crate::window::types::Point;
use crate::window::windows::layout::Spacer;
use crate::window::windows::splitter::{Splitter, SplitterOrientation, DIVIDER_WIDTH};
use crate::window::{with_window_manager, Rect, WindowId};

// ---------------------------------------------------------------------------
// Synthetic event helpers
// ---------------------------------------------------------------------------

fn left_event(event_type: MouseEventType, local: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type,
        position: local,
        global_position: local,
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
    })
}

fn release_event(local: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::ButtonUp,
        position: local,
        global_position: local,
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    })
}

// ---------------------------------------------------------------------------
// Pure construction tests (no manager interaction needed)
// ---------------------------------------------------------------------------

/// Default vertical splitter splits its container 50/50 (modulo the
/// divider strip).
fn test_vertical_splitter_default_50_50() {
    let splitter = Splitter::new_vertical(Rect::new(0, 0, 400, 200));
    assert_eq!(splitter.orientation(), SplitterOrientation::Vertical);
    // 400 / 2 = 200.
    assert_eq!(splitter.divider_position(), 200);
}

/// Default horizontal splitter splits top/bottom at 50%.
fn test_horizontal_splitter_default_50_50() {
    let splitter = Splitter::new_horizontal(Rect::new(0, 0, 100, 300));
    assert_eq!(splitter.orientation(), SplitterOrientation::Horizontal);
    assert_eq!(splitter.divider_position(), 150);
}

/// Setting an explicit divider position retains it (no minimums set).
fn test_set_divider_position_no_minimums() {
    let mut splitter = Splitter::new_vertical(Rect::new(0, 0, 400, 200));
    splitter.set_divider_position(120);
    assert_eq!(splitter.divider_position(), 120);
}

/// `set_divider_position` clamps to the first pane's minimum when too
/// small, and to `total - second_min - DIVIDER_WIDTH` when too large.
/// This is pure math on a standalone Splitter — no manager interaction
/// needed since the slot ids never need to resolve to real children.
fn test_set_divider_position_clamps_to_minimums() {
    let mut s = Splitter::new_vertical(Rect::new(0, 0, 600, 100));
    s.set_first(WindowId::new(), 200);
    s.set_second(WindowId::new(), 200);

    // Below first_min — clamps up.
    s.set_divider_position(50);
    assert_eq!(s.divider_position(), 200);

    // Past the upper bound — clamps down to total - second_min - strip.
    s.set_divider_position(1000);
    assert_eq!(s.divider_position(), 600 - 200 - DIVIDER_WIDTH);
}

/// Container too small to honor both minimums — divider centers at the
/// first pane's minimum, both panes overflow at minimum size.
fn test_too_small_container_centers_at_first_min() {
    let mut splitter = Splitter::new_vertical(Rect::new(0, 0, 100, 50));
    // Add a Splitter without children but with minimums; the clamp
    // routine runs only when set_first / set_second / set_bounds /
    // set_divider_position fires — exercise the path via
    // set_divider_position.
    splitter.set_first(WindowId::new(), 200);
    splitter.set_second(WindowId::new(), 200);
    // Total 100 < 200+200+4=404, so the clamp lands at first_min.
    assert_eq!(splitter.divider_position(), 200);
}

// ---------------------------------------------------------------------------
// WindowManager integration tests
// ---------------------------------------------------------------------------

/// Vertical splitter with two equal-min panes splits 50/50 by default
/// and writes the correct child bounds through the registry.
fn test_vertical_splitter_writes_child_bounds() {
    with_window_manager(|wm| {
        let a = wm.create_window(None);
        let b = wm.create_window(None);
        let splitter_id = wm.create_window(None);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            SplitterOrientation::Vertical,
            Rect::new(0, 0, 400, 200),
        );
        splitter.set_first(a, 0);
        splitter.set_second(b, 0);

        wm.set_window_impl(a, Box::new(Spacer::new_with_id(a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(b, Box::new(Spacer::new_with_id(b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(splitter_id, Box::new(splitter));

        // Trigger relayout via set_bounds to the same dimensions —
        // ensures children are written through the active manager flow.
        wm.with_window_mut(splitter_id, |w| {
            w.set_bounds(Rect::new(0, 0, 400, 200));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        let bounds_b = wm.window_registry.get(&b).unwrap().bounds();
        // Divider at 200, strip width 4. First pane: x=0..200; second
        // pane: x=204..400 (width 196).
        assert_eq!(bounds_a, Rect::new(0, 0, 200, 200));
        assert_eq!(bounds_b, Rect::new(204, 0, 400 - 200 - DIVIDER_WIDTH, 200));

        wm.destroy_window(splitter_id);
    });
}

/// Horizontal splitter splits top/bottom and writes correct bounds.
fn test_horizontal_splitter_writes_child_bounds() {
    with_window_manager(|wm| {
        let a = wm.create_window(None);
        let b = wm.create_window(None);
        let splitter_id = wm.create_window(None);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            SplitterOrientation::Horizontal,
            Rect::new(10, 20, 100, 300),
        );
        splitter.set_first(a, 0);
        splitter.set_second(b, 0);

        wm.set_window_impl(a, Box::new(Spacer::new_with_id(a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(b, Box::new(Spacer::new_with_id(b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(splitter_id, Box::new(splitter));

        wm.with_window_mut(splitter_id, |w| {
            w.set_bounds(Rect::new(10, 20, 100, 300));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        let bounds_b = wm.window_registry.get(&b).unwrap().bounds();
        // Divider at 150 (300/2). First: y=20..170, second: y=174..320.
        assert_eq!(bounds_a, Rect::new(10, 20, 100, 150));
        assert_eq!(
            bounds_b,
            Rect::new(
                10,
                20 + 150 + DIVIDER_WIDTH as i32,
                100,
                300 - 150 - DIVIDER_WIDTH
            )
        );

        wm.destroy_window(splitter_id);
    });
}

/// Dragging the divider right grows the first pane and shrinks the
/// second.
fn test_drag_grows_first_shrinks_second() {
    with_window_manager(|wm| {
        let a = wm.create_window(None);
        let b = wm.create_window(None);
        let splitter_id = wm.create_window(None);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            SplitterOrientation::Vertical,
            Rect::new(0, 0, 400, 200),
        );
        splitter.set_first(a, 0);
        splitter.set_second(b, 0);

        wm.set_window_impl(a, Box::new(Spacer::new_with_id(a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(b, Box::new(Spacer::new_with_id(b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(splitter_id, Box::new(splitter));

        // ButtonDown on the divider strip (local x = 200, anywhere on y).
        wm.with_window_mut(splitter_id, |w| {
            let _ = w.handle_event(left_event(MouseEventType::ButtonDown, Point::new(200, 50)));
            // Move the divider to x = 280.
            let _ = w.handle_event(left_event(MouseEventType::Move, Point::new(280, 50)));
            let _ = w.handle_event(release_event(Point::new(280, 50)));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        let bounds_b = wm.window_registry.get(&b).unwrap().bounds();
        assert_eq!(bounds_a.width, 280);
        assert_eq!(bounds_b.x, 280 + DIVIDER_WIDTH as i32);
        assert_eq!(bounds_b.width, 400 - 280 - DIVIDER_WIDTH);

        wm.destroy_window(splitter_id);
    });
}

/// AE5 coverage. Both panes have a 200-pixel minimum; dragging far past
/// the minimum stops at the minimum and does not occlude either pane.
fn test_drag_clamped_by_minimums() {
    with_window_manager(|wm| {
        let a = wm.create_window(None);
        let b = wm.create_window(None);
        let splitter_id = wm.create_window(None);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            SplitterOrientation::Vertical,
            Rect::new(0, 0, 600, 100),
        );
        splitter.set_first(a, 200);
        splitter.set_second(b, 200);

        wm.set_window_impl(a, Box::new(Spacer::new_with_id(a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(b, Box::new(Spacer::new_with_id(b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(splitter_id, Box::new(splitter));

        // Drag toward the right edge — should stop at
        // 600 - 200 - DIVIDER_WIDTH = 396.
        wm.with_window_mut(splitter_id, |w| {
            let _ = w.handle_event(left_event(MouseEventType::ButtonDown, Point::new(300, 50)));
            let _ = w.handle_event(left_event(MouseEventType::Move, Point::new(700, 50)));
            let _ = w.handle_event(release_event(Point::new(700, 50)));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        let bounds_b = wm.window_registry.get(&b).unwrap().bounds();
        // First pane must not exceed 600 - 200 - 4 = 396.
        assert_eq!(bounds_a.width, 396);
        // Second pane must remain at 200 (its minimum).
        assert_eq!(bounds_b.width, 200);
        // Total layout still adds up: 396 + 4 + 200 = 600.
        assert_eq!(bounds_a.width + DIVIDER_WIDTH + bounds_b.width, 600);

        // Drag toward the left edge — should stop at first_min = 200.
        wm.with_window_mut(splitter_id, |w| {
            let _ = w.handle_event(left_event(
                MouseEventType::ButtonDown,
                // Currently the divider sits at 396, so click on it.
                Point::new(396, 50),
            ));
            let _ = w.handle_event(left_event(MouseEventType::Move, Point::new(0, 50)));
            let _ = w.handle_event(release_event(Point::new(0, 50)));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        let bounds_b = wm.window_registry.get(&b).unwrap().bounds();
        // First pane must remain at first_min = 200.
        assert_eq!(bounds_a.width, 200);
        // Second pane gets the rest minus divider: 600 - 200 - 4 = 396.
        assert_eq!(bounds_b.width, 396);

        wm.destroy_window(splitter_id);
    });
}

/// Resizing the splitter container preserves the divider's relative
/// ratio (within a pixel of integer rounding).
fn test_resize_preserves_divider_ratio() {
    with_window_manager(|wm| {
        let a = wm.create_window(None);
        let b = wm.create_window(None);
        let splitter_id = wm.create_window(None);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            SplitterOrientation::Vertical,
            Rect::new(0, 0, 400, 200),
        );
        splitter.set_first(a, 0);
        splitter.set_second(b, 0);
        // Move the divider to 25% of the way across.
        splitter.set_divider_position(100);

        wm.set_window_impl(a, Box::new(Spacer::new_with_id(a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(b, Box::new(Spacer::new_with_id(b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(splitter_id, Box::new(splitter));

        // Resize to 800 wide. 25% of 800 = 200.
        wm.with_window_mut(splitter_id, |w| {
            w.set_bounds(Rect::new(0, 0, 800, 200));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        // Allow ±1 pixel for rounding (here exact: 100 * 800 / 400 = 200).
        let diff = (bounds_a.width as i64 - 200i64).abs();
        assert!(diff <= 1, "expected ~200, got {}", bounds_a.width);

        wm.destroy_window(splitter_id);
    });
}

/// `ButtonDown` outside the divider strip does NOT enter the dragging
/// state — a subsequent `Move` is therefore ignored.
fn test_buttondown_outside_divider_does_not_drag() {
    with_window_manager(|wm| {
        let a = wm.create_window(None);
        let b = wm.create_window(None);
        let splitter_id = wm.create_window(None);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            SplitterOrientation::Vertical,
            Rect::new(0, 0, 400, 200),
        );
        splitter.set_first(a, 0);
        splitter.set_second(b, 0);

        wm.set_window_impl(a, Box::new(Spacer::new_with_id(a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(b, Box::new(Spacer::new_with_id(b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(splitter_id, Box::new(splitter));

        // Trigger an initial relayout now that both panes are in the
        // registry — `set_first` / `set_second` ran before the children
        // were registered, so their bounds in the registry are still the
        // 0×0 placeholder set above.
        wm.with_window_mut(splitter_id, |w| {
            w.set_bounds(Rect::new(0, 0, 400, 200));
        });

        // Click well left of the divider (which sits at x=200..204).
        wm.with_window_mut(splitter_id, |w| {
            let _ = w.handle_event(left_event(MouseEventType::ButtonDown, Point::new(50, 50)));
            // A Move now should be ignored (no drag in progress).
            let _ = w.handle_event(left_event(MouseEventType::Move, Point::new(80, 50)));
            let _ = w.handle_event(release_event(Point::new(80, 50)));
        });

        let bounds_a = wm.window_registry.get(&a).unwrap().bounds();
        // Divider is unchanged: first pane width still 200.
        assert_eq!(bounds_a.width, 200);

        wm.destroy_window(splitter_id);
    });
}

// ---------------------------------------------------------------------------
// Test registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_vertical_splitter_default_50_50,
        &test_horizontal_splitter_default_50_50,
        &test_set_divider_position_no_minimums,
        &test_set_divider_position_clamps_to_minimums,
        &test_too_small_container_centers_at_first_min,
        &test_vertical_splitter_writes_child_bounds,
        &test_horizontal_splitter_writes_child_bounds,
        &test_drag_grows_first_shrinks_second,
        &test_drag_clamped_by_minimums,
        &test_resize_preserves_divider_ratio,
        &test_buttondown_outside_divider_does_not_drag,
    ]
}
