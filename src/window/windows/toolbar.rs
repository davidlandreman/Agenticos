#![allow(dead_code)]
//! `Toolbar` — horizontal strip of command buttons.
//!
//! `Toolbar` is a thin composition over [`HBox`]. It owns an internal
//! `HBox` that shares its `WindowId`, so children added through the
//! toolbar appear as children of the toolbar itself from the window
//! manager's perspective. The toolbar's `paint` fills the strip
//! background and draws 1-pixel vertical separators inside any
//! configured separator slots; the buttons themselves paint via the
//! manager's normal child-paint flow.
//!
//! Typical use:
//! ```ignore
//! let mut tb = Toolbar::new(Rect::new(0, 0, 400, 32));
//! let back = tb.add_button("Back", || { /* … */ });
//! let fwd  = tb.add_button("Forward", || { /* … */ });
//! tb.add_separator();
//! let up   = tb.add_button("Up", || { /* … */ });
//! tb.set_enabled(back, false);
//! ```

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::window::manager::{with_active_manager, WindowManager};
use crate::window::windows::base::WindowBase;
use crate::window::windows::button::Button;
use crate::window::windows::layout::{HBox, SizeHint, Spacer};
use crate::window::{
    with_window_manager, Event, EventResult, GraphicsDevice, Rect, Window, WindowId,
};

/// Run `f` against the live window manager regardless of whether the
/// caller is already inside a `with_window_manager` / `with_window_mut`
/// scope. Tries the active-manager pointer first (no lock), then falls
/// back to acquiring the global manager lock. Returns `None` only when
/// neither path is reachable (e.g. before `init_window_manager`).
fn with_any_manager<F>(f: F) -> Option<()>
where
    F: FnOnce(&mut WindowManager),
{
    // Use a single-use Option so we can move the FnOnce into whichever
    // branch fires.
    let mut slot = Some(f);
    if let Some(()) = with_active_manager(|wm| (slot.take().unwrap())(wm)) {
        return Some(());
    }
    with_window_manager(|wm| (slot.take().unwrap())(wm))
}

/// Default button height inside a `Toolbar`.
const BUTTON_HEIGHT: u32 = 24;

/// Minimum button width — short labels still get a clickable area.
const MIN_BUTTON_WIDTH: u32 = 48;

/// Per-character padding added to the label's measured width when sizing
/// a button on the toolbar.
const BUTTON_HORIZONTAL_PADDING: u32 = 16;

/// Width of a separator slot (a thin gap with a 1-pixel rule).
const SEPARATOR_WIDTH: u32 = 8;

/// Color used for vertical separator rules.
const SEPARATOR_COLOR: Color = Color {
    red: 160,
    green: 160,
    blue: 160,
};

/// Identifies a slot in the toolbar's child list — either a button or
/// a separator. We track this on the toolbar (in addition to the inner
/// `HBox`'s child list) so `paint` knows where to draw separator rules.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Button(WindowId),
    Separator(WindowId),
}

/// Horizontal strip of command buttons.
pub struct Toolbar {
    /// Internal layout container. Shares the toolbar's `WindowId` so
    /// children added here appear as children of the toolbar in the
    /// window manager's tree.
    hbox: HBox,
    /// Per-slot kind, in the same order as `hbox.base().children()`.
    slots: Vec<Slot>,
}

