//! `Splitter` — two-pane container with a draggable divider.
//!
//! A `Splitter` exposes two slots (`first` and `second`) that hold child
//! window ids. The container is split along one axis at a configurable
//! pixel offset (`divider_position`); a thin divider strip sits between the
//! two panes and can be dragged with the mouse to re-balance them. Each
//! slot carries a minimum-size constraint that clamps how far the divider
//! can move toward that pane.
//!
//! Layout follows the same pattern as `VBox`/`HBox`:
//! - `set_bounds` writes the new container bounds, then calls `relayout()`.
//! - `relayout()` walks the two slot ids and writes each child's bounds
//!   via `WindowManager::with_window_mut(child, |w| w.set_bounds(rect))`.
//!   This is the same active-manager flow the layout primitives use; it
//!   means a real `set_bounds` propagation runs through the registry, so
//!   children invalidate as they would for any other geometry change.
//!
//! Drag interaction follows the `FrameWindow` mouse-down/move/up pattern:
//! `ButtonDown` on the divider strip sets `pressed = true`; subsequent
//! `Move` events update `divider_position` (clamped by both minimums) and
//! relayout; `ButtonUp` clears `pressed`.

use crate::graphics::color::Color;
use crate::window::event::MouseEventType;
use crate::window::manager::with_active_manager;
use crate::window::windows::base::WindowBase;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

/// Width of the draggable divider strip in pixels.
pub const DIVIDER_WIDTH: u32 = 4;

/// Splitter orientation: which axis the divider strip runs along.
///
/// - `Horizontal` — divider strip runs horizontally; the two panes are
///   stacked top/bottom (split is along the y axis).
/// - `Vertical` — divider strip runs vertically; the two panes sit
///   side-by-side left/right (split is along the x axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitterOrientation {
    Horizontal,
    Vertical,
}

/// Two-pane container with a draggable divider.
pub struct Splitter {
    base: WindowBase,
    orientation: SplitterOrientation,
    first: Option<WindowId>,
    second: Option<WindowId>,
    first_min: u32,
    second_min: u32,
    /// Pixels from the start edge along the divided axis. The divider
    /// strip occupies `[divider_position, divider_position + DIVIDER_WIDTH)`
    /// on that axis.
    divider_position: u32,
    /// True while the user is dragging the divider.
    pressed: bool,
    /// Background color drawn behind the divider strip.
    bg_color: Color,
}

