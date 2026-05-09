//! Tests for the U13 `IconView` widget.
//!
//! Behavior is exercised through synthetic mouse / keyboard events
//! delivered straight to the widget's `handle_event`, mirroring the
//! pattern established by `list_migration_tests.rs`. We don't go through
//! the WindowManager — these tests live at the widget level.

extern crate alloc;

use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::selection::{Selection, SelectionMode};
use crate::window::types::Point;
use crate::window::windows::icon_view::IconView;
use crate::window::{Rect, Window};

// ---------------------------------------------------------------------------
// Synthetic-event helpers
// ---------------------------------------------------------------------------

fn click_at(local: Point, shift: bool, ctrl: bool) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: local,
        global_position: local,
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers {
            shift,
            ctrl,
            alt: false,
            meta: false,
        },
    })
}

fn key_event(key_code: KeyCode, shift: bool) -> Event {
    Event::Keyboard(KeyboardEvent {
        key_code,
        pressed: true,
        modifiers: KeyModifiers {
            shift,
            ctrl: false,
            alt: false,
            meta: false,
        },
    })
}

/// Build an IconView with `count` tiles of `tile_w × tile_h`, with the
/// supplied outer bounds. Labels are short ASCII strings so they fit any
/// reasonable tile width.
fn make_icon_view(count: usize, bounds: Rect, tile_w: u32, tile_h: u32) -> IconView {
    let mut iv = IconView::new(bounds);
    iv.set_tile_size(tile_w, tile_h);
    for i in 0..count {
        // Label like "t0", "t1", ... up to "t99".
        let mut label = alloc::string::String::new();
        label.push('t');
        if i < 10 {
            label.push((b'0' + i as u8) as char);
        } else {
            label.push((b'0' + (i / 10) as u8) as char);
            label.push((b'0' + (i % 10) as u8) as char);
        }
        iv.add_tile(&label, None);
    }
    iv
}

// ---------------------------------------------------------------------------
// Layout & rendering happy paths
// ---------------------------------------------------------------------------

/// 10 tiles in a 5-tile-wide viewport produce 2 rows.
fn test_layout_10_tiles_5_per_row_2_rows() {
    let iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    assert_eq!(iv.tiles_per_row(), 5);
    // ceil(10 / 5) = 2 rows × 20 px = 40 px content height.
    assert_eq!(iv.content_height(), 40);
}

// ---------------------------------------------------------------------------
// Mouse selection
// ---------------------------------------------------------------------------

/// Clicking at (row=1, col=2) selects index 7 (with tiles_per_row=5).
fn test_click_at_row_1_col_2_selects_index_7() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    // x = col*20 + 5 = 45; y = row*20 + 5 = 25.
    let _ = iv.handle_event(click_at(Point::new(45, 25), false, false));
    assert_eq!(iv.selection(), &Selection::Single(7));
}

/// Shift-click in Multi mode extends the selection to a range.
fn test_shift_click_extends_selection_in_multi_mode() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    iv.set_selection_mode(SelectionMode::Multi);

    // Plain click at idx 2.
    let _ = iv.handle_event(click_at(Point::new(2 * 20 + 5, 5), false, false));
    assert_eq!(iv.selection(), &Selection::Single(2));

    // Shift-click at idx 7 (row 1, col 2).
    let _ = iv.handle_event(click_at(Point::new(2 * 20 + 5, 25), true, false));
    assert_eq!(iv.selection(), &Selection::Range { anchor: 2, end: 7 });
}

// ---------------------------------------------------------------------------
// Arrow keys — happy paths
// ---------------------------------------------------------------------------

/// Arrow-right at the end of a non-last row wraps to the first tile of
/// the next row.
fn test_arrow_right_end_of_row_wraps_to_next_row() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    // Place selection at idx 4 (row 0, col 4 — last column of first row).
    let _ = iv.handle_event(click_at(Point::new(4 * 20 + 5, 5), false, false));
    assert_eq!(iv.selection(), &Selection::Single(4));

    let _ = iv.handle_event(key_event(KeyCode::Right, false));
    assert_eq!(iv.selection(), &Selection::Single(5));
}

/// Arrow-down moves the selection down by `tiles_per_row`, preserving
/// column position when possible.
fn test_arrow_down_moves_down_one_row_same_column() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    // Click at idx 2 (row 0, col 2).
    let _ = iv.handle_event(click_at(Point::new(2 * 20 + 5, 5), false, false));
    let _ = iv.handle_event(key_event(KeyCode::Down, false));
    assert_eq!(iv.selection(), &Selection::Single(7));
}

// ---------------------------------------------------------------------------
// Arrow keys — edge clamps
// ---------------------------------------------------------------------------

