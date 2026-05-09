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
    fn id(&self) -> WindowId {
        self.base.id()
    }

    fn bounds(&self) -> Rect {
        self.base.bounds()
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        self.relayout();
    }

    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.base.set_bounds_no_invalidate(bounds);
    }

    fn visible(&self) -> bool {
        self.base.visible()
    }

    fn set_visible(&mut self, visible: bool) {
        self.base.set_visible(visible);
    }

    fn parent(&self) -> Option<WindowId> {
        self.base.parent()
    }

    fn children(&self) -> &[WindowId] {
        self.base.children()
    }

    fn set_parent(&mut self, parent: Option<WindowId>) {
        self.base.set_parent(parent);
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

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Propagate
    }

    fn can_focus(&self) -> bool {
        false
    }

    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }

    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }
}
