//! `IconView` — a Finder-style grid of icon+label tiles.
//!
//! Tiles flow left-to-right, wrapping to the next row when the next tile
//! would exceed the viewport width. Selection is delegated to the shared
//! [`Selection`](crate::window::selection::Selection) model, matching the
//! conventions established in `list.rs` and `tree_view.rs`. Keyboard arrow
//! navigation clamps at grid boundaries (no wrap-around): walking off the
//! end of the last row, the start of the first row, the top of the first
//! row, or the bottom of the last row leaves selection unchanged.
//!
//! Like `List`, `IconView` paints its full content rect; consumers wrap it
//! in a [`ScrollView`](crate::window::windows::scroll_view::ScrollView) to
//! scroll. The caller drives `ScrollView::set_content_size` from
//! `tiles_per_row` × `tile_w` and `content_height()`.
//!
//! Rubber-band (drag-to-multi-select) selection is **deferred** — out of
//! scope for v1. When the File Manager actually exposes a need for it, the
//! mouse-drag code path can add a band-rect compute that toggles
//! `Selection::Multi` membership for tiles overlapping the band.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::selection::{ClickMods, Selection, SelectionMode};
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::base::WindowBase;

/// Default tile width in pixels (square icon area + label area below).
pub const DEFAULT_TILE_WIDTH: u32 = 64;
/// Default tile height in pixels.
pub const DEFAULT_TILE_HEIGHT: u32 = 80;
/// Height (in px) reserved at the bottom of each tile for the label.
const LABEL_AREA_HEIGHT: u32 = 16;

/// Callback invoked when the user changes the selection.
pub type SelectionCallback = Box<dyn FnMut(&Selection) + Send>;

/// A single tile rendered by `IconView`.
///
/// `icon` is a placeholder for v1 — the icon-loading API is not in place
/// yet. When `Some`, the icon area is rendered as a colored square whose
/// color is derived from the bytes (so different placeholder icons appear
/// visually distinct). When `None`, the icon area is filled with gray.
pub struct Tile {
    pub label: String,
    pub icon: Option<Vec<u8>>,
}

/// Finder-style grid of icon+label tiles.
pub struct IconView {
    base: WindowBase,
    tiles: Vec<Tile>,
    tile_w: u32,
    tile_h: u32,
    selection: Selection,
    selection_mode: SelectionMode,
    on_select: Option<SelectionCallback>,
    bg_color: Color,
    text_color: Color,
    selected_bg_color: Color,
    selected_text_color: Color,
}

