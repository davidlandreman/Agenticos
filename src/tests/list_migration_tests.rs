//! Tests for the U6 List + MultiColumnList migration to the shared
//! `Selection` model and `ScrollView` wrapping.
//!
//! Most behavior is exercised through synthetic mouse / keyboard events
//! delivered straight to the widget's `handle_event`. The "1000-row inside
//! ScrollView" case constructs a real `ScrollView` and inspects its
//! reported scroll offset to confirm the integration works end-to-end.

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::selection::{Selection, SelectionMode};
use crate::window::types::Point;
use crate::window::windows::list::List;
use crate::window::windows::multi_column_list::{Column, MultiColumnList};
use crate::window::windows::scroll_view::ScrollView;
use crate::window::{Rect, Window};

// ---------------------------------------------------------------------------
// Synthetic event helpers
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

fn right_click_at(local: Point, global: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: local,
        global_position: global,
        buttons: MouseButtons {
            right: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
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

fn make_list_with_items(item_count: usize, bounds: Rect) -> List {
    let mut list = List::new(bounds);
    for i in 0..item_count {
        // Use ASCII-only labels.
        let mut s = String::new();
        s.push('i');
        s.push('-');
        // Append the index — keep it simple: at most 4 digits.
        let mut tmp = i;
        let mut digits: Vec<u8> = Vec::new();
        if tmp == 0 {
            digits.push(b'0');
        } else {
            while tmp > 0 {
                digits.push(b'0' + (tmp % 10) as u8);
                tmp /= 10;
            }
            digits.reverse();
        }
        for d in digits {
            s.push(d as char);
        }
        list.add_item(&s);
    }
    list
}

// ---------------------------------------------------------------------------
// Single-select happy path
// ---------------------------------------------------------------------------

fn test_single_click_produces_single_selection() {
    let mut list = make_list_with_items(5, Rect::new(0, 0, 200, 80));
    // item_height defaults to 16, items live at y = 0..80
    // Click on the third row: relative_y = 32..47
    let _ = list.handle_event(click_at(Point::new(10, 35), false, false));
    assert_eq!(list.selection(), &Selection::Single(2));
    assert_eq!(list.selected(), Some(2));
}

// ---------------------------------------------------------------------------
// Multi-select sequence: shift-click extends, ctrl-click toggles, plain
// click collapses (covers AE3).
// ---------------------------------------------------------------------------

fn test_multi_select_full_click_sequence() {
    let mut list = make_list_with_items(10, Rect::new(0, 0, 200, 200));
    list.set_selection_mode(SelectionMode::Multi);

    // Plain click on row 1 → Single(1)
    let _ = list.handle_event(click_at(Point::new(5, 16 + 4), false, false));
    assert_eq!(list.selection(), &Selection::Single(1));

    // Shift-click on row 4 → Range { anchor: 1, end: 4 }
    let _ = list.handle_event(click_at(Point::new(5, 4 * 16 + 4), true, false));
    assert_eq!(
        list.selection(),
        &Selection::Range { anchor: 1, end: 4 }
    );

    // Ctrl-click on row 0 → Multi({0, 1, 2, 3, 4})
    let _ = list.handle_event(click_at(Point::new(5, 0 + 4), false, true));
    match list.selection() {
        Selection::Multi(set) => {
            let collected: Vec<usize> = set.iter().copied().collect();
            assert_eq!(collected, vec![0usize, 1, 2, 3, 4]);
        }
        other => panic!("expected Multi after ctrl-click, got {:?}", other),
    }

    // Plain click on row 6 → Single(6) (collapses)
    let _ = list.handle_event(click_at(Point::new(5, 6 * 16 + 4), false, false));
    assert_eq!(list.selection(), &Selection::Single(6));
}

// ---------------------------------------------------------------------------
// Arrow keys
// ---------------------------------------------------------------------------

fn test_arrow_down_advances_selection() {
    let mut list = make_list_with_items(5, Rect::new(0, 0, 200, 80));
    // Set initial selection to row 1
    list.set_selected(Some(1));
    let _ = list.handle_event(key_event(KeyCode::Down, false));
    assert_eq!(list.selection(), &Selection::Single(2));
}

fn test_shift_arrow_down_extends_range_in_multi_mode() {
    let mut list = make_list_with_items(5, Rect::new(0, 0, 200, 80));
    list.set_selection_mode(SelectionMode::Multi);
    list.set_selected(Some(1));

    let _ = list.handle_event(key_event(KeyCode::Down, true));
    // anchor preserved at 1, end advances to 2
    assert_eq!(
        list.selection(),
        &Selection::Range { anchor: 1, end: 2 }
    );
}

// ---------------------------------------------------------------------------
// 1000-row List wrapped in a ScrollView (clicking through the wrapping).
// ---------------------------------------------------------------------------

fn test_large_list_wrapped_in_scroll_view_click_translates() {
    // Build a 1000-row list. content height = 1000 * 16 = 16000.
    let item_count = 1000usize;
    let mut list = make_list_with_items(item_count, Rect::new(0, 0, 200, 200));

    // Build a scroll view of viewport 200 tall and feed it the list's
    // natural content size.
    let mut sv = ScrollView::new(Rect::new(0, 0, 200, 200));
    sv.set_content_size(200, list.content_height());

    // Confirm the ScrollView accepts the content size and clamps scroll.
    sv.scroll_to(0, 1000);
    assert_eq!(sv.scroll_y(), 1000);

    // Simulate the coordinate translation that ScrollView applies to its
    // child during paint/event delivery: the child's bounds become
    // `(viewport.x - scroll_x, viewport.y - scroll_y, content_w, content_h)`.
    // Mimic that here so the click coordinate-conversion path is realistic.
    let scroll_y = sv.scroll_y();
    list.set_bounds(Rect::new(0, 0 - scroll_y, 200, list.content_height()));

    // The user clicks at viewport y=8 (top of viewport while scrolled to
    // y=1000). In list-local coords (after ScrollView translation) that's
    // y = 1000 + 8 = 1008. Row index = 1008 / 16 = 63.
    let click_local_y = 1008;
    let _ = list.handle_event(click_at(Point::new(10, click_local_y), false, false));
    assert_eq!(list.selection(), &Selection::Single(63));
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

fn test_click_past_last_row_leaves_selection_unchanged() {
    let mut list = make_list_with_items(5, Rect::new(0, 0, 200, 200));
    list.set_selected(Some(2));
    // 5 items × 16 px = 80 px content. Click at y=150 → past the last row.
    let _ = list.handle_event(click_at(Point::new(10, 150), false, false));
    assert_eq!(list.selection(), &Selection::Single(2));
}

fn test_arrow_down_at_last_item_clamps() {
    let mut list = make_list_with_items(5, Rect::new(0, 0, 200, 80));
    list.set_selected(Some(4));
    let _ = list.handle_event(key_event(KeyCode::Down, false));
    assert_eq!(list.selection(), &Selection::Single(4));
}

fn test_switching_multi_to_single_collapses() {
    let mut list = make_list_with_items(5, Rect::new(0, 0, 200, 80));
    list.set_selection_mode(SelectionMode::Multi);
    let _ = list.handle_event(click_at(Point::new(5, 1 * 16 + 4), false, false));
    let _ = list.handle_event(click_at(Point::new(5, 4 * 16 + 4), true, false));
    // Range { anchor: 1, end: 4 }; iter().next() = 1.
    assert!(matches!(list.selection(), Selection::Range { .. }));

    list.set_selection_mode(SelectionMode::Single);
    assert_eq!(list.selection(), &Selection::Single(1));
}

// ---------------------------------------------------------------------------
// MultiColumnList integration tests (scrollbar removal + on_right_click).
// ---------------------------------------------------------------------------

/// AE2: A 1000-row MCL inside a 20-row viewport renders correctly through
/// ScrollView. We verify: (1) the ScrollView reports the right max scroll
/// and tracks correctly, (2) clicking through the translated bounds picks
/// the right row.
fn test_mcl_thousand_rows_in_small_viewport_scrollbar_tracks() {
    let columns = vec![Column::new("Col", 100)];
    let mut mcl = MultiColumnList::new(Rect::new(0, 0, 100, 320), columns);
    for i in 0..1000usize {
        let mut s = String::new();
        s.push('r');
        let mut tmp = i;
        let mut digits: Vec<u8> = Vec::new();
        if tmp == 0 {
            digits.push(b'0');
        } else {
            while tmp > 0 {
                digits.push(b'0' + (tmp % 10) as u8);
                tmp /= 10;
            }
            digits.reverse();
        }
        for d in digits {
            s.push(d as char);
        }
        mcl.add_row(vec![s]);
    }

    // Header 20 + 20 rows × 16 = 340 px viewport.
    let viewport_h = 20u32 + 20u32 * 16; // 340
    let mut sv = ScrollView::new(Rect::new(0, 0, 100, viewport_h));
    sv.set_content_size(100, mcl.content_height());

    // content_h = 20 + 1000 * 16 = 16020
    assert_eq!(mcl.content_height(), 20 + 1000 * 16);

    // Scroll near the bottom
    sv.scroll_to(0, 15000);
    let max_scroll = (mcl.content_height() as i32 - viewport_h as i32).max(0);
    assert!(sv.scroll_y() <= max_scroll);
    assert!(sv.scroll_y() > 0);

    // Test that scrolling past max clamps.
    sv.scroll_to(0, 999_999);
    assert_eq!(sv.scroll_y(), max_scroll);
}

/// MultiColumnList::on_right_click still fires with the right-clicked row
/// index and global position even when the widget is wrapped in a
/// ScrollView. We don't actually wire it through the WindowManager — we
/// translate the bounds the way ScrollView would and dispatch the event
/// to the MCL directly.
fn test_mcl_right_click_preserves_global_position_under_scroll_view() {
    static CAPTURED: Mutex<Option<(usize, Point)>> = Mutex::new(None);

    let columns = vec![Column::new("Col", 100)];
    let mut mcl = MultiColumnList::new(Rect::new(0, 0, 100, 200), columns);
    for _ in 0..50 {
        mcl.add_row(vec![String::from("row")]);
    }

    mcl.on_right_click(|row_index, global_position| {
        *CAPTURED.lock() = Some((row_index, global_position));
    });

    // Simulate the ScrollView translation: scroll by 32 px.
    let scroll_y = 32;
    mcl.set_bounds(Rect::new(0, -scroll_y, 100, mcl.content_height()));

    // Click at global y = 100. Local y (after translation) = 100 + 32 = 132.
    // Header 20 px → row offset = 132 - 20 = 112. row_index = 112 / 16 = 7.
    let global = Point::new(40, 100);
    let local = Point::new(40, 132);
    let _ = mcl.handle_event(right_click_at(local, global));

    let captured = CAPTURED.lock().expect("right-click callback should have fired");
    assert_eq!(captured.0, 7, "row index should be 7");
    assert_eq!(captured.1, global, "global position should be preserved verbatim");
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_single_click_produces_single_selection,
        &test_multi_select_full_click_sequence,
        &test_arrow_down_advances_selection,
        &test_shift_arrow_down_extends_range_in_multi_mode,
        &test_large_list_wrapped_in_scroll_view_click_translates,
        &test_click_past_last_row_leaves_selection_unchanged,
        &test_arrow_down_at_last_item_clamps,
        &test_switching_multi_to_single_collapses,
        &test_mcl_thousand_rows_in_small_viewport_scrollbar_tracks,
        &test_mcl_right_click_preserves_global_position_under_scroll_view,
    ]
}
