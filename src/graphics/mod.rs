pub mod color;
pub mod fonts;
pub mod core_text;
pub mod core_gfx;
pub mod mouse_cursor;
pub mod images;

// New graphics pipeline modules (Phase 2)
pub mod framebuffer;
pub mod compositor;
pub mod render;

// Re-exports for convenient access
pub use framebuffer::{SavedRegion, RegionCapableBuffer, FramebufferInfo};
pub use compositor::{DirtyRectManager, CursorOverlay, Compositor, FrameState};
pub use render::{RenderTarget, PaintContext};