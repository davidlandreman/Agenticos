use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::graphics::images::{BmpImage, Image};
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowBuffer, WindowId};
use super::base::WindowBase;

/// A desktop window that provides a background.
///
/// Without `wallpaper`, the desktop fills with `background_color`. When
/// `wallpaper` holds raw BMP bytes, the desktop pre-rasterizes the
/// scaled image into a `WindowBuffer` (`backing_store`) on each
/// invalidation; the compositor blits that buffer to the back buffer
/// per frame, so cursor moves and frame drags do not re-parse the BMP.
///
/// Parse failure or any other error during rasterization silently falls
/// back to filling the backing store with `background_color` so a
/// missing or malformed wallpaper never blocks boot.
pub struct DesktopWindow {
    base: WindowBase,
    background_color: Color,
    wallpaper: Option<Vec<u8>>,
    backing_store: Option<WindowBuffer>,
}

impl DesktopWindow {
    pub fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            base: WindowBase::new_with_id(id, bounds),
            background_color: Color::new(0, 50, 100), // Nice blue desktop color
            wallpaper: None,
            backing_store: None,
        }
    }

    /// Construct a desktop with raw BMP wallpaper bytes. The bytes are owned
    /// for the lifetime of the desktop and reparsed only on invalidation;
    /// the solid fallback color is retained for use when parsing fails.
    pub fn new_with_wallpaper(id: WindowId, bounds: Rect, wallpaper: Vec<u8>) -> Self {
        Self {
            base: WindowBase::new_with_id(id, bounds),
            background_color: Color::new(0, 50, 100),
            wallpaper: Some(wallpaper),
            backing_store: None,
        }
    }

    /// Fill the backing store with the solid background color in the
    /// store's pixel format. Used for the no-wallpaper / parse-failure
    /// fallback path so the desktop still renders.
    fn rasterize_solid(&mut self) {
        if let Some(buf) = self.backing_store.as_mut() {
            let color = self.background_color;
            for y in 0..buf.height {
                for x in 0..buf.width {
                    buf.write_pixel(x, y, color);
                }
            }
        }
    }

    /// Rasterize the scaled wallpaper into the backing store using
    /// nearest-neighbor sampling. Returns whether rasterization
    /// succeeded; on failure callers should fall back to the solid
    /// color path so the desktop never goes black.
    fn rasterize_wallpaper(&mut self) -> bool {
        let Some(bytes) = self.wallpaper.as_ref() else {
            return false;
        };
        let Ok(image) = BmpImage::from_bytes(bytes) else {
            return false;
        };
        let Some(buf) = self.backing_store.as_mut() else {
            return false;
        };

        let src_w = image.width();
        let src_h = image.height();
        if src_w == 0 || src_h == 0 || buf.width == 0 || buf.height == 0 {
            return false;
        }

        for dy in 0..buf.height {
            let sy = (dy * src_h) / buf.height;
            for dx in 0..buf.width {
                let sx = (dx * src_w) / buf.width;
                if let Some(color) = image.get_pixel(sx, sy) {
                    buf.write_pixel(dx, dy, color);
                }
            }
        }
        true
    }
}

impl Window for DesktopWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        // Direct-paint fallback. The compositor routes around this for
        // opted-in windows (wants_backing_store == true) by calling
        // paint_into_backing_store + blitting from the cache; this path
        // is reached only when the backing store is unavailable (e.g. in
        // tests that bypass the backing-store flow, or pathological
        // cases where rasterization didn't produce a buffer).
        //
        // Per the `Window::paint` contract, this does not early-return on
        // `!needs_repaint()` — the compositor decides whether to call us
        // and sets the device clip; our job is just to write correct
        // pixels within that clip.
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let mut painted = false;

        if let Some(bytes) = self.wallpaper.as_ref() {
            if let Ok(image) = BmpImage::from_bytes(bytes) {
                device.draw_image_scaled(
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    &image,
                );
                painted = true;
            }
        }

        if !painted {
            device.fill_rect(
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                self.background_color,
            );
        }

        self.base.clear_needs_repaint();
    }

    fn wants_backing_store(&self) -> bool {
        true
    }

    fn paint_into_backing_store(&mut self, device: &dyn GraphicsDevice) {
        let bounds = self.base.bounds();
        let target_w = bounds.width as usize;
        let target_h = bounds.height as usize;

        // Allocate or resize the backing store to match current bounds and
        // the framebuffer's actual format.
        match self.backing_store.as_mut() {
            Some(buf)
                if buf.width == target_w
                    && buf.height == target_h
                    && buf.pixel_format == device.pixel_format()
                    && buf.bytes_per_pixel == device.bytes_per_pixel() =>
            {
                // Reuse the existing buffer in place; rasterization below
                // overwrites it.
            }
            _ => {
                self.backing_store = Some(WindowBuffer::for_device(
                    target_w, target_h, device,
                ));
            }
        }

        // Try the wallpaper path first; on any failure, fill the buffer
        // with the solid fallback color so the desktop still renders.
        if !self.rasterize_wallpaper() {
            self.rasterize_solid();
        }

        // Mark this rasterization complete so subsequent renders without
        // an explicit invalidate don't re-parse the BMP.
        self.base.clear_needs_repaint();
    }

    fn backing_store(&self) -> Option<&WindowBuffer> {
        self.backing_store.as_ref()
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Ignored
    }

    // Desktop never accepts focus — override the default delegation so
    // `set_focus(true)` cannot mark the desktop as focused.
    fn has_focus(&self) -> bool {
        false
    }

    fn set_focus(&mut self, _focused: bool) {
        // Desktop never has focus
    }
}