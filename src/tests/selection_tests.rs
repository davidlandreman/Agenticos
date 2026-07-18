//! Tests for the shared `Selection` model used by list-style widgets.

extern crate alloc;

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::lib::test_utils::Testable;
use crate::window::selection::{ArrowDirection, ClickMods, Selection, SelectionMode};

fn collect(sel: &Selection) -> Vec<usize> {
    sel.iter().collect()
}

fn test_single_is_selected() {
    let sel = Selection::Single(3);
    assert!(sel.is_selected(3));
    assert!(!sel.is_selected(4));
    assert_eq!(sel.len(), 1);
    assert!(!sel.is_empty());
}

fn test_multi_is_selected() {
    let mut set = BTreeSet::new();
    set.insert(1);
    set.insert(4);
    set.insert(7);
    let sel = Selection::Multi(set);

    assert!(sel.is_selected(1));
    assert!(sel.is_selected(4));
    assert!(sel.is_selected(7));
    assert!(!sel.is_selected(0));
    assert!(!sel.is_selected(2));
    assert!(!sel.is_selected(5));
    assert_eq!(sel.len(), 3);
}

fn test_range_iter_normalizes_descending() {
    // anchor > end must still yield indices in ascending order.
    let sel = Selection::Range { anchor: 5, end: 2 };
    assert_eq!(collect(&sel), [2, 3, 4, 5]);
    assert_eq!(sel.len(), 4);
    assert!(sel.is_selected(3));
    assert!(!sel.is_selected(1));
    assert!(!sel.is_selected(6));
}

fn test_range_iter_ascending() {
    let sel = Selection::Range { anchor: 2, end: 4 };
    assert_eq!(collect(&sel), [2, 3, 4]);
}

fn test_clear_multi_yields_none() {
    let mut set = BTreeSet::new();
    set.insert(1);
    set.insert(4);
    let mut sel = Selection::Multi(set);
    sel.clear();
    assert_eq!(sel, Selection::None);
    assert_eq!(sel.len(), 0);
    assert!(sel.is_empty());
}

fn test_out_of_range_is_selected_no_panic() {
    let sel = Selection::None;
    assert!(!sel.is_selected(0));
    assert!(!sel.is_selected(usize::MAX));

    let sel = Selection::Single(2);
    assert!(!sel.is_selected(usize::MAX));

    let sel = Selection::Range { anchor: 1, end: 3 };
    assert!(!sel.is_selected(usize::MAX));

    let mut set = BTreeSet::new();
    set.insert(1);
    let sel = Selection::Multi(set);
    assert!(!sel.is_selected(usize::MAX));
}

fn test_ae3_model_layer_click_sequence() {
    // AE3 (model layer): A=0, B=1, C=2, D=3.
    let mut sel = Selection::None;

    // Click on B with no mods → Single(B).
    sel.click(1, ClickMods::NONE, SelectionMode::Multi);
    assert_eq!(sel, Selection::Single(1));

    // Shift-click on D → Range { anchor: B, end: D }.
    sel.click(3, ClickMods::new(true, false), SelectionMode::Multi);
    assert_eq!(sel, Selection::Range { anchor: 1, end: 3 });

    // Ctrl-click on A → Multi({A} ∪ {B..=D}) = {0, 1, 2, 3}.
    sel.click(0, ClickMods::new(false, true), SelectionMode::Multi);
    let expected: BTreeSet<usize> = [0usize, 1, 2, 3].iter().copied().collect();
    assert_eq!(sel, Selection::Multi(expected));

    // Plain click on C collapses to Single(C).
    sel.click(2, ClickMods::NONE, SelectionMode::Multi);
    assert_eq!(sel, Selection::Single(2));
}

fn test_ctrl_click_toggles_idx() {
    let mut sel = Selection::None;
    sel.click(2, ClickMods::new(false, true), SelectionMode::Multi);
    // None + ctrl-click(2) → Multi({2}).
    let expected: BTreeSet<usize> = [2usize].iter().copied().collect();
    assert_eq!(sel, Selection::Multi(expected));

    // Ctrl-click on 5 adds 5.
    sel.click(5, ClickMods::new(false, true), SelectionMode::Multi);
    let expected: BTreeSet<usize> = [2usize, 5].iter().copied().collect();
    assert_eq!(sel, Selection::Multi(expected));

    // Ctrl-click on 2 removes 2.
    sel.click(2, ClickMods::new(false, true), SelectionMode::Multi);
    let expected: BTreeSet<usize> = [5usize].iter().copied().collect();
    assert_eq!(sel, Selection::Multi(expected));
}

