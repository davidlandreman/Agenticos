//! `ScrollView` — a single-child wrapper that draws a scrollbar when its
//! content is larger than its viewport, and translates child paint and
//! event coordinates by the current scroll offset.
//!
//! Content size is supplied by the caller via [`ScrollView::set_content_size`];
//! the wrapper does not introspect the child's natural size. Callers that
//! migrate to `ScrollView` (List, MultiColumnList, TextEditor, ...) keep
//! the content extent up to date as their data changes.
//!
//! The render-time-transform pattern (temporarily writing the child's
//! bounds via `set_bounds_no_invalidate` while painting) mirrors what the
//! `WindowManager`'s parent-offset path already does — the difference is
//! that here the offset includes the scroll offset, and the bounds we
//! write extend to cover the full content rectangle so the child draws
//! everything (clipping is handled by the active clip rect).

use crate::graphics::color::Color;
use crate::window::event::MouseEventType;
use crate::window::manager::with_active_manager;
use crate::window::windows::base::WindowBase;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

/// Pixels per unit of scroll-wheel delta. Roughly one line at the default
/// font height.
const WHEEL_STEP: i32 = 16;

/// Width in pixels of the scrollbar track and thumb.
const SCROLLBAR_WIDTH: u32 = 8;

/// Minimum thumb size in pixels along the scroll axis.
const MIN_THUMB_SIZE: u32 = 10;

/// A single-child scroll viewport.
///
/// The child window's local bounds during paint are temporarily set to
/// `(viewport_x - scroll_x, viewport_y - scroll_y, content_w, content_h)`
/// via `set_bounds_no_invalidate`, so the child can render its full
/// content area while the active clip rect (set by `ScrollView::paint`)
/// limits visible pixels to the viewport.
pub struct ScrollView {
    base: WindowBase,
    /// Single content child window id, if set.
    content_id: Option<WindowId>,
    /// Caller-provided content extent (width, height).
    content_w: u32,
    content_h: u32,
    /// Current scroll offset.
    scroll_x: i32,
    scroll_y: i32,
    /// Whether horizontal scrolling is enabled (default: false).
    h_scroll_enabled: bool,
    /// Whether vertical scrolling is enabled (default: true).
    v_scroll_enabled: bool,
    /// When the user grabs the scrollbar thumb, stores
    /// `(thumb_top - mouse.y)` so we can preserve the grab offset across
    /// drag moves. `None` while no thumb-drag is in progress.
    thumb_grab: Option<i32>,
    /// Background color drawn behind the viewport.
    bg_color: Color,
}