impl IconView {
    /// Create a new `IconView`.
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Create a new `IconView` with a specific window id.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        IconView {
            base: WindowBase::new_with_id(id, bounds),
            tiles: Vec::new(),
            tile_w: DEFAULT_TILE_WIDTH,
            tile_h: DEFAULT_TILE_HEIGHT,
            selection: Selection::None,
            selection_mode: SelectionMode::Single,
            on_select: None,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            text_color: crate::window::PALETTE_TEXT,
            selected_bg_color: crate::window::PALETTE_HIGHLIGHT_BG,
            selected_text_color: crate::window::PALETTE_HIGHLIGHT_TEXT,
        }
    }

    /// Set the tile size (`w` × `h`). Both dimensions must be positive; a
    /// zero in either is silently treated as `1` to avoid divide-by-zero.
    pub fn set_tile_size(&mut self, w: u32, h: u32) {
        let w = w.max(1);
        let h = h.max(1);
        if self.tile_w != w || self.tile_h != h {
            self.tile_w = w;
            self.tile_h = h;
            self.base.invalidate();
        }
    }

    /// Append a tile.
    pub fn add_tile(&mut self, label: &str, icon: Option<Vec<u8>>) {
        self.tiles.push(Tile {
            label: String::from(label),
            icon,
        });
        self.base.invalidate();
    }

    /// Drop all tiles and clear the selection.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn clear_tiles(&mut self) {
        self.tiles.clear();
        self.selection = Selection::None;
        self.base.invalidate();
    }

    /// Number of tiles.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// True when there are no tiles.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Borrow the underlying selection state.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Configure the selection mode. Switching from `Multi` back to `Single`
    /// collapses any existing multi-selection to its first index.
    pub fn set_selection_mode(&mut self, mode: SelectionMode) {
        if self.selection_mode == mode {
            return;
        }
        self.selection_mode = mode;
        if matches!(mode, SelectionMode::Single) {
            let first = self.selection.iter().next();
            self.selection = match first {
                Some(i) => Selection::Single(i),
                None => Selection::None,
            };
            self.base.invalidate();
        }
    }

    /// Current selection mode.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn selection_mode(&self) -> SelectionMode {
        self.selection_mode
    }

    /// Set the selection-change callback.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(&Selection) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    /// Number of tiles per row given the current viewport width and tile
    /// width. Always at least 1, even if the viewport is narrower than a
    /// single tile (vertical-only fallback).
    pub fn tiles_per_row(&self) -> usize {
        let viewport_w = self.base.bounds().width;
        let raw = (viewport_w / self.tile_w.max(1)) as usize;
        raw.max(1)
    }

    /// Natural content height in pixels. Used by callers that wrap the
    /// `IconView` in a `ScrollView` so the wrapper can compute the
    /// scrollbar geometry.
    pub fn content_height(&self) -> u32 {
        let tpr = self.tiles_per_row();
        let rows = (self.tiles.len() + tpr - 1) / tpr; // ceil
        rows as u32 * self.tile_h
    }

    /// Tile width (px).
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn tile_width(&self) -> u32 {
        self.tile_w
    }

    /// Tile height (px).
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn tile_height(&self) -> u32 {
        self.tile_h
    }

    /// Set the background color.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn set_bg_color(&mut self, color: Color) {
        self.bg_color = color;
        self.base.invalidate();
    }

    /// Set the label text color.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn set_text_color(&mut self, color: Color) {
        self.text_color = color;
        self.base.invalidate();
    }

    /// Set the background color for selected tiles.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn set_selected_bg_color(&mut self, color: Color) {
        self.selected_bg_color = color;
        self.base.invalidate();
    }

    /// Set the label color for selected tiles.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn set_selected_text_color(&mut self, color: Color) {
        self.selected_text_color = color;
        self.base.invalidate();
    }

    /// Convert a position (in the icon view's local coordinate frame, the
    /// same frame as `MouseEvent::position`) to a tile index, if any.
    fn position_to_index(&self, x: i32, y: i32) -> Option<usize> {
        let bounds = self.base.bounds();
        let rel_x = x - bounds.x;
        let rel_y = y - bounds.y;
        if rel_x < 0 || rel_y < 0 {
            return None;
        }
        if self.tile_w == 0 || self.tile_h == 0 {
            return None;
        }
        let tpr = self.tiles_per_row();
        let col = (rel_x as u32) / self.tile_w;
        let row = (rel_y as u32) / self.tile_h;
        // The "column" derived from x must still be within the row to count
        // as a hit — clicks past the right edge of the last column don't
        // select.
        if (col as usize) >= tpr {
            return None;
        }
        let idx = (row as usize) * tpr + (col as usize);
        if idx < self.tiles.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Compute the target index for an arrow-key press, applying the
    /// clamp-at-edges rules described in the unit's plan. Returns `None`
    /// when the press should be ignored (no tiles, or the press would walk
    /// off the grid edge).
    fn arrow_target(&self, key: ArrowKey) -> Option<usize> {
        if self.tiles.is_empty() {
            return None;
        }
        let n = self.tiles.len();
        let tpr = self.tiles_per_row();
        let current = current_focus(&self.selection).unwrap_or(0);
        let row = current / tpr;
        let col = current % tpr;
        let last_idx = n - 1;
        let last_row = last_idx / tpr;

        match key {
            ArrowKey::Right => {
                if current >= last_idx {
                    None
                } else if col == tpr - 1 {
                    // End of a non-last row: go to first tile of next row.
                    let target = (row + 1) * tpr;
                    if target < n {
                        Some(target)
                    } else {
                        None
                    }
                } else {
                    Some(current + 1)
                }
            }
            ArrowKey::Left => {
                if current == 0 {
                    None
                } else {
                    // Whether we're at the start of a non-first row or
                    // anywhere else, idx - 1 is the right answer (because
                    // the last column of the previous row sits at
                    // (row * tpr) - 1).
                    Some(current - 1)
                }
            }
            ArrowKey::Down => {
                if row == last_row {
                    None
                } else {
                    let target = current + tpr;
                    Some(target.min(last_idx))
                }
            }
            ArrowKey::Up => {
                if row == 0 {
                    None
                } else {
                    Some(current - tpr)
                }
            }
        }
    }

    /// Apply a click to the selection model and fire the callback.
    fn apply_click(&mut self, idx: usize, mods: ClickMods) {
        let before = self.selection.clone();
        self.selection.click(idx, mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
        }
        if let Some(ref mut callback) = self.on_select {
            callback(&self.selection);
        }
    }

    /// Apply an arrow-key navigation step, including shift+arrow extension.
    fn apply_arrow(&mut self, key: ArrowKey, mods: ClickMods) {
        let Some(target) = self.arrow_target(key) else {
            return;
        };
        let before = self.selection.clone();
        // Reuse Selection::click so shift+arrow extends a range and plain
        // arrow collapses to Single — matching how `List` already routes
        // arrow-key navigation through the selection model.
        self.selection.click(target, mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
            if let Some(ref mut callback) = self.on_select {
                callback(&self.selection);
            }
        }
    }
}

