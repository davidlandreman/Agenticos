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

/// Compute the blink-phase at a monotonic millisecond timestamp.
/// Returns `true` when the caret should be drawn. Toggles every
/// [`BLINK_INTERVAL_MS`]; the on/off cycle is therefore
/// `2 * BLINK_INTERVAL_MS` end-to-end.
pub fn blink_on_at(ms: u64) -> bool {
    (ms / BLINK_INTERVAL_MS) % 2 == 0
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