fn test_single_mode_rejects_multi_and_range() {
    let mut sel = Selection::None;

    // Shift-click in Single mode → Single.
    sel.click(3, ClickMods::new(true, false), SelectionMode::Single);
    assert_eq!(sel, Selection::Single(3));

    // Ctrl-click in Single mode → Single.
    sel.click(7, ClickMods::new(false, true), SelectionMode::Single);
    assert_eq!(sel, Selection::Single(7));

    // Plain click in Single mode → Single.
    sel.click(2, ClickMods::NONE, SelectionMode::Single);
    assert_eq!(sel, Selection::Single(2));
}

fn test_arrow_down_clamps_at_last_index() {
    let mut sel = Selection::Single(4);
    sel.arrow(
        ArrowDirection::Down,
        5, // item_count=5, max idx 4
        ClickMods::NONE,
        SelectionMode::Single,
    );
    assert_eq!(sel, Selection::Single(4));
}

fn test_arrow_up_clamps_at_zero() {
    let mut sel = Selection::Single(0);
    sel.arrow(
        ArrowDirection::Up,
        5,
        ClickMods::NONE,
        SelectionMode::Single,
    );
    assert_eq!(sel, Selection::Single(0));
}

fn test_plain_arrow_moves_index() {
    let mut sel = Selection::Single(2);
    sel.arrow(
        ArrowDirection::Down,
        10,
        ClickMods::NONE,
        SelectionMode::Multi,
    );
    assert_eq!(sel, Selection::Single(3));

    sel.arrow(
        ArrowDirection::Up,
        10,
        ClickMods::NONE,
        SelectionMode::Multi,
    );
    assert_eq!(sel, Selection::Single(2));
}

fn test_shift_arrow_in_multi_mode_extends_range_from_anchor() {
    // Start from Single(2), shift+down extends a range from anchor=2.
    let mut sel = Selection::Single(2);
    sel.arrow(
        ArrowDirection::Down,
        10,
        ClickMods::new(true, false),
        SelectionMode::Multi,
    );
    assert_eq!(sel, Selection::Range { anchor: 2, end: 3 });

    // Continue extending: anchor stays at 2, end advances.
    sel.arrow(
        ArrowDirection::Down,
        10,
        ClickMods::new(true, false),
        SelectionMode::Multi,
    );
    assert_eq!(sel, Selection::Range { anchor: 2, end: 4 });

    // Reverse direction: anchor stays, end retreats.
    sel.arrow(
        ArrowDirection::Up,
        10,
        ClickMods::new(true, false),
        SelectionMode::Multi,
    );
    assert_eq!(sel, Selection::Range { anchor: 2, end: 3 });
}

fn test_shift_arrow_in_single_mode_behaves_as_plain_arrow() {
    let mut sel = Selection::Single(2);
    sel.arrow(
        ArrowDirection::Down,
        10,
        ClickMods::new(true, false),
        SelectionMode::Single,
    );
    // Single mode ignores the shift modifier — result is plain move.
    assert_eq!(sel, Selection::Single(3));
}

fn test_extend_to_uses_existing_anchor() {
    let mut sel = Selection::Single(2);
    sel.extend_to(5);
    assert_eq!(sel, Selection::Range { anchor: 2, end: 5 });

    // Already a range: extend_to keeps the anchor.
    let mut sel = Selection::Range { anchor: 1, end: 3 };
    sel.extend_to(7);
    assert_eq!(sel, Selection::Range { anchor: 1, end: 7 });

    // None: anchor falls back to idx itself.
    let mut sel = Selection::None;
    sel.extend_to(4);
    assert_eq!(sel, Selection::Range { anchor: 4, end: 4 });
}

fn test_iter_single_and_multi() {
    let sel = Selection::Single(7);
    assert_eq!(collect(&sel), [7]);

    let sel = Selection::None;
    assert_eq!(collect(&sel), Vec::<usize>::new());

    let mut set = BTreeSet::new();
    set.insert(4);
    set.insert(1);
    set.insert(9);
    let sel = Selection::Multi(set);
    // BTreeSet iterates in ascending order.
    assert_eq!(collect(&sel), [1, 4, 9]);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_single_is_selected,
        &test_multi_is_selected,
        &test_range_iter_normalizes_descending,
        &test_range_iter_ascending,
        &test_clear_multi_yields_none,
        &test_out_of_range_is_selected_no_panic,
        &test_ae3_model_layer_click_sequence,
        &test_ctrl_click_toggles_idx,
        &test_single_mode_rejects_multi_and_range,
        &test_arrow_down_clamps_at_last_index,
        &test_arrow_up_clamps_at_zero,
        &test_plain_arrow_moves_index,
        &test_shift_arrow_in_multi_mode_extends_range_from_anchor,
        &test_shift_arrow_in_single_mode_behaves_as_plain_arrow,
        &test_extend_to_uses_existing_anchor,
        &test_iter_single_and_multi,
    ]
}
