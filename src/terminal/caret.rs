//! Caret state + blink-phase math.
//!
//! The screen owns the *logical* caret — position, visibility, shape.
//! The renderer asks for a `Caret` snapshot at paint time and decides
//! whether to draw it given an externally-supplied blink phase. This
//! separation keeps Screen free of any timer dependency (Screen is
//! unit-tested in isolation; pulling in a clock would push a heap-init
//! dependency into every test).
//!
//! Blink discipline: the compositor reads a monotonic millisecond
//! counter, computes `blink_on_at(ms)`, and passes it into the
//! renderer. When the renderer detects a phase transition it
//! invalidates only the caret cell — one repaint per second per
//! focused terminal, well under the budget. The renderer also drops
//! blink entirely while the terminal is in raw mode (zsh draws its
//! own caret).

use super::config::BLINK_INTERVAL_MS;
use super::screen::CursorShape;

/// A renderer-ready snapshot of the caret. Cheap to copy (24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
    pub shape: CursorShape,
}

impl Caret {
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub const fn hidden() -> Self {
        Caret {
            row: 0,
            col: 0,
            visible: false,
            shape: CursorShape::Block,
        }
    }
}

/// Compute the blink-phase at a monotonic millisecond timestamp.
/// Returns `true` when the caret should be drawn. Toggles every
/// [`BLINK_INTERVAL_MS`]; the on/off cycle is therefore
/// `2 * BLINK_INTERVAL_MS` end-to-end.
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn blink_on_at(ms: u64) -> bool {
    (ms / BLINK_INTERVAL_MS) % 2 == 0
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &tests::test_blink_starts_on,
        &tests::test_blink_toggles_at_interval,
        &tests::test_blink_full_cycle,
        &tests::test_hidden_caret_constant,
    ]
}

#[cfg(feature = "test")]
mod tests {
    use super::*;

    pub(super) fn test_blink_starts_on() {
        assert!(blink_on_at(0));
        assert!(blink_on_at(BLINK_INTERVAL_MS - 1));
    }

    pub(super) fn test_blink_toggles_at_interval() {
        assert!(blink_on_at(BLINK_INTERVAL_MS - 1));
        assert!(!blink_on_at(BLINK_INTERVAL_MS));
        assert!(!blink_on_at(2 * BLINK_INTERVAL_MS - 1));
        assert!(blink_on_at(2 * BLINK_INTERVAL_MS));
    }

    pub(super) fn test_blink_full_cycle() {
        // Over a 2 * BLINK_INTERVAL_MS window we should see exactly one
        // on→off and one off→on transition. Sample at every quarter.
        let q = BLINK_INTERVAL_MS / 2;
        assert!(blink_on_at(0));
        assert!(blink_on_at(q));
        assert!(!blink_on_at(BLINK_INTERVAL_MS + q));
        assert!(blink_on_at(2 * BLINK_INTERVAL_MS + q));
    }

    pub(super) fn test_hidden_caret_constant() {
        let h = Caret::hidden();
        assert!(!h.visible);
        assert_eq!(h.row, 0);
        assert_eq!(h.col, 0);
    }
}
