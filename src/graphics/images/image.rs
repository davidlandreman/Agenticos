use crate::graphics::color::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
#[expect(dead_code, reason = "intentional kernel API surface")]
pub enum ImageFormat {
    Bmp,
    Svg,
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
    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "intentional kernel API surface")
    )]
    fn format(&self) -> ImageFormat;
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn pixel_format(&self) -> PixelFormat;
    fn get_pixel(&self, x: usize, y: usize) -> Option<Color>;

    /// Sample this image as though it were rendered at `target_width` x
    /// `target_height`. Raster formats inherit nearest-neighbor sampling;
    /// vector formats override this to rasterize directly at the requested
    /// size.
    fn get_scaled_pixel(
        &self,
        x: usize,
        y: usize,
        target_width: usize,
        target_height: usize,
    ) -> Option<Color> {
        if target_width == 0 || target_height == 0 {
            return None;
        }
        let source_x = x.saturating_mul(self.width()) / target_width;
        let source_y = y.saturating_mul(self.height()) / target_height;
        self.get_pixel(source_x, source_y)
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn get_pixel_data(&self) -> &[u8];
}
