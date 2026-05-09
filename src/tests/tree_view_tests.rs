//! Tests for the U9 `TreeView` widget — expand/collapse semantics,
//! disclosure-triangle vs. label hit-testing, keyboard navigation
//! (Up/Down/Left/Right/Enter), and ScrollView integration.
//!
//! Behaviors are exercised via synthetic mouse / keyboard events
//! delivered straight to the widget's `handle_event`. Where a
//! `ScrollView` is in play, the test mimics ScrollView's
//! coordinate translation by writing the child's bounds the same
//! way the wrapper does at paint time.

extern crate alloc;

use alloc::string::String;

use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::selection::Selection;
use crate::window::types::Point;
use crate::window::windows::scroll_view::ScrollView;
use crate::window::windows::tree_view::{NodeId, TreeView, INDENT_PX};
use crate::window::{Rect, Window};

// ---------------------------------------------------------------------------
// Synthetic event helpers
// ---------------------------------------------------------------------------

fn click_at(local: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: local,
        global_position: local,
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
    })
}

fn key(key_code: KeyCode) -> Event {
    Event::Keyboard(KeyboardEvent {
        key_code,
        pressed: true,
        modifiers: KeyModifiers::default(),
    })
}

// ---------------------------------------------------------------------------
// Construction helpers
// ---------------------------------------------------------------------------

/// Build a tiny tree:
///   root_a
///     ├── child_a1
///     └── child_a2
///   root_b (leaf)
fn build_simple_tree() -> (TreeView, NodeId, NodeId, NodeId, NodeId) {
    let mut tv = TreeView::new(Rect::new(0, 0, 200, 200));
    let a = tv.add_node(None, "root_a");
    let a1 = tv.add_node(Some(a), "child_a1");
    let a2 = tv.add_node(Some(a), "child_a2");
    let b = tv.add_node(None, "root_b");
    (tv, a, a1, a2, b)
}

// ---------------------------------------------------------------------------
// Happy path: AE4 — Right expands, Left collapses, Left on collapsed
// moves to parent.
// ---------------------------------------------------------------------------

fn test_keyboard_right_expands_then_left_collapses_then_left_moves_to_parent() {
    let (mut tv, a, _a1, _a2, _b) = build_simple_tree();
    // Default state: roots are not expanded → visible rows = [a, b]
    assert_eq!(tv.visible_row_count(), 2);
    // Select root_a (row 0).
    tv.set_selected_row(Some(0));
    assert_eq!(tv.selected_node(), Some(a));

    // Right on a collapsed non-leaf expands it.
    let _ = tv.handle_event(key(KeyCode::Right));
    assert!(tv.is_expanded(a));
    // Visible rows: [a, a1, a2, b] = 4 rows. Selection still on row 0.
    assert_eq!(tv.visible_row_count(), 4);
    assert_eq!(tv.selection(), &Selection::Single(0));

    // Right again on an expanded node → move to first child.
    let _ = tv.handle_event(key(KeyCode::Right));
    assert_eq!(tv.selection(), &Selection::Single(1));

    // Left on a leaf (child_a1) → move to parent.
    let _ = tv.handle_event(key(KeyCode::Left));
    assert_eq!(tv.selection(), &Selection::Single(0));
    assert_eq!(tv.selected_node(), Some(a));

    // Left on an expanded node → collapse.
    let _ = tv.handle_event(key(KeyCode::Left));
    assert!(!tv.is_expanded(a));
    assert_eq!(tv.visible_row_count(), 2);
    // Selection stays on root_a.
    assert_eq!(tv.selected_node(), Some(a));

    // Left on a collapsed root with no parent → no-op.
    let _ = tv.handle_event(key(KeyCode::Left));
    assert_eq!(tv.selected_node(), Some(a));
}

// ---------------------------------------------------------------------------
// Happy path: clicking a disclosure triangle toggles expand/collapse
// without changing selection.
// ---------------------------------------------------------------------------

