#![allow(dead_code)]
//! `StatusBar` — bottom-anchored horizontal strip of `Label` sections.
//!
//! `StatusBar` is a thin composition over [`HBox`]. Sections are
//! `Label` children with weighted widths so callers can mix fixed
//! and proportional sections (e.g. a wide left section + a narrow
//! right one for a row count).
//!
//! Typical use:
//! ```ignore
//! let mut sb = StatusBar::new(Rect::new(0, 580, 800, 20));
//! let path  = sb.add_section("/", 3);
//! let count = sb.add_section("0 items", 1);
//! sb.set_section_text(count, "42 items");
//! ```

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::window::manager::{with_active_manager, WindowManager};
use crate::window::windows::base::WindowBase;
use crate::window::windows::label::Label;
use crate::window::windows::layout::{HBox, SizeHint};
use crate::window::{
    with_window_manager, Event, EventResult, GraphicsDevice, Rect, Window, WindowId,
};

/// Run `f` against the live window manager regardless of whether the
/// caller is already inside a `with_window_manager` / `with_window_mut`
/// scope. Tries the active-manager pointer first (no lock), then falls
/// back to acquiring the global manager lock.
fn with_any_manager<F>(f: F) -> Option<()>
where
    F: FnOnce(&mut WindowManager),
{
    let mut slot = Some(f);
    if let Some(()) = with_active_manager(|wm| (slot.take().unwrap())(wm)) {
        return Some(());
    }
    with_window_manager(|wm| (slot.take().unwrap())(wm))
}

/// Default section background color — shared content background from
/// the U15 palette so a default status bar sits flush against a default
/// container or toolbar.
const DEFAULT_BG: Color = crate::window::PALETTE_CONTENT_BG;

/// Default text color — shared with the U15 palette.
const DEFAULT_FG: Color = crate::window::PALETTE_TEXT;

/// Bottom-anchored horizontal strip of `Label` sections.
pub struct StatusBar {
    hbox: HBox,
    /// Section ids in display order (also tracked in
    /// `hbox.base().children()`).
    sections: Vec<WindowId>,
    bg_color: Color,
    text_color: Color,
}

impl StatusBar {
    /// Create a new `StatusBar` covering `bounds`.

    /// Create a new `StatusBar` with a specific `WindowId`.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        StatusBar {
            hbox: HBox::new_with_id(id, bounds),
            sections: Vec::new(),
            bg_color: DEFAULT_BG,
            text_color: DEFAULT_FG,
        }
    }

    /// Set the strip background color.

    /// Set the default text color used by future sections. Existing
    /// sections retain whatever color they were constructed with.

    /// Append a section with `text` and the given `Fill` weight. Returns
    /// the section's `WindowId` so callers can later update its text via
    /// `set_section_text`.
    ///
    /// Must be called when the global window manager is reachable
    /// (i.e. after `init_window_system`); the label is registered into
    /// the manager and laid out inside the status bar's `HBox`.
    pub fn add_section(&mut self, text: &str, weight: u32) -> WindowId {
        let parent_id = self.hbox.base().id();
        let strip_height = self.hbox.base().bounds().height;
        // Initial bounds are placeholders; the inner HBox overwrites
        // them in its relayout pass.
        let mut label = Label::new(Rect::new(0, 0, 0, strip_height), text);
        label.set_color(self.text_color);
        label.set_background(Some(self.bg_color));
        label.base_mut().set_parent(Some(parent_id));
        let label_id = label.base().id();

        with_any_manager(|wm| {
            wm.set_window_impl(label_id, Box::new(label));
        });

        // Treat zero weights as `Fill(1)` rather than `Fill(0)` so a
        // single zero-weight section still receives the full width
        // (otherwise distribution math gives it zero pixels).
        let hint = if weight == 0 {
            SizeHint::Fill(1)
        } else {
            SizeHint::Fill(weight)
        };
        self.hbox.add_child(label_id, hint);
        self.sections.push(label_id);
        self.hbox.base_mut().invalidate();
        label_id
    }

    /// Update the text of a previously-added section. Silently no-ops
    /// when `label_id` is not a known section.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn set_section_text(&mut self, label_id: WindowId, text: &str) {
        if !self.sections.contains(&label_id) {
            return;
        }
        with_any_manager(|wm| {
            wm.with_window_mut(label_id, |w| {
                if let Some(label) = w.as_label_mut() {
                    label.set_text(text);
                }
            });
        });
    }
}

impl Window for StatusBar {
    fn base(&self) -> &WindowBase {
        self.hbox.base()
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        self.hbox.base_mut()
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.hbox.set_bounds(bounds);
    }

    fn add_child(&mut self, child: WindowId) {
        // Trait-level fallback — defer to the HBox's default Fill(1).
        self.hbox.add_child(child, SizeHint::Fill(1));
    }

    fn remove_child(&mut self, child: WindowId) {
        self.hbox.remove_child(child);
        self.sections.retain(|id| *id != child);
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.hbox.base().visible() {
            return;
        }
        if !self.hbox.base().needs_repaint() {
            return;
        }
        let bounds = self.hbox.base().bounds();
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            self.bg_color,
        );
        self.hbox.base_mut().clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Propagate
    }
}
