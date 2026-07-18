//! `VBox` layout container — stacks children vertically.

use alloc::vec::Vec;

use crate::window::manager::with_active_manager;
use crate::window::windows::base::WindowBase;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::spacer::SizeHint;

/// Vertical box. Children are stacked top-to-bottom on the main axis
/// (height); each child fills the box's full width.
pub struct VBox {
    base: WindowBase,
    /// Per-child sizing hint, in the same order as `WindowBase::children`.
    hints: Vec<SizeHint>,
}

impl VBox {
    /// Create a new `VBox` covering `bounds`.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        VBox {
            base: WindowBase::new(bounds),
            hints: Vec::new(),
        }
    }

    /// Create a new `VBox` with a specific id.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        VBox {
            base: WindowBase::new_with_id(id, bounds),
            hints: Vec::new(),
        }
    }

    /// Append a child window with the given main-axis sizing hint and
    /// trigger a relayout so the child's bounds are correct immediately.
    pub fn add_child(&mut self, id: WindowId, hint: SizeHint) {
        self.base.add_child(id);
        self.hints.push(hint);
        self.relayout();
    }

    /// Compute the per-child bounds (relative to the `VBox`'s own
    /// coordinate system) given the current container bounds and the
    /// stored sizing hints. Returned slice matches the child order.
    ///
    /// Distribution rules:
    /// - Each `Fixed(n)` consumes exactly `n` pixels.
    /// - `MinContent` is treated as zero (no minimum-size API yet).
    /// - Remaining height after `Fixed` + `MinContent` is split among
    ///   `Fill` children proportionally to their weight (integer math).
    /// - When fixed sizes alone exceed the container, leftover children
    ///   are clipped to zero — never panic.
    pub fn compute_child_bounds(&self) -> Vec<Rect> {
        compute_main_axis_bounds(self.base.bounds(), &self.hints, Axis::Vertical)
    }

    fn relayout(&mut self) {
        let layouts = self.compute_child_bounds();
        let ids = self.base.children().to_vec();
        with_active_manager(|wm| {
            for (id, rect) in ids.iter().zip(layouts.iter()) {
                wm.with_window_mut(*id, |w| w.set_bounds(*rect));
            }
        });
    }
}

impl Window for VBox {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    // Custom override: VBox relayouts its children when its own bounds
    // change.
    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        self.relayout();
    }

    fn add_child(&mut self, child: WindowId) {
        // The trait-level `add_child` has no hint, so default to a
        // single Fill weight. Callers that want a specific hint should
        // use `VBox::add_child(id, hint)` directly.
        VBox::add_child(self, child, SizeHint::Fill(1));
    }

    fn remove_child(&mut self, child: WindowId) {
        if let Some(idx) = self.base.children().iter().position(|c| *c == child) {
            self.base.remove_child(child);
            if idx < self.hints.len() {
                self.hints.remove(idx);
            }
            self.relayout();
        }
    }

    fn paint(&mut self, _device: &mut dyn GraphicsDevice) {
        // VBox paints nothing — children fill its area.
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Propagate
    }
}

/// Which axis a layout container distributes children along.
#[derive(Debug, Clone, Copy)]
pub(super) enum Axis {
    Vertical,
    Horizontal,
}

/// Shared main-axis distribution math used by both `VBox` and `HBox`.
///
/// `outer` is the container's own bounds (parent-relative). The returned
/// rects are in the container's own coordinate system.
pub(super) fn compute_main_axis_bounds(outer: Rect, hints: &[SizeHint], axis: Axis) -> Vec<Rect> {
    let main_total: u32 = match axis {
        Axis::Vertical => outer.height,
        Axis::Horizontal => outer.width,
    };
    let cross_total: u32 = match axis {
        Axis::Vertical => outer.width,
        Axis::Horizontal => outer.height,
    };

    // Pass 1: tally fixed/min-content consumption and total fill weight.
    let mut consumed: u32 = 0;
    let mut total_weight: u32 = 0;
    for hint in hints {
        match *hint {
            SizeHint::Fixed(n) => consumed = consumed.saturating_add(n),
            SizeHint::MinContent => {
                // Today, no Window exposes a min-content size, so this
                // contributes zero. The variant is kept so callers can
                // express intent for a future extension.
            }
            SizeHint::Fill(w) => total_weight = total_weight.saturating_add(w),
        }
    }

    // Available leftover for Fill weights.
    let leftover: u32 = main_total.saturating_sub(consumed);

    // Pass 2: emit rects. Track the running offset on the main axis,
    // and clip each rect so its main-axis size never exceeds what is
    // actually available (the running cursor stops at `main_total`).
    let mut out = Vec::with_capacity(hints.len());
    let mut cursor: u32 = 0;
    let mut weight_remaining = total_weight;
    let mut leftover_remaining = leftover;

    for hint in hints {
        let raw_size = match *hint {
            SizeHint::Fixed(n) => n,
            SizeHint::MinContent => 0,
            SizeHint::Fill(w) => {
                if total_weight == 0 || w == 0 {
                    0
                } else if weight_remaining == w {
                    // Last Fill child claims everything left so rounding
                    // residue does not leak below `main_total`.
                    leftover_remaining
                } else {
                    let share = (leftover as u64 * w as u64) / total_weight as u64;
                    let share = share as u32;
                    weight_remaining = weight_remaining.saturating_sub(w);
                    leftover_remaining = leftover_remaining.saturating_sub(share);
                    share
                }
            }
        };

        // Clip the rect's main-axis size so it never extends past the
        // container. When the running cursor has already reached the
        // total, subsequent rects collapse to size zero rather than
        // panicking on overflow.
        let space_left = main_total.saturating_sub(cursor);
        let size = raw_size.min(space_left);

        let rect = match axis {
            Axis::Vertical => Rect::new(0, cursor as i32, cross_total, size),
            Axis::Horizontal => Rect::new(cursor as i32, 0, size, cross_total),
        };
        out.push(rect);
        cursor = cursor.saturating_add(raw_size);
    }

    out
}
