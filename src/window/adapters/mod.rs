//! Adapters for existing graphics implementations

pub mod direct_framebuffer;
pub mod double_buffered;

pub use direct_framebuffer::DirectFrameBufferDevice;
pub use double_buffered::DoubleBufferedDevice;