impl Splitter {
    /// Create a horizontally-divided splitter (panes stacked top/bottom).
    pub fn new_horizontal(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), SplitterOrientation::Horizontal, bounds)
    }

    /// Create a vertically-divided splitter (panes side-by-side).
    pub fn new_vertical(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), SplitterOrientation::Vertical, bounds)
    }

    /// Create a splitter with a specific window id and orientation.
    /// Default divider position is half of the divided-axis size.
    pub fn new_with_id(id: WindowId, orientation: SplitterOrientation, bounds: Rect) -> Self {
        let total = main_axis_total(orientation, bounds);
        let divider_position = total / 2;
        Splitter {
            base: WindowBase::new_with_id(id, bounds),
            orientation,
            first: None,
            second: None,
            first_min: 0,
            second_min: 0,
            divider_position,
            pressed: false,
            bg_color: Color::GRAY,
        }
    }

    /// Set (or replace) the first pane. The container is relayouted so
    /// the child's bounds are correct immediately.
    pub fn set_first(&mut self, id: WindowId, min_size: u32) {
        if let Some(prev) = self.first.take() {
            if prev != id {
                self.base.remove_child(prev);
            }
        }
        self.first = Some(id);
        self.first_min = min_size;
        if !self.base.children().contains(&id) {
            self.base.add_child(id);
        }
        self.clamp_divider_position();
        self.relayout();
        self.base.invalidate();
    }

    /// Set (or replace) the second pane.
    pub fn set_second(&mut self, id: WindowId, min_size: u32) {
        if let Some(prev) = self.second.take() {
            if prev != id {
                self.base.remove_child(prev);
            }
        }
        self.second = Some(id);
        self.second_min = min_size;
        if !self.base.children().contains(&id) {
            self.base.add_child(id);
        }
        self.clamp_divider_position();
        self.relayout();
        self.base.invalidate();
    }

    /// Move the divider to the given pixel offset (clamped by minimums)
    /// and relayout the children.
    pub fn set_divider_position(&mut self, position: u32) {
        self.divider_position = position;
        self.clamp_divider_position();
        self.relayout();
        self.base.invalidate();
    }

    /// Current divider offset, in pixels from the start edge along the
    /// divided axis.
    pub fn divider_position(&self) -> u32 {
        self.divider_position
    }

    /// The orientation chosen at construction.
    pub fn orientation(&self) -> SplitterOrientation {
        self.orientation
    }

    /// Compute the divider strip's bounds (parent-relative — same
    /// coordinate space as `self.base.bounds()`).
    fn divider_strip_bounds(&self) -> Rect {
        let b = self.base.bounds();
        match self.orientation {
            SplitterOrientation::Vertical => Rect::new(
                b.x + self.divider_position as i32,
                b.y,
                DIVIDER_WIDTH,
                b.height,
            ),
            SplitterOrientation::Horizontal => Rect::new(
                b.x,
                b.y + self.divider_position as i32,
                b.width,
                DIVIDER_WIDTH,
            ),
        }
    }

    /// Compute the per-pane bounds (in the splitter's own coordinate
    /// system, i.e. relative to its parent — matching the bounds we
    /// wrote into `WindowBase`). Returned as `(first, second)`.
    fn compute_pane_bounds(&self) -> (Rect, Rect) {
        let b = self.base.bounds();
        let total = main_axis_total(self.orientation, b);
        // `divider_position` should already be clamped, but treat it
        // defensively here so a stale value can never produce a negative
        // size.
        let pos = self.divider_position.min(total);
        let strip = DIVIDER_WIDTH.min(total.saturating_sub(pos));
        let second_size = total.saturating_sub(pos).saturating_sub(strip);
        match self.orientation {
            SplitterOrientation::Vertical => {
                let first = Rect::new(b.x, b.y, pos, b.height);
                let second = Rect::new(
                    b.x + pos as i32 + strip as i32,
                    b.y,
                    second_size,
                    b.height,
                );
                (first, second)
            }
            SplitterOrientation::Horizontal => {
                let first = Rect::new(b.x, b.y, b.width, pos);
                let second = Rect::new(
                    b.x,
                    b.y + pos as i32 + strip as i32,
                    b.width,
                    second_size,
                );
                (first, second)
            }
        }
    }

    /// Clamp `divider_position` so both panes meet their minimums when
    /// space allows. When the container is too small to honor both
    /// minimums, center the divider — both panes overflow at minimum,
    /// rather than one being squashed to zero.
    fn clamp_divider_position(&mut self) {
        let total = main_axis_total(self.orientation, self.base.bounds());
        let needed = self
            .first_min
            .saturating_add(self.second_min)
            .saturating_add(DIVIDER_WIDTH);
        if total < needed {
            // Not enough room: center the divider so each pane gets
            // exactly its declared minimum (content overflows; that is
            // acceptable per the design).
            self.divider_position = self.first_min;
            return;
        }
        let lo = self.first_min;
        let hi = total.saturating_sub(self.second_min).saturating_sub(DIVIDER_WIDTH);
        if self.divider_position < lo {
            self.divider_position = lo;
        } else if self.divider_position > hi {
            self.divider_position = hi;
        }
    }

    /// Walk the two slot ids and write their freshly-computed bounds
    /// back into the registry via the active manager. Mirrors
    /// `VBox::relayout` / `HBox::relayout`.
    fn relayout(&mut self) {
        let (first_rect, second_rect) = self.compute_pane_bounds();
        let first = self.first;
        let second = self.second;
        with_active_manager(|wm| {
            if let Some(id) = first {
                wm.with_window_mut(id, |w| w.set_bounds(first_rect));
            }
            if let Some(id) = second {
                wm.with_window_mut(id, |w| w.set_bounds(second_rect));
            }
        });
    }

    /// Local-coordinate hit test for the divider strip. `local_point` is
    /// relative to the splitter's own bounds (origin at `bounds().x,y`).
    fn local_point_on_divider(&self, local_x: i32, local_y: i32) -> bool {
        let b = self.base.bounds();
        match self.orientation {
            SplitterOrientation::Vertical => {
                let strip_x = self.divider_position as i32;
                local_x >= strip_x
                    && local_x < strip_x + DIVIDER_WIDTH as i32
                    && local_y >= 0
                    && local_y < b.height as i32
            }
            SplitterOrientation::Horizontal => {
                let strip_y = self.divider_position as i32;
                local_y >= strip_y
                    && local_y < strip_y + DIVIDER_WIDTH as i32
                    && local_x >= 0
                    && local_x < b.width as i32
            }
        }
    }

    /// Update the divider position from a mouse coordinate (in the
    /// splitter's local coordinate space — relative to its bounds).
    fn move_divider_to(&mut self, local_x: i32, local_y: i32) {
        let raw = match self.orientation {
            SplitterOrientation::Vertical => local_x,
            SplitterOrientation::Horizontal => local_y,
        };
        let clamped = raw.max(0) as u32;
        self.divider_position = clamped;
        self.clamp_divider_position();
        self.relayout();
        self.base.invalidate();
    }
}

