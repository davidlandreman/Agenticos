#![allow(dead_code)]
//! Breadcrumb path-bar widget
//!
//! Displays a path like "/host/Documents/Projects" as a horizontal strip of
//! clickable segments separated by `>` glyphs. Each segment is its own
//! hit zone; clicking a segment fires `on_segment_click(truncated_path)`
//! where the truncated path runs from the path root through (and
//! including) the clicked segment.
//!
//! When the available width is too small to fit all segments, leading
//! segments are dropped and a "..." indicator is painted at the start of
//! the bar. The "..." token is **inert** — it never enters the hover
//! state and never fires the click callback. (A future overflow-menu
//! reveal could be hung off this token; not in this version.)
//!
//! Patterned on `menu_bar.rs` for "horizontal segments with hit-testing
//! and hover".

use alloc::{boxed::Box, string::String, vec::Vec};

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::base::WindowBase;

/// Horizontal padding inside each segment's hit zone, in pixels.
const SEGMENT_PADDING: u32 = 8;

/// Total width of the `>` separator drawn between adjacent segments.
const SEPARATOR_WIDTH: u32 = 12;

/// Width reserved for the "..." overflow indicator (including the
/// trailing separator that follows it before the first visible segment).
const OVERFLOW_INDICATOR_WIDTH: u32 = 24;

/// The literal token painted when leading segments are collapsed.
const OVERFLOW_TOKEN: &str = "...";

/// Callback invoked when a segment is clicked. The argument is the
/// truncated path from root through (and including) the clicked
/// segment.
pub type SegmentClickCallback = Box<dyn FnMut(&str) + Send>;

/// Layout entry computed by `relayout` for each visible segment.
#[derive(Debug, Clone, Copy)]
struct SegmentLayout {
    /// Index into `segments`.
    seg_index: usize,
    /// Hit-zone left edge, in PathBar-local coordinates.
    x: i32,
    /// Hit-zone width (text width plus 2 * SEGMENT_PADDING).
    width: u32,
}

/// Clickable breadcrumb path bar.
pub struct PathBar {
    base: WindowBase,
    /// Original path as last set via `set_path`.
    path: String,
    /// Whether the original path started with `/` (so we can rebuild
    /// truncated paths with the correct leading `/`).
    is_absolute: bool,
    /// Segments after splitting on `/` and dropping empty pieces.
    segments: Vec<String>,
    /// Layout of segments that fit, leftmost first. If
    /// `visible_layout.len() < segments.len()`, the leading segments
    /// were dropped and "..." should be painted at the start of the bar.
    visible_layout: Vec<SegmentLayout>,
    /// Index into `visible_layout` for the currently hovered segment,
    /// or `None`. The "..." token never enters this state.
    hover_index: Option<usize>,
    /// Click callback.
    on_segment_click: Option<SegmentClickCallback>,
    bg_color: Color,
    text_color: Color,
    hover_bg_color: Color,
    separator_color: Color,
}