fn test_click_disclosure_triangle_toggles_without_changing_selection() {
    let (mut tv, a, _a1, _a2, _b) = build_simple_tree();
    // Selection starts as None.
    assert_eq!(tv.selection(), &Selection::None);

    // Click on root_a's disclosure triangle (row 0, x=8 → inside the
    // 16px disclosure cell at depth 0). Default row_height = 16, so
    // y = 8 lands in row 0.
    let _ = tv.handle_event(click_at(Point::new(8, 8)));

    // Triangle toggled root_a to expanded; selection unchanged.
    assert!(tv.is_expanded(a));
    assert_eq!(tv.visible_row_count(), 4);
    assert_eq!(tv.selection(), &Selection::None);

    // Click triangle again → collapses, still no selection change.
    let _ = tv.handle_event(click_at(Point::new(8, 8)));
    assert!(!tv.is_expanded(a));
    assert_eq!(tv.visible_row_count(), 2);
    assert_eq!(tv.selection(), &Selection::None);
}

// ---------------------------------------------------------------------------
// Happy path: clicking a row label selects the row.
// ---------------------------------------------------------------------------

fn test_click_label_selects_row() {
    let (mut tv, a, _a1, _a2, _b) = build_simple_tree();
    // Expand root_a so we have a depth-1 child to click.
    tv.expand(a);

    // Click the label of child_a1 (row 1). Depth = 1, so triangle
    // hit-zone is x=[16, 32). Click at x=40 lands in label area.
    // y = 16 + 8 = 24 → row 1.
    let _ = tv.handle_event(click_at(Point::new(40, 24)));
    assert_eq!(tv.selection(), &Selection::Single(1));
}

// ---------------------------------------------------------------------------
// Edge case: tree with a single leaf node — Right and Left are no-ops.
// ---------------------------------------------------------------------------

fn test_single_leaf_tree_arrow_left_right_noop() {
    let mut tv = TreeView::new(Rect::new(0, 0, 100, 50));
    let only = tv.add_node(None, "only");
    tv.set_selected_row(Some(0));
    assert_eq!(tv.selected_node(), Some(only));

    let _ = tv.handle_event(key(KeyCode::Right));
    assert!(!tv.is_expanded(only));
    assert_eq!(tv.selected_node(), Some(only));

    let _ = tv.handle_event(key(KeyCode::Left));
    assert_eq!(tv.selected_node(), Some(only));
    assert_eq!(tv.visible_row_count(), 1);
}

// ---------------------------------------------------------------------------
// Edge case: collapsing a node with selection inside it moves selection
// to the collapsing parent.
// ---------------------------------------------------------------------------

fn test_collapse_with_selection_inside_moves_to_parent() {
    let (mut tv, a, _a1, _a2, _b) = build_simple_tree();
    tv.expand(a);
    // Select child_a2 (row 2).
    tv.set_selected_row(Some(2));
    let selected = tv.selected_node();
    assert!(selected.is_some());

    tv.collapse(a);

    // visible_rows is now [a, b]; the previously-selected child is
    // gone. Selection should land on root_a (row 0).
    assert_eq!(tv.visible_row_count(), 2);
    assert_eq!(tv.selected_node(), Some(a));
}

// ---------------------------------------------------------------------------
// Edge case: deeply nested tree (5 levels) — visible_rows correct after
// expanding all; correct after collapsing root.
// ---------------------------------------------------------------------------

