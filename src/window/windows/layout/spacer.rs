//! Spacer window and `SizeHint` enum used by the layout containers.
//!
//! `Spacer` is a fixed-size empty window used to introduce explicit gaps
//! between siblings in a `VBox`/`HBox`. It paints nothing — sampled pixels
//! fall through to the parent container's background.
//!
//! `SizeHint` is the per-child sizing instruction that `VBox`/`HBox` use to
//! distribute their main-axis space between children.

use crate::window::windows::base::WindowBase;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

/// Per-child sizing hint for `VBox` / `HBox`.
///
/// On the box's main axis (height for `VBox`, width for `HBox`):
/// - `Fixed(n)` — exactly `n` pixels.
/// - `Fill(weight)` — claim a share of the remaining space proportional
///   to `weight`, after `Fixed` and `MinContent` children have taken
///   their portion.
/// - `MinContent` — ask the child for its minimum size on the main
///   axis. Today no `Window` exposes a minimum-size API, so this
///   degrades to zero and behaves like `Fixed(0)`. The variant exists
///   as a future extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeHint {
    /// Exact pixel size on the box's main axis.
    Fixed(u32),
    /// Weight for proportional distribution of leftover space.
    Fill(u32),
    /// Use the child's minimum size; defaults to `0` if the child has
    /// no minimum-size API.
    MinContent,
}

/// An empty window used as an explicit gap between siblings.
///
/// `Spacer` does not paint — its area is left to whatever the parent
/// drew underneath, so the visible effect is "this many pixels of
/// background between my neighbours."
pub struct Spacer {
    base: WindowBase,
}

impl Spacer {
    /// Create a new `Spacer` covering `bounds`.
    pub fn new(bounds: Rect) -> Self {
        Spacer {
            base: WindowBase::new(bounds),
        }
    }

    /// Create a new `Spacer` with a specific id.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        Spacer {
            base: WindowBase::new_with_id(id, bounds),
        }
    }
}

impl Window for Spacer {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, _device: &mut dyn GraphicsDevice) {
        // Intentionally blank: Spacer leaves its area untouched so the
        // parent's background shows through.
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Ignored
    }
}