impl PathBar {
    /// Create a new PathBar with a specific window id. Colors default to the
    /// active theme's content palette.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        let palette = crate::window::theme::controls::palette();
        PathBar {
            base: WindowBase::new_with_id(id, bounds),
            path: String::new(),
            is_absolute: false,
            segments: Vec::new(),
            visible_layout: Vec::new(),
            hover_index: None,
            on_segment_click: None,
            bg_color: palette.content_bg,
            text_color: palette.text,
            // Subtle grey hover (kept distinct from the selection color)
            // — breadcrumb hover shouldn't read as a selection state.
            hover_bg_color: Color::new(200, 200, 200),
            separator_color: palette.border,
        }
    }

    /// Create a new PathBar (generates its own id).
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Set the path. Splits on `/`, drops empty segments (so a trailing
    /// slash is harmless), and recomputes layout.
    ///
    /// The special case `"/"` produces a single segment whose label is
    /// `/`; clicking it yields `/`.
    pub fn set_path(&mut self, path: &str) {
        self.path = String::from(path);
        self.is_absolute = path.starts_with('/');
        self.segments.clear();

        if path == "/" {
            self.segments.push(String::from("/"));
        } else {
            for piece in path.split('/') {
                if !piece.is_empty() {
                    self.segments.push(String::from(piece));
                }
            }
        }

        self.hover_index = None;
        self.relayout();
        self.base.invalidate();
    }

    /// Returns the path most recently set, byte-for-byte.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Set the click callback. Fires with the truncated path through
    /// the clicked segment.
    pub fn on_segment_click<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.on_segment_click = Some(Box::new(callback));
    }

    /// Width in pixels of a segment's hit zone (text + horizontal
    /// padding on both sides).
    fn segment_hit_width(&self, seg: &str) -> u32 {
        let font = get_default_font();
        let char_width = font.cell_width();
        (seg.chars().count() as u32) * char_width + 2 * SEGMENT_PADDING
    }

    /// Recompute `visible_layout` from `segments` and current bounds.
    ///
    /// Walks segments right-to-left accumulating widths; stops when the
    /// next segment + its preceding separator would overflow. If any
    /// segments were dropped, reserves space at the left for the
    /// `OVERFLOW_TOKEN` and shifts visible segments rightward by that
    /// amount.
    fn relayout(&mut self) {
        self.visible_layout.clear();
        if self.segments.is_empty() {
            return;
        }

        let total_width = self.base.bounds().width;
        if total_width == 0 {
            return;
        }

        // Right-to-left fitting pass. We track:
        //   - `widths`: hit-zone widths of segments that fit, in
        //     right-to-left order.
        //   - `consumed`: total horizontal pixels they take when
        //     painted with separators between them. Separators sit
        //     BETWEEN segments, so N visible segments have N-1
        //     separators.
        let mut widths_rtl: Vec<u32> = Vec::new();
        let mut consumed: u32 = 0;

        for seg in self.segments.iter().rev() {
            let w = self.segment_hit_width(seg);
            // Cost of adding this segment: its width, plus a separator
            // if it isn't the first one we've added.
            let extra = if widths_rtl.is_empty() {
                w
            } else {
                w + SEPARATOR_WIDTH
            };
            // If after adding this segment we'd still need room for an
            // overflow indicator (because there are MORE segments to
            // its left that we'd be hiding), we need to leave space
            // for `OVERFLOW_INDICATOR_WIDTH` too.
            let segments_left_after_adding = self.segments.len() - 1 - widths_rtl.len();
            let reserve = if segments_left_after_adding > 0 {
                OVERFLOW_INDICATOR_WIDTH
            } else {
                0
            };
            if consumed + extra + reserve > total_width {
                break;
            }
            consumed += extra;
            widths_rtl.push(w);
        }

        // If even the rightmost segment doesn't fit on its own, force
        // a single-segment fit with no overflow reservation; better to
        // show *something* than nothing. (Edge case for narrow bars.)
        if widths_rtl.is_empty() {
            let last_idx = self.segments.len() - 1;
            let w = self.segment_hit_width(&self.segments[last_idx]);
            // Clamp to total_width so painting stays in bounds.
            let clamped = w.min(total_width);
            widths_rtl.push(clamped);
        }

        let visible_count = widths_rtl.len();
        let dropped = self.segments.len() - visible_count;

        // Place visible segments left-to-right.
        let mut x: i32 = if dropped > 0 {
            OVERFLOW_INDICATOR_WIDTH as i32
        } else {
            0
        };
        // widths_rtl is right-to-left; reverse to walk in display order.
        widths_rtl.reverse();
        for (i, w) in widths_rtl.iter().enumerate() {
            let seg_index = self.segments.len() - visible_count + i;
            self.visible_layout.push(SegmentLayout {
                seg_index,
                x,
                width: *w,
            });
            x += *w as i32;
            // Separator follows every segment except the last visible one.
            if i + 1 < visible_count {
                x += SEPARATOR_WIDTH as i32;
            }
        }
    }

    /// True when the leftmost segments were dropped to fit.
    fn has_overflow(&self) -> bool {
        self.visible_layout.len() < self.segments.len()
    }

    /// Build the truncated path through `segments[..=seg_index]`.
    ///
    /// - Absolute paths: returns `"/a/b"` etc.
    /// - The single-segment-`/` case maps to `"/"`.
    /// - Relative paths: returns `"a/b"` etc.
    fn truncated_path(&self, seg_index: usize) -> String {
        if self.segments.len() == 1 && self.segments[0] == "/" {
            return String::from("/");
        }
        let mut out = String::new();
        if self.is_absolute {
            out.push('/');
        }
        for i in 0..=seg_index {
            if i > 0 {
                out.push('/');
            }
            out.push_str(&self.segments[i]);
        }
        out
    }

    /// Hit-test in PathBar-local coordinates. Returns the index into
    /// `visible_layout` for a hit, or `None` if the click missed every
    /// segment (including hits inside the inert "..." region).
    fn segment_at_x(&self, x: i32) -> Option<usize> {
        if x < 0 {
            return None;
        }
        for (i, layout) in self.visible_layout.iter().enumerate() {
            if x >= layout.x && x < layout.x + layout.width as i32 {
                return Some(i);
            }
        }
        None
    }
}