fn test_deeply_nested_tree_expand_and_collapse() {
    let mut tv = TreeView::new(Rect::new(0, 0, 400, 200));
    let lvl0 = tv.add_node(None, "L0");
    let lvl1 = tv.add_node(Some(lvl0), "L1");
    let lvl2 = tv.add_node(Some(lvl1), "L2");
    let lvl3 = tv.add_node(Some(lvl2), "L3");
    let lvl4 = tv.add_node(Some(lvl3), "L4");

    // Initially only the root is visible.
    assert_eq!(tv.visible_row_count(), 1);

    // Expand each level.
    tv.expand(lvl0);
    tv.expand(lvl1);
    tv.expand(lvl2);
    tv.expand(lvl3);
    // L4 is a leaf — nothing more to expand.
    assert!(!tv.is_expanded(lvl4));

    // All five levels visible in order.
    assert_eq!(tv.visible_row_count(), 5);
    assert_eq!(tv.node_at_row(0), Some(lvl0));
    assert_eq!(tv.node_at_row(1), Some(lvl1));
    assert_eq!(tv.node_at_row(2), Some(lvl2));
    assert_eq!(tv.node_at_row(3), Some(lvl3));
    assert_eq!(tv.node_at_row(4), Some(lvl4));

    // Collapsing the root hides everything below — only L0 visible.
    tv.collapse(lvl0);
    assert_eq!(tv.visible_row_count(), 1);
    assert_eq!(tv.node_at_row(0), Some(lvl0));
}

// ---------------------------------------------------------------------------
// Integration: TreeView with 10 nodes wrapped in a small ScrollView —
// scroll-wheel scrolls; clicking a visible (post-scroll) row selects
// the right NodeId.
// ---------------------------------------------------------------------------

fn test_tree_view_in_scroll_view_click_after_scroll() {
    let mut tv = TreeView::new(Rect::new(0, 0, 200, 32));
    // 10 sibling roots at depth 0.
    let mut ids: alloc::vec::Vec<NodeId> = alloc::vec::Vec::new();
    for i in 0..10 {
        let mut s = String::new();
        s.push('n');
        // Single-digit suffix is fine; 10 fits in two digits, but
        // keep ASCII-only labels.
        if i >= 10 {
            s.push((b'0' + (i / 10) as u8) as char);
        }
        s.push((b'0' + (i % 10) as u8) as char);
        ids.push(tv.add_node(None, &s));
    }
    assert_eq!(tv.visible_row_count(), 10);

    // ScrollView viewport is 32 px tall (2 rows of 16 px); content
    // height = 10 * 16 = 160.
    let mut sv = ScrollView::new(Rect::new(0, 0, 200, 32));
    sv.set_content_size(200, tv.content_height());
    assert_eq!(tv.content_height(), 160);

    // Scroll halfway down.
    sv.scroll_to(0, 80);
    assert_eq!(sv.scroll_y(), 80);

    // Mimic ScrollView's render-time bounds translation. The
    // child's bounds.y becomes -scroll_y so that paint draws the
    // full content rect at translated coordinates. MouseEvent
    // positions arrive in the same frame as `bounds` (per the
    // docs in ScrollView), so a viewport click at y=8 stays at
    // position.y=8; the y_to_row helper subtracts bounds.y to
    // recover the row.
    let scroll_y = sv.scroll_y();
    tv.set_bounds(Rect::new(0, -scroll_y, 200, tv.content_height()));

    // Click at viewport y=8 (position.y stays in bounds frame).
    // y_to_row: relative_y = 8 - (-80) = 88 → row 5.
    // x=40 lands in the label area (depth 0 → triangle at x in
    // [0,16); label at x >= 18 once text padding kicks in).
    let _ = tv.handle_event(click_at(Point::new(40, 8)));
    assert_eq!(tv.selection(), &Selection::Single(5));
    assert_eq!(tv.selected_node(), Some(ids[5]));
}

// ---------------------------------------------------------------------------
// Sanity check on hit-zone constants — protects against accidental
// drift in INDENT_PX away from the documented 16.
// ---------------------------------------------------------------------------

fn test_indent_constant_pinned_at_16() {
    assert_eq!(INDENT_PX, 16);
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_keyboard_right_expands_then_left_collapses_then_left_moves_to_parent,
        &test_click_disclosure_triangle_toggles_without_changing_selection,
        &test_click_label_selects_row,
        &test_single_leaf_tree_arrow_left_right_noop,
        &test_collapse_with_selection_inside_moves_to_parent,
        &test_deeply_nested_tree_expand_and_collapse,
        &test_tree_view_in_scroll_view_click_after_scroll,
        &test_indent_constant_pinned_at_16,
    ]
}
