use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::graphics::images::BmpImage;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use super::base::WindowBase;

/// A desktop window that provides a background.
///
/// Without `wallpaper`, the desktop fills with `background_color`. When
/// `wallpaper` holds raw BMP bytes, `paint` reparses them per repaint and
/// blits the result, scaled to the desktop bounds. Parse failure or any
/// other error during paint silently falls back to the solid color so a
/// missing or malformed wallpaper never blocks boot.
pub struct DesktopWindow {
    base: WindowBase,
    background_color: Color,
    wallpaper: Option<Vec<u8>>,
}

impl DesktopWindow {
    pub fn new(id: WindowId, bounds: Rect) -> Self {
        Self {
            base: WindowBase::new_with_id(id, bounds),
            background_color: Color::new(0, 50, 100), // Nice blue desktop color
            wallpaper: None,
        }
    }

    /// Construct a desktop with raw BMP wallpaper bytes. The bytes are owned
    /// for the lifetime of the desktop and reparsed on each repaint; the
    /// solid fallback color is retained for use when parsing fails.
    pub fn new_with_wallpaper(id: WindowId, bounds: Rect, wallpaper: Vec<u8>) -> Self {
        Self {
            base: WindowBase::new_with_id(id, bounds),
            background_color: Color::new(0, 50, 100),
            wallpaper: Some(wallpaper),
        }
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
        if !self.base.visible() {
            return;
        }

        // Only paint if we actually need to repaint
        // This prevents overwriting child windows during partial updates
        if !self.base.needs_repaint() {
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