/// Internal arrow-key direction enum used by `arrow_target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrowKey {
    Up,
    Down,
    Left,
    Right,
}

/// Mirror of `Selection::current_focus` (which is private). Returns the
/// "moving cursor" index — the one arrow keys advance. For `Range`, this
/// is `end`; for `Multi`, the largest index; for `Single`, the index;
/// for `None`, `None`. We compute it here rather than going through
/// `Selection::iter()` because `Range { anchor: 5, end: 2 }` iterates
/// ascending and would lose track of the actual moving cursor.
fn current_focus(selection: &Selection) -> Option<usize> {
    match selection {
        Selection::None => None,
        Selection::Single(idx) => Some(*idx),
        Selection::Multi(set) => set.iter().next_back().copied(),
        Selection::Range { end, .. } => Some(*end),
    }
}

/// Derive a stable placeholder color from a label (for icon bytes that are
/// `None`) or from icon bytes. The hash is intentionally cheap — this is a
/// v1 placeholder until a real icon-loading API lands.
fn placeholder_color(label: &str, icon: Option<&[u8]>) -> Color {
    let bytes: &[u8] = match icon {
        Some(b) if !b.is_empty() => b,
        _ => label.as_bytes(),
    };
    if bytes.is_empty() {
        return Color::GRAY;
    }
    // FNV-1a 32-bit
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // Spread bits across the channels and bias away from very-dark colors
    // so the placeholder square stays visible against a white background.
    let r = ((hash & 0xFF) as u8) | 0x40;
    let g = (((hash >> 8) & 0xFF) as u8) | 0x40;
    let b = (((hash >> 16) & 0xFF) as u8) | 0x40;
    Color::new(r, g, b)
}

impl Window for IconView {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let tpr = self.tiles_per_row();
        let tile_w = self.tile_w;
        let tile_h = self.tile_h;

        // Background fill for the entire content rect (when wrapped in a
        // ScrollView the bounds are temporarily extended to cover the
        // content area).
        device.fill_rect(bounds.x, bounds.y, bounds.width, bounds.height, self.bg_color);

        if self.tiles.is_empty() {
            self.base.clear_needs_repaint();
            return;
        }

