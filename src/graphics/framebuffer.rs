//! Unified framebuffer abstraction for the graphics system.
//!
//! This module provides a clean abstraction over the underlying framebuffer
//! with support for:
//! - Region save/restore (for cursor overlays)
//! - Optimized row-based operations
//! - Partial buffer swaps (for dirty region updates)

use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::window::types::Rect;

/// Saved region of the framebuffer for overlay operations (like mouse cursor).
#[derive(Debug)]
pub struct SavedRegion {
    /// X position of the saved region
    pub x: usize,
    /// Y position of the saved region
    pub y: usize,
    /// Width of the saved region
    pub width: usize,
    /// Height of the saved region
    pub height: usize,
    /// Saved pixel data (packed RGB values)
    pub pixels: Vec<u32>,
}

impl SavedRegion {
    /// Create a new empty saved region
    pub fn new() -> Self {
        SavedRegion {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }
    }

    /// Check if the region has valid saved data
    pub fn is_valid(&self) -> bool {
        !self.pixels.is_empty() && self.width > 0 && self.height > 0
    }

    /// Get the bounding rectangle
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x as i32, self.y as i32, self.width as u32, self.height as u32)
    }
}

impl Default for SavedRegion {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for framebuffer devices that support region operations.
///
/// This extends the basic pixel operations with region save/restore
/// functionality needed for proper cursor overlay handling.
pub trait RegionCapableBuffer {
    /// Save a rectangular region of pixels
    fn save_region(&self, x: usize, y: usize, width: usize, height: usize) -> SavedRegion;

    /// Restore a previously saved region
    fn restore_region(&mut self, region: &SavedRegion);

    /// Fill a row with a solid color (optimized bulk operation)
    fn fill_row(&mut self, x: usize, y: usize, width: usize, color: Color);

    /// Copy pixels between regions (for scrolling)
    fn copy_region(&mut self, src_x: usize, src_y: usize, dst_x: usize, dst_y: usize, width: usize, height: usize);

    /// Swap only a specific rectangular region to the front buffer
    fn swap_region(&mut self, rect: &Rect);
}

/// Information about a framebuffer's layout and format.
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    /// Width in pixels
    pub width: usize,
    /// Height in pixels
    pub height: usize,
    /// Bytes per pixel (typically 3 or 4)
    pub bytes_per_pixel: usize,
    /// Stride (bytes per row, may be larger than width * bytes_per_pixel)
    pub stride: usize,
    /// Whether the buffer uses BGR byte order (vs RGB)
    pub is_bgr: bool,
}

impl FramebufferInfo {
    /// Calculate byte offset for a pixel at (x, y)
    #[inline]
    pub fn pixel_offset(&self, x: usize, y: usize) -> usize {
        (y * self.stride + x) * self.bytes_per_pixel
    }

    /// Calculate byte offset for start of row y
    #[inline]
    pub fn row_offset(&self, y: usize) -> usize {
        y * self.stride * self.bytes_per_pixel
    }

    /// Calculate bytes per visible row
    #[inline]
    pub fn row_bytes(&self) -> usize {
        self.width * self.bytes_per_pixel
    }

    /// Total buffer size in bytes
    #[inline]
    pub fn buffer_size(&self) -> usize {
        self.height * self.stride * self.bytes_per_pixel
    }
}

/// Helper to pack a Color into bytes based on pixel format.
#[inline]
pub fn color_to_bytes(color: Color, is_bgr: bool) -> [u8; 4] {
    if is_bgr {
        [color.blue, color.green, color.red, 0xFF]
    } else {
        [color.red, color.green, color.blue, 0xFF]
    }
}

/// Helper to unpack bytes into a Color based on pixel format.
#[inline]
pub fn bytes_to_color(bytes: &[u8], is_bgr: bool) -> Color {
    if is_bgr {
        Color::new(bytes[2], bytes[1], bytes[0])
    } else {
        Color::new(bytes[0], bytes[1], bytes[2])
    }
}

/// Pack a color into a u32 for efficient storage.
#[inline]
pub fn color_to_u32(color: Color) -> u32 {
    ((color.red as u32) << 16) | ((color.green as u32) << 8) | (color.blue as u32)
}

/// Unpack a u32 back to a Color.
#[inline]
pub fn u32_to_color(packed: u32) -> Color {
    Color::new(
        ((packed >> 16) & 0xFF) as u8,
        ((packed >> 8) & 0xFF) as u8,
        (packed & 0xFF) as u8,
    )
}
