//! Tests for the layout primitives (`VBox`, `HBox`, `Padding`,
//! `Spacer`).
//!
//! The distribution math is exercised directly through
//! `compute_child_bounds` (a pure function over container bounds + size
//! hints), which keeps these tests independent of the global
//! `WindowManager`. The full set_bounds → relayout → child set_bounds
//! integration runs through the live manager — which is already
//! initialized by kernel boot before the test runner fires.

use alloc::boxed::Box;

use crate::lib::test_utils::Testable;
use crate::window::windows::layout::{HBox, Padding, SizeHint, Spacer, VBox};
use crate::window::{with_window_manager, Rect, Window, WindowId};

// -- VBox / HBox pure-math tests ------------------------------------------

fn test_vbox_three_fill_equal_weights_in_300() {
    let mut vbox = VBox::new(Rect::new(0, 0, 100, 300));
    vbox.add_child(WindowId::new(), SizeHint::Fill(1));
    vbox.add_child(WindowId::new(), SizeHint::Fill(1));
    vbox.add_child(WindowId::new(), SizeHint::Fill(1));

    let layouts = vbox.compute_child_bounds();
    assert_eq!(layouts.len(), 3);
    assert_eq!(layouts[0], Rect::new(0, 0, 100, 100));
    assert_eq!(layouts[1], Rect::new(0, 100, 100, 100));
    assert_eq!(layouts[2], Rect::new(0, 200, 100, 100));
}

fn test_hbox_fixed_then_fill_in_200() {
    let mut hbox = HBox::new(Rect::new(0, 0, 200, 50));
    hbox.add_child(WindowId::new(), SizeHint::Fixed(50));
    hbox.add_child(WindowId::new(), SizeHint::Fill(1));

    let layouts = hbox.compute_child_bounds();
    assert_eq!(layouts.len(), 2);
    assert_eq!(layouts[0], Rect::new(0, 0, 50, 50));
    assert_eq!(layouts[1], Rect::new(50, 0, 150, 50));
}

fn test_vbox_zero_children_does_not_panic() {
    let vbox = VBox::new(Rect::new(0, 0, 200, 200));
    let layouts = vbox.compute_child_bounds();
    assert!(layouts.is_empty());
}

fn test_hbox_all_fixed_sum_exceeds_width_clips_last() {
    let mut hbox = HBox::new(Rect::new(0, 0, 200, 30));
    hbox.add_child(WindowId::new(), SizeHint::Fixed(150));
    hbox.add_child(WindowId::new(), SizeHint::Fixed(100));

    let layouts = hbox.compute_child_bounds();
    assert_eq!(layouts.len(), 2);
    assert_eq!(layouts[0], Rect::new(0, 0, 150, 30));
    // Only 50 px remain; the last child is clipped to that width.
    assert_eq!(layouts[1], Rect::new(150, 0, 50, 30));
}

fn test_vbox_mixed_fill_weights_2_and_1_in_30() {
    let mut vbox = VBox::new(Rect::new(0, 0, 10, 30));
    vbox.add_child(WindowId::new(), SizeHint::Fill(2));
    vbox.add_child(WindowId::new(), SizeHint::Fill(1));

    let layouts = vbox.compute_child_bounds();
    assert_eq!(layouts.len(), 2);
    assert_eq!(layouts[0], Rect::new(0, 0, 10, 20));
    assert_eq!(layouts[1], Rect::new(0, 20, 10, 10));
}

fn test_vbox_with_spacer_between_children() {
    // Layout: 10-px child, 5-px spacer gap, then a child fills the rest.
    let mut vbox = VBox::new(Rect::new(0, 0, 40, 100));
    vbox.add_child(WindowId::new(), SizeHint::Fixed(10));
    vbox.add_child(WindowId::new(), SizeHint::Fixed(5));
    vbox.add_child(WindowId::new(), SizeHint::Fill(1));

    let layouts = vbox.compute_child_bounds();
    assert_eq!(layouts[0], Rect::new(0, 0, 40, 10));
    // Spacer slot — empty in paint output (Spacer does not draw).
    assert_eq!(layouts[1], Rect::new(0, 10, 40, 5));
    assert_eq!(layouts[2], Rect::new(0, 15, 40, 85));
}

// -- Padding pure-math tests ----------------------------------------------

fn test_padding_uniform_10_in_100x100() {
    let mut padding = Padding::new(Rect::new(0, 0, 100, 100), 10, 10, 10, 10);
    padding.set_child(WindowId::new());

    assert_eq!(padding.child_bounds(), Rect::new(10, 10, 80, 80));
}

