//! Shared selection model for list-shaped widgets.
//!
//! Defines a `Selection` enum that can represent no selection, a single
//! selected index, a multi-set of selected indices, or an inclusive
//! contiguous range with an anchor. List-style widgets (lists, tree views,
//! icon views, etc.) use this model so click and arrow-key semantics stay
//! consistent across the kernel's window components.
//!
//! Patterns:
//! - `alloc::collections::BTreeSet` is used (per `.claude/rules/no-std.md`);
//!   never `std::collections::HashSet`.
//! - `ClickMods` mirrors the shape of `KeyModifiers` from `event.rs` but
//!   carries only the modifiers relevant to mouse selection (shift, ctrl).
//! - All helpers are bounds-safe: out-of-range queries return `false` rather
//!   than panicking. The widget enforces `item_count` for arrow movement.

extern crate alloc;

use alloc::collections::BTreeSet;

/// What kind of selection a widget supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// At most one item may be selected at a time.
    Single,
    /// Any subset of items may be selected.
    Multi,
}

/// Mouse-modifier state at the time of a click. Mirrors the relevant subset
/// of `event::KeyModifiers` so call sites can pass either source through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickMods {
    pub shift: bool,
    pub ctrl: bool,
}

impl ClickMods {
    pub const NONE: ClickMods = ClickMods { shift: false, ctrl: false };

    pub const fn new(shift: bool, ctrl: bool) -> Self {
        Self { shift, ctrl }
    }
}

/// Direction for arrow-key navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowDirection {
    Up,
    Down,
}

/// Selection state shared across list-style widgets.
///
/// `Range { anchor, end }` is inclusive at both ends and `anchor` may be
/// greater than `end` (it tracks where the user started extending from).
/// `iter` always normalizes to ascending order; ordering is preserved in the
/// stored representation only so that `extend_to`/`arrow` can keep extending
/// from the original anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    None,
    Single(usize),
    Multi(BTreeSet<usize>),
    Range { anchor: usize, end: usize },
}

impl Default for Selection {
    fn default() -> Self {
        Selection::None
    }
}

impl Selection {
    /// Returns `true` when `idx` is selected. Never panics — out-of-range
    /// indices simply return `false`.
    pub fn is_selected(&self, idx: usize) -> bool {
        match self {
            Selection::None => false,
            Selection::Single(s) => *s == idx,
            Selection::Multi(set) => set.contains(&idx),
            Selection::Range { anchor, end } => {
                let (lo, hi) = normalize(*anchor, *end);
                idx >= lo && idx <= hi
            }
        }
    }

    /// Number of selected items.
    pub fn len(&self) -> usize {
        match self {
            Selection::None => 0,
            Selection::Single(_) => 1,
            Selection::Multi(set) => set.len(),
            Selection::Range { anchor, end } => {
                let (lo, hi) = normalize(*anchor, *end);
                hi - lo + 1
            }
        }
    }

    /// Returns `true` when nothing is selected.
    pub fn is_empty(&self) -> bool {
        matches!(self, Selection::None)
    }