impl ScrollView {
    /// Create a new `ScrollView` with the given outer bounds.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Create a new `ScrollView` with a specific window id.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        ScrollView {
            base: WindowBase::new_with_id(id, bounds),
            content_id: None,
            content_w: 0,
            content_h: 0,
            scroll_x: 0,
            scroll_y: 0,
            h_scroll_enabled: false,
            v_scroll_enabled: true,
            thumb_grab: None,
            bg_color: crate::window::PALETTE_CONTENT_BG,
        }
    }

    /// Set (or replace) the single child content window.
    pub fn set_content(&mut self, content_id: WindowId) {
        if let Some(prev) = self.content_id.take() {
            if prev != content_id {
                self.base.remove_child(prev);
            }
        }
        self.content_id = Some(content_id);
        self.base.add_child(content_id);
        self.base.invalidate();
    }

    /// Set the natural size of the content area. The caller is responsible
    /// for keeping this up to date as the child's data changes.
    pub fn set_content_size(&mut self, w: u32, h: u32) {
        if self.content_w != w || self.content_h != h {
            self.content_w = w;
            self.content_h = h;
            self.clamp_scroll();
            self.base.invalidate();
        }
    }

    /// Enable or disable horizontal scrolling. Vertical scrolling is on
    /// by default.
    pub fn set_horizontal_enabled(&mut self, enabled: bool) {
        if self.h_scroll_enabled != enabled {
            self.h_scroll_enabled = enabled;
            if !enabled {
                self.scroll_x = 0;
            }
            self.base.invalidate();
        }
    }

    /// Set the background color drawn behind the viewport.

    /// Current horizontal scroll offset (in content-coordinate pixels).
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn scroll_x(&self) -> i32 {
        self.scroll_x
    }

    /// Current vertical scroll offset (in content-coordinate pixels).
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn scroll_y(&self) -> i32 {
        self.scroll_y
    }

    /// Programmatically scroll to `(x, y)`. Values are clamped to
    /// `[0, content - viewport]` on each axis (and to 0 when content
    /// fits entirely inside the viewport).
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn scroll_to(&mut self, x: i32, y: i32) {
        self.scroll_x = x;
        self.scroll_y = y;
        self.clamp_scroll();
        self.base.invalidate();
    }

    /// Adjust scroll offsets so that `rect` (in content coordinates) is
    /// fully visible inside the viewport. If the rect is larger than the
    /// viewport on an axis, the rect's top-left edge wins.
    pub fn ensure_visible(&mut self, rect: Rect) {
        let (vw, vh) = self.viewport_size();
        let viewport_w = vw as i32;
        let viewport_h = vh as i32;

        // Vertical
        if self.v_scroll_enabled {
            let rect_top = rect.y;
            let rect_bottom = rect.y + rect.height as i32;
            if rect_top < self.scroll_y {
                self.scroll_y = rect_top;
            } else if rect_bottom > self.scroll_y + viewport_h {
                self.scroll_y = rect_bottom - viewport_h;
            }
        }

        // Horizontal
        if self.h_scroll_enabled {
            let rect_left = rect.x;
            let rect_right = rect.x + rect.width as i32;
            if rect_left < self.scroll_x {
                self.scroll_x = rect_left;
            } else if rect_right > self.scroll_x + viewport_w {
                self.scroll_x = rect_right - viewport_w;
            }
        }

        self.clamp_scroll();
        self.base.invalidate();
    }

    /// True when the content's vertical extent exceeds the viewport.
    fn v_overflow(&self) -> bool {
        self.v_scroll_enabled && self.content_h > self.base.bounds().height
    }

    /// True when the content's horizontal extent exceeds the viewport
    /// AND horizontal scrolling is enabled.
    fn h_overflow(&self) -> bool {
        self.h_scroll_enabled && self.content_w > self.base.bounds().width
    }

    /// Effective viewport dimensions (outer bounds shrunk by any visible
    /// scrollbar gutters).
    fn viewport_size(&self) -> (u32, u32) {
        let bounds = self.base.bounds();
        let w = if self.v_overflow() {
            bounds.width.saturating_sub(SCROLLBAR_WIDTH)
        } else {
            bounds.width
        };
        let h = if self.h_overflow() {
            bounds.height.saturating_sub(SCROLLBAR_WIDTH)
        } else {
            bounds.height
        };
        (w, h)
    }

    /// Clamp scroll offsets to `[0, content - viewport]` on each axis.
    fn clamp_scroll(&mut self) {
        let (vw, vh) = self.viewport_size();
        let max_y = (self.content_h as i32 - vh as i32).max(0);
        let max_x = (self.content_w as i32 - vw as i32).max(0);
        if !self.v_scroll_enabled {
            self.scroll_y = 0;
        } else {
            self.scroll_y = self.scroll_y.clamp(0, max_y);
        }
        if !self.h_scroll_enabled {
            self.scroll_x = 0;
        } else {
            self.scroll_x = self.scroll_x.clamp(0, max_x);
        }
    }

    /// Geometry of the vertical scrollbar's track and thumb.
    ///
    /// Returned tuple is `(track_top, track_height, thumb_top, thumb_height)`
    /// in the same coordinate frame as `WindowBase::bounds()` (i.e.
    /// parent-relative — the same frame `MouseEvent::position` arrives in).
    /// Returns `None` if the bar is not drawn.
    fn vbar_geometry(&self) -> Option<(i32, u32, i32, u32)> {
        if !self.v_overflow() {
            return None;
        }
        let bounds = self.base.bounds();
        let (_, viewport_h) = self.viewport_size();
        if viewport_h == 0 || self.content_h == 0 {
            return None;
        }

        // Track spans the (possibly shortened) viewport height.
        let track_top = bounds.y;
        let track_h = viewport_h;

        // Thumb height is proportional to viewport / content, with a
        // minimum so it stays grabbable for very long content.
        let thumb_h_raw = (viewport_h as u64 * track_h as u64) / (self.content_h as u64);
        let thumb_h = (thumb_h_raw as u32).max(MIN_THUMB_SIZE).min(track_h);

        // Thumb position: scroll_y / (content - viewport) * (track - thumb).
        let scroll_range = (self.content_h as i32 - viewport_h as i32).max(1);
        let thumb_range = (track_h as i32 - thumb_h as i32).max(0);
        let thumb_offset = (self.scroll_y * thumb_range) / scroll_range;
        let thumb_top = track_top + thumb_offset;

        Some((track_top, track_h, thumb_top, thumb_h))
    }

    /// Apply a scroll-wheel event payload, clamping and invalidating.
    fn apply_wheel(&mut self, delta_x: i32, delta_y: i32) {
        if self.v_scroll_enabled {
            // Convention: delta_y > 0 = scroll content up (i.e. show
            // content further down). Match the upstream MouseEventType
            // doc ("positive = down").
            self.scroll_y = self.scroll_y.saturating_add(delta_y * WHEEL_STEP);
        }
        if self.h_scroll_enabled {
            self.scroll_x = self.scroll_x.saturating_add(delta_x * WHEEL_STEP);
        }
        self.clamp_scroll();
        self.base.invalidate();
    }

    /// Handle a `ButtonDown` over the (possibly visible) vertical thumb.
    /// `mouse_y` is in the same frame as `MouseEvent::position` (i.e.
    /// parent-relative, same as `bounds`). Returns `true` if the event
    /// was consumed (i.e. the click landed on the thumb).
    fn try_begin_thumb_drag(&mut self, mouse_y: i32) -> bool {
        let Some((_track_top, _track_h, thumb_top, thumb_h)) = self.vbar_geometry() else {
            return false;
        };
        let thumb_bottom = thumb_top + thumb_h as i32;
        if mouse_y >= thumb_top && mouse_y < thumb_bottom {
            self.thumb_grab = Some(thumb_top - mouse_y);
            return true;
        }
        false
    }

    /// Update scroll_y based on a thumb drag in progress.
    fn update_thumb_drag(&mut self, mouse_y: i32) {
        let Some(grab) = self.thumb_grab else {
            return;
        };
        let Some((track_top, track_h, _thumb_top, thumb_h)) = self.vbar_geometry() else {
            return;
        };
        let (_, viewport_h) = self.viewport_size();
        let new_thumb_top = mouse_y + grab;
        let scroll_range = (self.content_h as i32 - viewport_h as i32).max(1);
        let thumb_range = (track_h as i32 - thumb_h as i32).max(1);
        let new_scroll_y = ((new_thumb_top - track_top) * scroll_range) / thumb_range;
        self.scroll_y = new_scroll_y;
        self.clamp_scroll();
        self.base.invalidate();
    }
}