impl Window for PathBar {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn as_path_bar_mut(&mut self) -> Option<&mut PathBar> {
        Some(self)
    }

    fn set_bounds(&mut self, bounds: Rect) {
        let prev = self.base.bounds();
        self.base.set_bounds(bounds);
        // Width changes can change which segments fit.
        if prev.width != bounds.width {
            self.relayout();
        }
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let font = get_default_font();
        let char_width = font.cell_width();
        let line_height = font.line_height();

        // Background.
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            self.bg_color,
        );

        // Center text vertically.
        let text_y = bounds.y + (bounds.height.saturating_sub(line_height) / 2) as i32;

        // Overflow indicator: painted at the very left, inert.
        if self.has_overflow() {
            // Left-pad slightly so "..." doesn't kiss the edge.
            let dot_x = bounds.x + SEGMENT_PADDING as i32;
            device.draw_text(
                dot_x,
                text_y,
                OVERFLOW_TOKEN,
                font.as_font(),
                self.text_color,
            );
            // Trailing separator before the first visible segment.
            // Position it just inside the OVERFLOW_INDICATOR_WIDTH slot.
            let sep_x = bounds.x + OVERFLOW_INDICATOR_WIDTH as i32 - SEPARATOR_WIDTH as i32
                + ((SEPARATOR_WIDTH - char_width) / 2) as i32;
            device.draw_text(sep_x, text_y, ">", font.as_font(), self.separator_color);
        }

        // Segments and inter-segment separators.
        let visible_count = self.visible_layout.len();
        for (i, layout) in self.visible_layout.iter().enumerate() {
            let abs_x = bounds.x + layout.x;

            // Hover highlight.
            if self.hover_index == Some(i) {
                device.fill_rect(
                    abs_x,
                    bounds.y,
                    layout.width,
                    bounds.height,
                    self.hover_bg_color,
                );
            }

            // Segment text.
            let text_x = abs_x + SEGMENT_PADDING as i32;
            let seg = &self.segments[layout.seg_index];
            device.draw_text(text_x, text_y, seg, font.as_font(), self.text_color);

            // Separator between this and the next visible segment.
            if i + 1 < visible_count {
                let sep_x =
                    abs_x + layout.width as i32 + ((SEPARATOR_WIDTH - char_width) / 2) as i32;
                device.draw_text(sep_x, text_y, ">", font.as_font(), self.separator_color);
            }
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse) => {
                let bounds = self.base.bounds();
                let local_x = mouse.position.x;
                let local_y = mouse.position.y;
                let in_bar = local_x >= 0
                    && local_x < bounds.width as i32
                    && local_y >= 0
                    && local_y < bounds.height as i32;

                if !in_bar {
                    if self.hover_index.is_some() {
                        self.hover_index = None;
                        self.base.invalidate();
                    }
                    return EventResult::Propagate;
                }

                match mouse.event_type {
                    MouseEventType::Move => {
                        let new_hover = self.segment_at_x(local_x);
                        if new_hover != self.hover_index {
                            self.hover_index = new_hover;
                            self.base.invalidate();
                        }
                        EventResult::Handled
                    }
                    MouseEventType::ButtonDown if mouse.buttons.left => {
                        if let Some(visible_idx) = self.segment_at_x(local_x) {
                            let seg_index = self.visible_layout[visible_idx].seg_index;
                            let truncated = self.truncated_path(seg_index);
                            if let Some(cb) = self.on_segment_click.as_mut() {
                                cb(&truncated);
                            }
                        }
                        // Clicks anywhere inside the bar are considered
                        // handled (including over the inert "..." token,
                        // so they don't propagate as misses).
                        EventResult::Handled
                    }
                    _ => EventResult::Handled,
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
