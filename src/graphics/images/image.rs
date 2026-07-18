use crate::graphics::color::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
#[expect(dead_code, reason = "intentional kernel API surface")]
pub enum ImageFormat {
    Bmp,
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelFormat {
    #[expect(dead_code, reason = "intentional kernel API surface")]
    Rgb888,
    Bgr888,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    Rgba8888,
    Bgra8888,
    Rgb565,
    Grayscale8,
    Monochrome,
}

pub trait Image {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn format(&self) -> ImageFormat;
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn pixel_format(&self) -> PixelFormat;
    fn get_pixel(&self, x: usize, y: usize) -> Option<Color>;
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn get_pixel_data(&self) -> &[u8];
}
