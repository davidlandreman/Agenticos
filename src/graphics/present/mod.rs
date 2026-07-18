//! Scanout/presentation boundary for retained composition.

mod boot_framebuffer;

pub use boot_framebuffer::BootFramebufferPresenter;

use crate::graphics::surface::Surface;
use crate::window::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(dead_code, reason = "intentional kernel API surface")]
pub enum PresenterKind {
    BootFramebuffer,
    VirtioGpu2d,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentError {
    #[expect(dead_code, reason = "intentional kernel API surface")]
    UnsupportedFormat,
    Device,
}

pub trait Presenter {
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn kind(&self) -> PresenterKind;
    fn present(&mut self, output: &Surface, damage: &[Rect]) -> Result<(), PresentError>;
}
