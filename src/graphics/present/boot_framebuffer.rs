use crate::graphics::color::Color;
use crate::graphics::surface::Surface;
use crate::window::{GraphicsDevice, Rect};

use super::{PresentError, Presenter, PresenterKind};

/// Damaged-row conversion into the existing boot framebuffer device.
pub struct BootFramebufferPresenter<'a> {
    device: &'a mut dyn GraphicsDevice,
}

impl<'a> BootFramebufferPresenter<'a> {
    pub fn new(device: &'a mut dyn GraphicsDevice) -> Self {
        Self { device }
    }
}

impl Presenter for BootFramebufferPresenter<'_> {
    fn kind(&self) -> PresenterKind {
        PresenterKind::BootFramebuffer
    }

    fn present(&mut self, output: &Surface, damage: &[Rect]) -> Result<(), PresentError> {
        let bounds = Rect::new(0, 0, output.width(), output.height());
        for requested in damage {
            let Some(rect) = requested.intersection(&bounds) else {
                continue;
            };
            self.device.set_clip_rect(Some(rect));
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    let pixel = output
                        .pixel(x as u32, y as u32)
                        .ok_or(PresentError::Device)?;
                    let (r, g, b, _) = pixel.to_rgba();
                    self.device.draw_pixel(x, y, Color::new(r, g, b));
                }
            }
        }
        self.device.set_clip_rect(None);
        self.device.flush_regions(damage);
        Ok(())
    }
}