    /// Iterate selected indices in ascending order. For `Range`, the iterator
    /// normalizes so that `Range { anchor: 5, end: 2 }` yields `2, 3, 4, 5`.
    pub fn iter(&self) -> SelectionIter<'_> {
        match self {
            Selection::None => SelectionIter::Empty,
            Selection::Single(idx) => SelectionIter::Single(Some(*idx)),
            Selection::Multi(set) => SelectionIter::Multi(set.iter()),
            Selection::Range { anchor, end } => {
                let (lo, hi) = normalize(*anchor, *end);
                SelectionIter::Range { next: lo, last: hi, done: false }
            }
        }
    }

    /// Drop all selected items.
    pub fn clear(&mut self) {
        *self = Selection::None;
    }

    /// Apply mouse-click semantics to the selection.
    ///
    /// - Plain click collapses to `Single(idx)`.
    /// - Shift-click in `Multi` mode extends to `Range { anchor, end: idx }`
    ///   where `anchor` is derived from the current selection (the existing
    ///   single index, the existing range's anchor, the smallest multi index,
    ///   or `idx` itself if there's no anchor yet).
    /// - Ctrl-click in `Multi` mode toggles `idx`. A `Range` selection is
    ///   first expanded into its `Multi` equivalent so the toggle is well
    ///   defined.
    /// - In `Single` mode, both shift- and ctrl-click fall back to
    ///   `Single(idx)`.
    pub fn click(&mut self, idx: usize, mods: ClickMods, mode: SelectionMode) {
        if matches!(mode, SelectionMode::Single) {
            *self = Selection::Single(idx);
            return;
        }

        // SelectionMode::Multi
        if mods.shift {
            let anchor = self.current_anchor().unwrap_or(idx);
            *self = Selection::Range { anchor, end: idx };
            return;
        }

        if mods.ctrl {
            let mut set = self.as_multi_set();
            if !set.remove(&idx) {
                set.insert(idx);
            }
            *self = if set.is_empty() {
                Selection::None
            } else if set.len() == 1 {
                // Stay in Multi to honor the "Multi was requested" intent —
                // keep it as a 1-element set rather than collapsing. This
                // matches how subsequent ctrl-clicks should keep extending
                // the multi-set rather than starting from a Single.
                Selection::Multi(set)
            } else {
                Selection::Multi(set)
            };
            return;
        }

        // Plain click
        *self = Selection::Single(idx);
    }

    /// Apply arrow-key semantics to the selection.
    ///
    /// Movement clamps to `[0, item_count - 1]`. With `item_count == 0` the
    /// selection is left unchanged. Plain arrows move the focused index;
    /// shift+arrow in `Multi` mode extends a range from the current anchor.
    /// In `Single` mode shift is treated as plain (no range extension).
    pub fn arrow(
        &mut self,
        direction: ArrowDirection,
        item_count: usize,
        mods: ClickMods,
        mode: SelectionMode,
    ) {
        if item_count == 0 {
            return;
        }
        let max = item_count - 1;

        // Compute the index we're moving from.
        let current = self.current_focus().unwrap_or(0);
        let target = match direction {
            ArrowDirection::Up => current.saturating_sub(1),
            ArrowDirection::Down => {
                if current >= max {
                    max
                } else {
                    current + 1
                }
            }
        };

        if mods.shift && matches!(mode, SelectionMode::Multi) {
            let anchor = self.current_anchor().unwrap_or(current);
            *self = Selection::Range { anchor, end: target };
        } else {
            *self = Selection::Single(target);
        }
    }

    /// Extend the current selection to a new endpoint, anchored on the
    /// existing anchor (or `idx` itself when no anchor is available).
    /// Used for shift-click.
    pub fn extend_to(&mut self, idx: usize) {
        let anchor = self.current_anchor().unwrap_or(idx);
        *self = Selection::Range { anchor, end: idx };
    }

    /// The "anchor" of the current selection — where range extensions pivot
    /// from. Returns `None` for `Selection::None`.
    fn current_anchor(&self) -> Option<usize> {
        match self {
            Selection::None => None,
            Selection::Single(idx) => Some(*idx),
            Selection::Multi(set) => set.iter().next().copied(),
            Selection::Range { anchor, .. } => Some(*anchor),
        }
    }

    /// The currently "focused" index for arrow-key navigation. For ranges,
    /// this is `end` (the moving cursor); for multi sets, the smallest index;
    /// for `Single`, the index itself; for `None`, `None`.
    fn current_focus(&self) -> Option<usize> {
        match self {
            Selection::None => None,
            Selection::Single(idx) => Some(*idx),
            Selection::Multi(set) => set.iter().next_back().copied(),
            Selection::Range { end, .. } => Some(*end),
        }
    }

    /// Expand the current selection into a `BTreeSet` representation suitable
    /// for ctrl-click toggling. Empty for `Selection::None`.
    fn as_multi_set(&self) -> BTreeSet<usize> {
        let mut set = BTreeSet::new();
        match self {
            Selection::None => {}
            Selection::Single(idx) => {
                set.insert(*idx);
            }
            Selection::Multi(existing) => {
                for &idx in existing.iter() {
                    set.insert(idx);
                }
            }
            Selection::Range { anchor, end } => {
                let (lo, hi) = normalize(*anchor, *end);
                for idx in lo..=hi {
                    set.insert(idx);
                }
            }
        }
        set
    }
}

fn normalize(anchor: usize, end: usize) -> (usize, usize) {
    if anchor <= end {
        (anchor, end)
    } else {
        (end, anchor)
    }
}

/// Iterator over the selected indices in ascending order.
pub enum SelectionIter<'a> {
    Empty,
    Single(Option<usize>),
    Multi(alloc::collections::btree_set::Iter<'a, usize>),
    Range { next: usize, last: usize, done: bool },
}

impl<'a> Iterator for SelectionIter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        match self {
            SelectionIter::Empty => None,
            SelectionIter::Single(slot) => slot.take(),
            SelectionIter::Multi(iter) => iter.next().copied(),
            SelectionIter::Range { next, last, done } => {
                if *done {
                    return None;
                }
                let value = *next;
                if *next == *last {
                    *done = true;
                } else {
                    *next += 1;
                }
                Some(value)
            }
        }
    }
}
