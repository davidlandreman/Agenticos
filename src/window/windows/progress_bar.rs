//! Determinate progress bar widget (U14).
//!
//! Renders a 1-pixel-bordered rectangle whose interior is filled
//! proportionally to `current / total`. An optional label is drawn
//! centered (vertically and horizontally) over the bar — typical
//! content is something like "47% — Copying file_42.txt".
//!
//! `ProgressBar` is non-interactive: `handle_event` ignores every
//! event. Composition is caller-driven — embed it as a child of a
//! dialog or `StatusBar` by adding it through the relevant container
//! API. There is no special embedding hook here.
//!
//! Edge cases handled silently (no panics):
//! - `total == 0`: treated as 0% (no fill). `fraction()` returns 0.0.
//! - `current > total`: clamped to 100%.
//! - `bounds.width <= 2` or `bounds.height <= 2`: no inner fill drawn;
//!   the (degenerate) border is still attempted via `draw_rect` which
//!   itself clips silently.

use alloc::string::String;

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::base::WindowBase;

/// Determinate progress widget with current/total values and an optional
/// centered label overlay.
pub struct ProgressBar {
    /// Base window functionality.
    base: WindowBase,
    /// Current progress value.
    current: u64,
    /// Total / maximum progress value. `0` means "indeterminate amount of
    /// work" and is rendered as an empty bar (no panic).
    total: u64,
    /// Optional centered label drawn on top of the filled/empty regions.
    label: Option<String>,
    /// Background color of the empty portion (also painted behind the
    /// border, so a 1px border has no see-through fringe).
    bg_color: Color,
    /// Color of the filled portion.
    fill_color: Color,
    /// Color of the 1-pixel outline.
    border_color: Color,
    /// Color of the optional centered label.
    text_color: Color,
}

impl ProgressBar {
    /// Create a new `ProgressBar` with a specific window ID.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        ProgressBar {
            base: WindowBase::new_with_id(id, bounds),
            current: 0,
            total: 0,
            label: None,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            fill_color: crate::window::PALETTE_PROGRESS_FILL,
            border_color: crate::window::PALETTE_BORDER,
            text_color: crate::window::PALETTE_TEXT,
        }
    }

    /// Create a new `ProgressBar`, generating its own window ID.
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Set the progress as a `current / total` pair.
    ///
    /// `total == 0` is accepted (treated as 0% filled) — never panics.
    /// `current > total` is also accepted; the rendered fill clamps to
    /// the full inner width.
    pub fn set_progress(&mut self, current: u64, total: u64) {
        if self.current != current || self.total != total {
            self.current = current;
            self.total = total;
            self.base.invalidate();
        }
    }

    /// Set or clear the optional centered label. Pass `None` to remove.
    pub fn set_label(&mut self, label: Option<String>) {
        if self.label != label {
            self.label = label;
            self.base.invalidate();
        }
    }

    /// Current progress value.
    pub fn current(&self) -> u64 {
        self.current
    }

    /// Total progress value.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Progress as a fraction in `[0.0, 1.0]`. Returns `0.0` when
    /// `total == 0`. Values where `current > total` are clamped to
    /// `1.0`.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        if self.current >= self.total {
            return 1.0;
        }
        // `total > 0` and `current < total`, both fit in u64. The cast
        // to f32 may lose precision for extreme values, but the
        // resulting fraction is still bounded in [0, 1).
        (self.current as f64 / self.total as f64) as f32
    }
}

impl Window for ProgressBar {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let height = bounds.height;

        // 1. Background fills the whole rect (so the area behind the
        //    border is also a known color and the empty portion is
        //    `bg_color`).
        device.fill_rect(x, y, width, height, self.bg_color);

        // 2. Filled inner rect, sized proportionally. We carve out a
        //    1-pixel border on each side, so inner_width = width - 2.
        if width > 2 && height > 2 {
            let inner_width: u32 = width - 2;
            let inner_height: u32 = height - 2;
            let filled: u32 = if self.total == 0 {
                0
            } else if self.current >= self.total {
                inner_width
            } else {
                // Promote to u128 so multiplication never overflows
                // even for u64::MAX inputs.
                let f = (self.current as u128 * inner_width as u128 / self.total as u128) as u64;
                // Defensive clamp; the math above already keeps
                // `f <= inner_width` whenever `current < total`.
                if f > inner_width as u64 {
                    inner_width
                } else {
                    f as u32
                }
            };
            if filled > 0 {
                device.fill_rect(x + 1, y + 1, filled, inner_height, self.fill_color);
            }
        }

        // 3. 1-pixel outline around the whole rect.
        device.draw_rect(x, y, width, height, self.border_color);

        // 4. Centered label, if any. Drawn last so it sits on top of
        //    both the filled and empty regions.
        if let Some(label) = self.label.as_ref() {
            if !label.is_empty() {
                let font = get_default_font();
                let line_h = font.line_height();
                // Approximate text width. The system font is monospaced
                // (cell_width is the per-character advance), so this is
                // exact for ASCII; for non-ASCII it's a close-enough
                // estimate for centering and never panics.
                let char_count = label.chars().count() as u32;
                let text_width = char_count.saturating_mul(font.cell_width());

                let text_x = if text_width < width {
                    x + ((width - text_width) / 2) as i32
                } else {
                    x
                };
                let text_y = if line_h < height {
                    y + ((height - line_h) / 2) as i32
                } else {
                    y
                };

                device.draw_text(text_x, text_y, label, font.as_font(), self.text_color);
            }
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        // Progress bars are non-interactive.
        EventResult::Ignored
    }
}