        let font = get_default_font();
        let line_h = font.line_height();
        let cell_w = font.cell_width().max(1);

        // The icon area sits above a `LABEL_AREA_HEIGHT`-tall label strip.
        // Clamp icon_h so very-short tiles still render something.
        let label_h = LABEL_AREA_HEIGHT.min(tile_h);
        let icon_h = tile_h.saturating_sub(label_h);

        for (idx, tile) in self.tiles.iter().enumerate() {
            let row = (idx / tpr) as i32;
            let col = (idx % tpr) as i32;
            let tile_x = bounds.x + col * tile_w as i32;
            let tile_y = bounds.y + row * tile_h as i32;

            let is_selected = self.selection.is_selected(idx);

            // Selection background covers the whole tile.
            if is_selected {
                device.fill_rect(tile_x, tile_y, tile_w, tile_h, self.selected_bg_color);
            }

            // Icon area: a small inset so the icon doesn't butt up against
            // neighboring tiles. For v1 the "icon" is a colored square.
            let inset: u32 = 4;
            let icon_box_w = tile_w.saturating_sub(inset * 2);
            let icon_box_h = icon_h.saturating_sub(inset * 2);
            if icon_box_w > 0 && icon_box_h > 0 {
                let icon_color = placeholder_color(&tile.label, tile.icon.as_deref());
                device.fill_rect(
                    tile_x + inset as i32,
                    tile_y + inset as i32,
                    icon_box_w,
                    icon_box_h,
                    icon_color,
                );
            }

            // Label: truncate to fit within `tile_w` (subtract a small
            // padding on each side). Truncation is byte-wise but only at
                // ASCII boundaries — the system font is ASCII-only, so this
            // keeps the implementation simple without breaking glyph
            // rendering.
            let pad: u32 = 2;
            let label_max_chars = (tile_w.saturating_sub(pad * 2) / cell_w) as usize;
            let label_text = if tile.label.len() > label_max_chars {
                // Find the largest ASCII-byte prefix that fits. (System
                // font is ASCII-only; non-ASCII bytes simply won't render.)
                let cut = label_max_chars.min(tile.label.len());
                &tile.label.as_str()[..cut]
            } else {
                tile.label.as_str()
            };

            // Center label horizontally inside the tile.
            let label_pixel_w = (label_text.len() as u32) * cell_w;
            let label_x = tile_x + ((tile_w as i32 - label_pixel_w as i32) / 2).max(0);
            // Vertically center the label inside the label strip.
            let label_y = tile_y + icon_h as i32 + ((label_h as i32 - line_h as i32) / 2).max(0);

            let text_color = if is_selected {
                self.selected_text_color
            } else {
                self.text_color
            };

            device.draw_text(label_x, label_y, label_text, font.as_font(), text_color);
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let bounds = self.base.bounds();
                if !bounds.contains_point(mouse_event.position) {
                    return EventResult::Ignored;
                }

                match mouse_event.event_type {
                    MouseEventType::ButtonDown if mouse_event.buttons.left => {
                        if let Some(idx) =
                            self.position_to_index(mouse_event.position.x, mouse_event.position.y)
                        {
                            let mods = ClickMods::new(
                                mouse_event.modifiers.shift,
                                mouse_event.modifiers.ctrl,
                            );
                            self.apply_click(idx, mods);
                        }
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                use crate::window::event::KeyCode;
                let mods = ClickMods::new(kbd_event.modifiers.shift, kbd_event.modifiers.ctrl);
                match kbd_event.key_code {
                    KeyCode::Up => {
                        self.apply_arrow(ArrowKey::Up, mods);
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        self.apply_arrow(ArrowKey::Down, mods);
                        EventResult::Handled
                    }
                    KeyCode::Left => {
                        self.apply_arrow(ArrowKey::Left, mods);
                        EventResult::Handled
                    }
                    KeyCode::Right => {
                        self.apply_arrow(ArrowKey::Right, mods);
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