impl Window for ScrollView {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    // Custom override: clamp scroll offsets when the viewport changes.
    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        self.clamp_scroll();
    }

    fn add_child(&mut self, child: WindowId) {
        // Treat any added child as the single content slot.
        self.set_content(child);
    }

    fn remove_child(&mut self, child: WindowId) {
        if self.content_id == Some(child) {
            self.content_id = None;
        }
        self.base.remove_child(child);
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let (viewport_w, viewport_h) = self.viewport_size();

        // Background fill for the entire ScrollView outer bounds (so
        // areas not covered by the child still look right).
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            self.bg_color,
        );

        // Draw vertical scrollbar (track + thumb). Coordinates from
        // `vbar_geometry` are already in the same frame as `bounds`.
        if let Some((track_top, track_h, thumb_top, thumb_h)) = self.vbar_geometry() {
            let track_x = bounds.x + bounds.width as i32 - SCROLLBAR_WIDTH as i32;
            // Track
            device.fill_rect(
                track_x,
                track_top,
                SCROLLBAR_WIDTH,
                track_h,
                Color::LIGHT_GRAY,
            );
            // Thumb
            device.fill_rect(track_x, thumb_top, SCROLLBAR_WIDTH, thumb_h, Color::GRAY);
        }

        // Draw horizontal scrollbar (track + thumb), if enabled and overflowing.
        if self.h_overflow() {
            let track_y = bounds.y + bounds.height as i32 - SCROLLBAR_WIDTH as i32;
            let track_w = viewport_w;
            device.fill_rect(
                bounds.x,
                track_y,
                track_w,
                SCROLLBAR_WIDTH,
                Color::LIGHT_GRAY,
            );
            // Horizontal thumb math, mirror of vertical.
            if track_w > 0 && self.content_w > 0 {
                let thumb_w_raw = (viewport_w as u64 * track_w as u64) / (self.content_w as u64);
                let thumb_w = (thumb_w_raw as u32).max(MIN_THUMB_SIZE).min(track_w);
                let scroll_range = (self.content_w as i32 - viewport_w as i32).max(1);
                let thumb_range = (track_w as i32 - thumb_w as i32).max(0);
                let thumb_offset = (self.scroll_x * thumb_range) / scroll_range;
                device.fill_rect(
                    bounds.x + thumb_offset,
                    track_y,
                    thumb_w,
                    SCROLLBAR_WIDTH,
                    Color::GRAY,
                );
            }
        }

        // Set clip to the viewport (excludes scrollbar gutters), then
        // reach into the manager to translate the child's bounds and
        // paint it.
        let viewport_rect = Rect::new(bounds.x, bounds.y, viewport_w, viewport_h);
        device.set_clip_rect(Some(viewport_rect));

        let content_id = self.content_id;
        let content_w = self.content_w.max(viewport_w);
        let content_h = self.content_h.max(viewport_h);
        let scroll_x = self.scroll_x;
        let scroll_y = self.scroll_y;

        if let Some(child_id) = content_id {
            // Translate child bounds: place its local origin at
            // `(viewport.x - scroll_x, viewport.y - scroll_y)`, give
            // it the full content extent so it draws everything, then
            // restore the original bounds afterwards.
            with_active_manager(|wm| {
                if let Some(child) = wm.window_registry.get_mut(&child_id) {
                    let original = child.bounds();
                    let translated = Rect::new(
                        bounds.x - scroll_x,
                        bounds.y - scroll_y,
                        content_w,
                        content_h,
                    );
                    child.set_bounds_no_invalidate(translated);
                    child.invalidate();
                    child.paint(device);
                    child.set_bounds_no_invalidate(original);
                }
            });
        }

        // Restore clipping (the manager's renderer will set its own
        // clip rect for sibling windows; we clear ours here so we don't
        // leak viewport clipping past this paint call).
        device.set_clip_rect(None);

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                match mouse_event.event_type {
                    MouseEventType::Scroll { delta_x, delta_y } => {
                        // No-op fast path: nothing to scroll.
                        if !self.v_overflow() && !self.h_overflow() {
                            return EventResult::Handled;
                        }
                        self.apply_wheel(delta_x, delta_y);
                        EventResult::Handled
                    }
                    MouseEventType::ButtonDown if mouse_event.buttons.left => {
                        // `position` arrives in the same frame as
                        // `bounds()` — see MouseEvent::position docs.
                        if self.try_begin_thumb_drag(mouse_event.position.y) {
                            EventResult::Handled
                        } else {
                            EventResult::Propagate
                        }
                    }
                    MouseEventType::Move => {
                        if self.thumb_grab.is_some() {
                            self.update_thumb_drag(mouse_event.position.y);
                            EventResult::Handled
                        } else {
                            EventResult::Propagate
                        }
                    }
                    MouseEventType::ButtonUp => {
                        if self.thumb_grab.is_some() {
                            self.thumb_grab = None;
                            EventResult::Handled
                        } else {
                            EventResult::Propagate
                        }
                    }
                    _ => EventResult::Propagate,
                }
            }
            Event::EnsureVisible(rect) => {
                self.ensure_visible(rect);
                EventResult::Handled
            }
            _ => EventResult::Propagate,
        }
    }

    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }

    fn is_scroll_view(&self) -> bool {
        true
    }
}
