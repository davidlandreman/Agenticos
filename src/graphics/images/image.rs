use crate::graphics::color::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageFormat {
    Bmp,
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelFormat {
    Rgb888,
    Bgr888,
    Rgba8888,
    Bgra8888,
    Rgb565,
    Grayscale8,
    Monochrome,
}

pub trait Image {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn format(&self) -> ImageFormat;
    fn pixel_format(&self) -> PixelFormat;
    fn get_pixel(&self, x: usize, y: usize) -> Option<Color>;
    fn get_pixel_data(&self) -> &[u8];
}