impl Toolbar {
    /// Create a new `Toolbar` covering `bounds` with a generated id.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Create a new `Toolbar` with a specific `WindowId`.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        Toolbar {
            hbox: HBox::new_with_id(id, bounds),
            slots: Vec::new(),
        }
    }

    /// Compute the pixel width a button needs in order to comfortably
    /// hold `label`. Uses the system font's cell width; pads by
    /// `BUTTON_HORIZONTAL_PADDING` and clamps to `MIN_BUTTON_WIDTH`.
    fn button_width_for(label: &str) -> u32 {
        use crate::graphics::fonts::core_font::get_default_font;
        let font = get_default_font();
        let text_w = (label.chars().count() as u32) * font.cell_width();
        text_w
            .saturating_add(BUTTON_HORIZONTAL_PADDING)
            .max(MIN_BUTTON_WIDTH)
    }

    /// Append a button with `label` and the given click callback.
    /// Returns the new button's `WindowId` so callers can later toggle
    /// its enabled state through `Toolbar::set_enabled`.
    ///
    /// Must be called when the global window manager is reachable
    /// (i.e. after `init_window_system`); the button is registered into
    /// the window manager and laid out inside the toolbar's `HBox`.
    pub fn add_button<F>(&mut self, label: &str, on_click: F) -> WindowId
    where
        F: FnMut() + Send + 'static,
    {
        let toolbar_id = self.hbox.base().id();
        let strip_height = self.hbox.base().bounds().height;
        // Vertical-center the button inside the strip; collapse to the
        // strip height when the strip is shorter than BUTTON_HEIGHT.
        let btn_h = BUTTON_HEIGHT.min(strip_height);
        let btn_y = ((strip_height.saturating_sub(btn_h)) / 2) as i32;
        let btn_w = Self::button_width_for(label);

        let mut button = Button::new(Rect::new(0, btn_y, btn_w, btn_h), label);
        button.on_click(on_click);
        button.base_mut().set_parent(Some(toolbar_id));
        let button_id = button.base().id();

        // Add to the inner HBox first so the slot list and layout state
        // stay consistent. While the toolbar is not yet registered with
        // the window manager, `HBox::add_child`'s relayout call is a
        // no-op for child bounds (it short-circuits when there is no
        // active manager) — we apply the centered slot bounds below.
        self.hbox.add_child(button_id, SizeHint::Fixed(btn_w));
        self.slots.push(Slot::Button(button_id));

        with_any_manager(|wm| {
            wm.set_window_impl(button_id, Box::new(button));
        });

        // Push the freshly-computed slot bounds into the manager.
        self.apply_slot_bounds();

        button_id
    }

    /// Append a separator (a small empty slot with a 1-pixel vertical
    /// rule painted by the toolbar).
    pub fn add_separator(&mut self) {
        let toolbar_id = self.hbox.base().id();
        let strip_height = self.hbox.base().bounds().height;

        let mut spacer = Spacer::new(Rect::new(0, 0, SEPARATOR_WIDTH, strip_height));
        spacer.base_mut().set_parent(Some(toolbar_id));
        let spacer_id = spacer.base().id();

        self.hbox
            .add_child(spacer_id, SizeHint::Fixed(SEPARATOR_WIDTH));
        self.slots.push(Slot::Separator(spacer_id));

        with_any_manager(|wm| {
            wm.set_window_impl(spacer_id, Box::new(spacer));
        });

        self.apply_slot_bounds();
    }

    /// Recompute the inner `HBox` layout and write each child's bounds
    /// back through the window manager. Buttons get a vertically-
    /// centered rect of `BUTTON_HEIGHT`; separators fill the strip
    /// height. Silently no-ops when the global manager is not yet
    /// initialized.
    fn apply_slot_bounds(&self) {
        let layouts = self.hbox.compute_child_bounds();
        let strip_height = self.hbox.base().bounds().height;
        let btn_h = BUTTON_HEIGHT.min(strip_height);
        let btn_y = ((strip_height.saturating_sub(btn_h)) / 2) as i32;

        // Pair each slot with its computed bounds and apply.
        let updates: Vec<(WindowId, Rect)> = self
            .slots
            .iter()
            .zip(layouts.iter())
            .map(|(slot, slot_rect)| match slot {
                Slot::Button(id) => (*id, Rect::new(slot_rect.x, btn_y, slot_rect.width, btn_h)),
                Slot::Separator(id) => (*id, *slot_rect),
            })
            .collect();
        with_any_manager(|wm| {
            for (id, rect) in updates {
                wm.with_window_mut(id, |w| w.set_bounds(rect));
            }
        });
    }

    /// Toggle the enabled state of a button previously added via
    /// `add_button`. Silently no-ops when `button_id` is not a known
    /// toolbar button.
    pub fn set_enabled(&mut self, button_id: WindowId, enabled: bool) {
        // Validate the id refers to one of our buttons before touching
        // the manager — avoids reaching into arbitrary windows.
        let is_known = self
            .slots
            .iter()
            .any(|s| matches!(s, Slot::Button(id) if *id == button_id));
        if !is_known {
            return;
        }
        with_any_manager(|wm| {
            wm.with_window_mut(button_id, |w| {
                if let Some(btn) = w.as_button_mut() {
                    btn.set_enabled(enabled);
                }
            });
        });
    }
}

impl Window for Toolbar {
    fn base(&self) -> &WindowBase {
        self.hbox.base()
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        self.hbox.base_mut()
    }

    fn set_bounds(&mut self, bounds: Rect) {
        // Update bounds without going through HBox::set_bounds so the
        // HBox's own relayout (which stretches children to the strip's
        // full height) doesn't fight our vertical-centered button rule;
        // `apply_slot_bounds` writes the per-slot rects (centered for
        // buttons, full-height for separators) through whichever
        // manager handle is reachable.
        self.hbox.base_mut().set_bounds(bounds);
        self.apply_slot_bounds();
    }

    fn add_child(&mut self, child: WindowId) {
        // Trait-level fallback: defer to HBox's default Fill(1) hint.
        // The Toolbar API prefers add_button / add_separator, but the
        // trait method is exercised by the manager when wiring parent
        // relationships — keep it benign.
        self.hbox.add_child(child, SizeHint::Fill(1));
    }

    fn remove_child(&mut self, child: WindowId) {
        self.hbox.remove_child(child);
        self.slots.retain(|s| match s {
            Slot::Button(id) | Slot::Separator(id) => *id != child,
        });
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.hbox.base().visible() {
            return;
        }
        if !self.hbox.base().needs_repaint() {
            return;
        }

        let bounds = self.hbox.base().bounds();
        // Themed strip background (flat face in Classic, soft gradient in
        // Aero) so the toolbar reads as window chrome, not content.
        crate::window::theme::controls::draw_raised_panel(device, bounds);

        // Draw a 1-pixel vertical rule centered inside each separator
        // slot. The HBox's distribution math is a pure function over its
        // own bounds + size hints, so we can recompute slot rects here
        // without going through the window manager.
        let layouts = self.hbox.compute_child_bounds();
        for (slot, local) in self.slots.iter().zip(layouts.iter()) {
            if let Slot::Separator(_) = slot {
                let cx = bounds.x + local.x + (local.width as i32) / 2;
                let top = bounds.y + local.y + 2;
                let bot = bounds.y + local.y + local.height as i32 - 3;
                if bot >= top {
                    device.draw_line(cx, top, cx, bot, SEPARATOR_COLOR);
                }
            }
        }

        self.hbox.base_mut().clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        // Buttons receive their own events through the manager's normal
        // child dispatch; the toolbar itself does not handle events.
        EventResult::Propagate
    }
}
