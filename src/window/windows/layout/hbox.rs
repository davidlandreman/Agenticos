//! `HBox` layout container — arranges children horizontally.

use alloc::vec::Vec;

use crate::window::manager::with_active_manager;
use crate::window::windows::base::WindowBase;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::spacer::SizeHint;
use super::vbox::{compute_main_axis_bounds, Axis};

/// Horizontal box. Children are placed left-to-right on the main axis
/// (width); each child fills the box's full height.
pub struct HBox {
    base: WindowBase,
    hints: Vec<SizeHint>,
}

impl HBox {
    /// Create a new `HBox` covering `bounds`.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        HBox {
            base: WindowBase::new(bounds),
            hints: Vec::new(),
        }
    }

    /// Create a new `HBox` with a specific id.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        HBox {
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

    /// Compute the per-child bounds (relative to the `HBox`'s own
    /// coordinate system). See `VBox::compute_child_bounds` for the
    /// distribution rules — they are identical, just rotated.
    pub fn compute_child_bounds(&self) -> Vec<Rect> {
        compute_main_axis_bounds(self.base.bounds(), &self.hints, Axis::Horizontal)
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

impl Window for HBox {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    // Custom override: HBox relayouts its children on resize.
    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        self.relayout();
    }

    fn add_child(&mut self, child: WindowId) {
        HBox::add_child(self, child, SizeHint::Fill(1));
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
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Propagate
    }
}