/// Arrow-right at the very last tile leaves the selection unchanged.
fn test_arrow_right_at_last_tile_clamps() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    // Click at idx 9 (last tile, row 1, col 4).
    let _ = iv.handle_event(click_at(Point::new(4 * 20 + 5, 25), false, false));
    assert_eq!(iv.selection(), &Selection::Single(9));

    let _ = iv.handle_event(key_event(KeyCode::Right, false));
    assert_eq!(iv.selection(), &Selection::Single(9));
}

/// Arrow-left at the very first tile (idx 0) leaves the selection
/// unchanged.
fn test_arrow_left_at_first_tile_clamps() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    let _ = iv.handle_event(click_at(Point::new(5, 5), false, false));
    assert_eq!(iv.selection(), &Selection::Single(0));

    let _ = iv.handle_event(key_event(KeyCode::Left, false));
    assert_eq!(iv.selection(), &Selection::Single(0));
}

/// Arrow-down at the last row clamps — even if the last row is shorter
/// than `tiles_per_row`. Here: 7 tiles, tpr=5 — last row has 2 tiles
/// (indices 5, 6). At idx 6 (last row, col 1), arrow-down stays at 6.
fn test_arrow_down_at_last_row_clamps_with_short_last_row() {
    let mut iv = make_icon_view(7, Rect::new(0, 0, 100, 40), 20, 20);
    // Click at idx 6 (row 1, col 1).
    let _ = iv.handle_event(click_at(Point::new(1 * 20 + 5, 25), false, false));
    assert_eq!(iv.selection(), &Selection::Single(6));

    let _ = iv.handle_event(key_event(KeyCode::Down, false));
    assert_eq!(iv.selection(), &Selection::Single(6));
}

/// Arrow-down from second-to-last row into a shorter last row clamps to
/// the last available index rather than walking off the end.
fn test_arrow_down_into_short_last_row_clamps_to_last_index() {
    let mut iv = make_icon_view(7, Rect::new(0, 0, 100, 40), 20, 20);
    // Click at idx 4 (row 0, col 4). target_col=4 doesn't exist in row 1
    // (row 1 has only cols 0,1). Should clamp to idx 6.
    let _ = iv.handle_event(click_at(Point::new(4 * 20 + 5, 5), false, false));
    assert_eq!(iv.selection(), &Selection::Single(4));

    let _ = iv.handle_event(key_event(KeyCode::Down, false));
    assert_eq!(iv.selection(), &Selection::Single(6));
}

/// Arrow-up at the first row (row 0) leaves the selection unchanged.
fn test_arrow_up_at_first_row_clamps() {
    let mut iv = make_icon_view(10, Rect::new(0, 0, 100, 40), 20, 20);
    // Click at idx 3 (row 0).
    let _ = iv.handle_event(click_at(Point::new(3 * 20 + 5, 5), false, false));
    let _ = iv.handle_event(key_event(KeyCode::Up, false));
    assert_eq!(iv.selection(), &Selection::Single(3));
}

// ---------------------------------------------------------------------------
// Viewport / data edge cases
// ---------------------------------------------------------------------------

/// Viewport narrower than one tile clamps `tiles_per_row` to 1 — the grid
/// degenerates into a single vertical column.
fn test_viewport_narrower_than_tile_single_column() {
    // tile_w = 30, viewport width = 20 → raw division gives 0; should
    // clamp to 1.
    let iv = make_icon_view(4, Rect::new(0, 0, 20, 80), 30, 20);
    assert_eq!(iv.tiles_per_row(), 1);
    // 4 tiles in 1-per-row → 4 rows × 20 = 80 px.
    assert_eq!(iv.content_height(), 80);
}

/// Zero tiles paints background (no panic) and reports zero content
/// height.
fn test_zero_tiles_does_not_panic() {
    let iv = make_icon_view(0, Rect::new(0, 0, 100, 40), 20, 20);
    assert!(iv.is_empty());
    assert_eq!(iv.len(), 0);
    assert_eq!(iv.content_height(), 0);
    // We can't easily call paint() without a GraphicsDevice in unit tests;
    // the important thing is that the widget exposes a sensible state for
    // empty input. If callers wrap us in a ScrollView, the wrapper feeds
    // `content_height() == 0` straight through.
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_layout_10_tiles_5_per_row_2_rows,
        &test_click_at_row_1_col_2_selects_index_7,
        &test_shift_click_extends_selection_in_multi_mode,
        &test_arrow_right_end_of_row_wraps_to_next_row,
        &test_arrow_down_moves_down_one_row_same_column,
        &test_arrow_right_at_last_tile_clamps,
        &test_arrow_left_at_first_tile_clamps,
        &test_arrow_down_at_last_row_clamps_with_short_last_row,
        &test_arrow_down_into_short_last_row_clamps_to_last_index,
        &test_arrow_up_at_first_row_clamps,
        &test_viewport_narrower_than_tile_single_column,
        &test_zero_tiles_does_not_panic,
    ]
}