impl Window for Splitter {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    /// When the container resizes, preserve the divider's relative ratio
    /// of available space, then relayout. Falls back to "no change in
    /// position" when the previous size was zero (no meaningful ratio
    /// existed).
    fn set_bounds(&mut self, bounds: Rect) {
        let old_total = main_axis_total(self.orientation, self.base.bounds());
        let new_total = main_axis_total(self.orientation, bounds);

        if old_total > 0 && new_total > 0 {
            // Preserve ratio with rounded integer math:
            // new_position = old_position * new_total / old_total.
            let scaled = (self.divider_position as u64 * new_total as u64) / old_total as u64;
            self.divider_position = scaled as u32;
        }

        self.base.set_bounds(bounds);
        self.clamp_divider_position();
        self.relayout();
    }

    fn add_child(&mut self, child: WindowId) {
        // Trait-level `add_child` has no slot/min hint. Fill the first
        // empty slot, falling through to the second. Most callers should
        // use `set_first` / `set_second` directly for explicit control.
        if self.first.is_none() {
            self.set_first(child, 0);
        } else if self.second.is_none() {
            self.set_second(child, 0);
        } else {
            // Both slots filled; record the child on the base so the
            // caller's tree shape is preserved, but do not lay it out.
            self.base.add_child(child);
        }
    }

    fn remove_child(&mut self, child: WindowId) {
        if self.first == Some(child) {
            self.first = None;
            self.first_min = 0;
        }
        if self.second == Some(child) {
            self.second = None;
            self.second_min = 0;
        }
        self.base.remove_child(child);
        self.relayout();
        self.base.invalidate();
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }
        if !self.base.needs_repaint() {
            return;
        }

        // Paint only the divider strip — the two panes draw themselves.
        let strip = self.divider_strip_bounds();
        device.fill_rect(strip.x, strip.y, strip.width, strip.height, self.bg_color);

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => match mouse_event.event_type {
                MouseEventType::ButtonDown if mouse_event.buttons.left => {
                    // `position` is local to this window — see
                    // `MouseEvent::position` in the event module and the
                    // way `route_mouse_event` translates it. Use that
                    // directly for the divider hit-test.
                    if self.local_point_on_divider(
                        mouse_event.position.x,
                        mouse_event.position.y,
                    ) {
                        self.pressed = true;
                        EventResult::Handled
                    } else {
                        EventResult::Propagate
                    }
                }
                MouseEventType::Move => {
                    if self.pressed {
                        self.move_divider_to(
                            mouse_event.position.x,
                            mouse_event.position.y,
                        );
                        EventResult::Handled
                    } else {
                        EventResult::Propagate
                    }
                }
                MouseEventType::ButtonUp => {
                    if self.pressed {
                        self.pressed = false;
                        EventResult::Handled
                    } else {
                        EventResult::Propagate
                    }
                }
                _ => EventResult::Propagate,
            },
            _ => EventResult::Propagate,
        }
    }
}

/// The total length along the divided axis for a given orientation.
fn main_axis_total(orientation: SplitterOrientation, bounds: Rect) -> u32 {
    match orientation {
        SplitterOrientation::Vertical => bounds.width,
        SplitterOrientation::Horizontal => bounds.height,
    }
}