fn test_padding_insets_larger_than_container_zero_size() {
    // Insets total 400 wide vs. 100 wide container; the child rect must
    // collapse to zero on the saturated axis rather than wrap negative.
    let padding = Padding::new(Rect::new(0, 0, 100, 100), 200, 200, 200, 200);
    let child = padding.child_bounds();
    assert_eq!(child.width, 0);
    assert_eq!(child.height, 0);
}

fn test_padding_asymmetric_insets() {
    let padding = Padding::new(Rect::new(0, 0, 100, 100), 5, 10, 15, 20);
    assert_eq!(padding.child_bounds(), Rect::new(20, 5, 70, 80));
}

// -- Spacer is a window that paints nothing -------------------------------

fn test_spacer_construction() {
    let spacer = Spacer::new(Rect::new(5, 6, 7, 8));
    // Sanity: the spacer reports the bounds it was constructed with.
    assert_eq!(spacer.bounds(), Rect::new(5, 6, 7, 8));
}

// -- WindowManager integration: AE1 partial coverage ---------------------

/// Resizing the container (via `with_window_mut` so the layout sees the
/// active manager) must propagate to children's bounds without any
/// caller-side relayout call. Uses `Spacer`s as concrete child windows;
/// we read each child's bounds after the resize to confirm `set_bounds`
/// fired on each one.
fn test_vbox_resize_propagates_to_children() {
    with_window_manager(|wm| {
        // Three children, equal weights.
        let child_a = wm.create_window(None);
        let child_b = wm.create_window(None);
        let child_c = wm.create_window(None);

        // Build the VBox up front with all three children. This calls
        // `relayout` immediately, but the VBox is not yet in the
        // registry, so child set_bounds calls during construction are
        // no-ops (and the children are not yet registered either —
        // which is fine; we are about to register them right below).
        let vbox_id = wm.create_window(None);
        let mut vbox = VBox::new_with_id(vbox_id, Rect::new(0, 0, 100, 30));
        vbox.add_child(child_a, SizeHint::Fill(1));
        vbox.add_child(child_b, SizeHint::Fill(1));
        vbox.add_child(child_c, SizeHint::Fill(1));

        // Insert children into the registry.
        wm.set_window_impl(child_a, Box::new(Spacer::new_with_id(child_a, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(child_b, Box::new(Spacer::new_with_id(child_b, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(child_c, Box::new(Spacer::new_with_id(child_c, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(vbox_id, Box::new(vbox));

        // Trigger a real resize through the active-manager flow.
        wm.with_window_mut(vbox_id, |w| {
            w.set_bounds(Rect::new(0, 0, 100, 300));
        });

        // Each child should now be 100 px tall and stacked vertically.
        let bounds_a = wm.window_registry.get(&child_a).unwrap().bounds();
        let bounds_b = wm.window_registry.get(&child_b).unwrap().bounds();
        let bounds_c = wm.window_registry.get(&child_c).unwrap().bounds();
        assert_eq!(bounds_a, Rect::new(0, 0, 100, 100));
        assert_eq!(bounds_b, Rect::new(0, 100, 100, 100));
        assert_eq!(bounds_c, Rect::new(0, 200, 100, 100));

        // Clean up so we do not leak windows across tests.
        wm.destroy_window(vbox_id);
    });
}

/// Padding's resize must propagate to its single child.
fn test_padding_resize_propagates_to_child() {
    with_window_manager(|wm| {
        let child = wm.create_window(None);
        let padding_id = wm.create_window(None);

        let mut padding = Padding::new_with_id(
            padding_id,
            Rect::new(0, 0, 100, 100),
            10,
            10,
            10,
            10,
        );
        padding.set_child(child);

        wm.set_window_impl(child, Box::new(Spacer::new_with_id(child, Rect::new(0, 0, 0, 0))));
        wm.set_window_impl(padding_id, Box::new(padding));

        wm.with_window_mut(padding_id, |w| {
            w.set_bounds(Rect::new(0, 0, 200, 200));
        });

        let child_bounds = wm.window_registry.get(&child).unwrap().bounds();
        // 200 - left(10) - right(10) = 180, same for height.
        assert_eq!(child_bounds, Rect::new(10, 10, 180, 180));

        wm.destroy_window(padding_id);
    });
}

// -- registration ---------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_vbox_three_fill_equal_weights_in_300,
        &test_hbox_fixed_then_fill_in_200,
        &test_vbox_zero_children_does_not_panic,
        &test_hbox_all_fixed_sum_exceeds_width_clips_last,
        &test_vbox_mixed_fill_weights_2_and_1_in_30,
        &test_vbox_with_spacer_between_children,
        &test_padding_uniform_10_in_100x100,
        &test_padding_insets_larger_than_container_zero_size,
        &test_padding_asymmetric_insets,
        &test_spacer_construction,
        &test_vbox_resize_propagates_to_children,
        &test_padding_resize_propagates_to_child,
    ]
}
