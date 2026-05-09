//! `Padding` layout container — wraps a single child and shrinks the
//! child's bounds by the configured insets.

use crate::window::manager::with_active_manager;
use crate::window::windows::base::WindowBase;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

/// Padding container with a single child.
///
/// The child's bounds are computed as the padding's bounds shrunk by
/// `(top, right, bottom, left)`. If the insets exceed the container on
/// either axis, the child is given a zero-sized rect (never negative).
pub struct Padding {
    base: WindowBase,
    child: Option<WindowId>,
    top: u32,
    right: u32,
    bottom: u32,
    left: u32,
}

impl Padding {
    /// Create a new `Padding` with the given outer bounds and insets.
    pub fn new(bounds: Rect, top: u32, right: u32, bottom: u32, left: u32) -> Self {
        Padding {
            base: WindowBase::new(bounds),
            child: None,
            top,
            right,
            bottom,
            left,
        }
    }

    /// Create a new `Padding` with a specific id.
    pub fn new_with_id(
        id: WindowId,
        bounds: Rect,
        top: u32,
        right: u32,
        bottom: u32,
        left: u32,
    ) -> Self {
        Padding {
            base: WindowBase::new_with_id(id, bounds),
            child: None,
            top,
            right,
            bottom,
            left,
        }
    }

    /// Set the child window. The child must already be registered with
    /// the `WindowManager`. Triggers a relayout so the child's bounds
    /// match immediately.
    pub fn set_child(&mut self, id: WindowId) {
        // Replace any previous child entry in WindowBase::children.
        if let Some(prev) = self.child.take() {
            if prev != id {
                self.base.remove_child(prev);
            }
        }
        self.child = Some(id);
        self.base.add_child(id);
        self.relayout();
    }

    /// Compute the child's bounds, in the `Padding`'s own coordinate
    /// system (i.e. relative to the `Padding` window itself, since
    /// `WindowBase` stores bounds parent-relative). When the insets
    /// exceed the container on either axis, the child rect has zero
    /// width or height (rather than wrapping into negative values).
    pub fn child_bounds(&self) -> Rect {
        let outer = self.base.bounds();
        let horizontal = self.left.saturating_add(self.right);
        let vertical = self.top.saturating_add(self.bottom);
        let inner_w = outer.width.saturating_sub(horizontal);
        let inner_h = outer.height.saturating_sub(vertical);
        Rect::new(
            self.left as i32,
            self.top as i32,
            inner_w,
            inner_h,
        )
    }

    /// Walk the single child and write its computed bounds back through
    /// the active `WindowManager`. Silently does nothing when invoked
    /// outside a `WindowManager::with_window_mut` callback (e.g. during
    /// initial construction before the container has been inserted into
    /// the manager).
    fn relayout(&mut self) {
        let child_id = match self.child {
            Some(id) => id,
            None => return,
        };
        let new_bounds = self.child_bounds();
        with_active_manager(|wm| {
            wm.with_window_mut(child_id, |w| w.set_bounds(new_bounds));
        });
    }
}

impl Window for Padding {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    // Custom override: propagate the new bounds to the inner child.
    fn set_bounds(&mut self, bounds: Rect) {
        // Update our own bounds first, then propagate to the child.
        self.base.set_bounds(bounds);
        self.relayout();
    }

    fn add_child(&mut self, child: WindowId) {
        // Treat a manually-added child as the single child slot.
        self.set_child(child);
    }

    fn remove_child(&mut self, child: WindowId) {
        if self.child == Some(child) {
            self.child = None;
        }
        self.base.remove_child(child);
    }

    fn paint(&mut self, _device: &mut dyn GraphicsDevice) {
        // Padding paints nothing of its own — the child fills its area
        // (or the parent's background shows through any insets).
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Propagate
    }
}
