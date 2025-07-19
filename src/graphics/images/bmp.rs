use core::mem::size_of;
use crate::graphics::color::Color;
use super::image::{Image, ImageFormat, PixelFormat};
use crate::debug_info;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct BmpFileHeader {
    signature: [u8; 2],
    file_size: u32,
    reserved: u32,
    data_offset: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct BmpInfoHeader {
    header_size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bits_per_pixel: u16,
    compression: u32,
    image_size: u32,
    x_pixels_per_meter: i32,
    y_pixels_per_meter: i32,
    colors_used: u32,
    colors_important: u32,
}

#[derive(Debug)]
pub enum BmpError {
    InvalidSignature,
    InvalidHeaderSize,
    UnsupportedCompression,
    UnsupportedBitsPerPixel,
    InvalidDimensions,
    InsufficientData,
}

pub struct BmpImage<'a> {
    width: usize,
    height: usize,
    pixel_format: PixelFormat,
    data: &'a [u8],
    bottom_up: bool,
    bytes_per_pixel: usize,
    row_stride: usize,
    bits_per_pixel: u16,
    palette: Option<&'a [u8]>,
}

impl<'a> BmpImage<'a> {
    pub fn from_bytes(data: &'a [u8]) -> Result<Self, BmpError> {
        debug_info!("BMP: Parsing BMP file, data size: {} bytes", data.len());
        
        if data.len() < size_of::<BmpFileHeader>() + size_of::<BmpInfoHeader>() {
            return Err(BmpError::InsufficientData);
        }

        let file_header = unsafe {
            *(data.as_ptr() as *const BmpFileHeader)
        };

        let file_size = file_header.file_size;
        let data_offset = file_header.data_offset;
        debug_info!("BMP: Signature: {:?}, File size: {}, Data offset: {}", 
                    file_header.signature, file_size, data_offset);

        if file_header.signature != [b'B', b'M'] {
            return Err(BmpError::InvalidSignature);
        }

        let info_header = unsafe {
            *(data.as_ptr().add(size_of::<BmpFileHeader>()) as *const BmpInfoHeader)
        };

        let width = info_header.width;
        let height = info_header.height;
        let bits_per_pixel = info_header.bits_per_pixel;
        let compression = info_header.compression;
        debug_info!("BMP: Width: {}, Height: {}, BPP: {}, Compression: {}", 
                    width, height, bits_per_pixel, compression);

        if info_header.header_size < 40 {
            return Err(BmpError::InvalidHeaderSize);
        }

        if info_header.compression != 0 {
            return Err(BmpError::UnsupportedCompression);
        }

        let (pixel_format, bytes_per_pixel) = match bits_per_pixel {
            24 => (PixelFormat::Bgr888, 3),
            32 => (PixelFormat::Bgra8888, 4),
            16 => (PixelFormat::Rgb565, 2),
            8 => (PixelFormat::Grayscale8, 1),
            4 => (PixelFormat::Grayscale8, 1), // We'll treat 4-bit as grayscale for now
            1 => (PixelFormat::Monochrome, 1),
            _ => return Err(BmpError::UnsupportedBitsPerPixel),
        };

        let width = info_header.width.abs() as usize;
        let height = info_header.height.abs() as usize;
        
        if width == 0 || height == 0 {
            return Err(BmpError::InvalidDimensions);
        }

        let row_size = ((bits_per_pixel as usize * width + 31) / 32) * 4;
        
        if data.len() < data_offset as usize {
            return Err(BmpError::InsufficientData);
        }

        // For 4-bit and 8-bit BMPs, there's usually a color palette
        let palette = if bits_per_pixel <= 8 {
            let palette_start = size_of::<BmpFileHeader>() + info_header.header_size as usize;
            let palette_colors = if info_header.colors_used > 0 {
                info_header.colors_used as usize
            } else {
                1 << bits_per_pixel  // 2^bits_per_pixel colors
            };
            let palette_size = palette_colors * 4; // Each palette entry is 4 bytes (BGRA)
            
            if palette_start + palette_size <= data_offset as usize {
                Some(&data[palette_start..palette_start + palette_size])
            } else {
                None
            }
        } else {
            None
        };

        debug_info!("BMP: Palette present: {}, bits_per_pixel: {}", palette.is_some(), bits_per_pixel);

        Ok(BmpImage {
            width,
            height,
            pixel_format,
            data: &data[data_offset as usize..],
            bottom_up: info_header.height > 0,
            bytes_per_pixel,
            row_stride: row_size,
            bits_per_pixel,
            palette,
        })
    }

    fn get_pixel_offset(&self, x: usize, y: usize) -> usize {
        let actual_y = if self.bottom_up {
            self.height - 1 - y
        } else {
            y
        };
        
        actual_y * self.row_stride + x * self.bytes_per_pixel
    }
}

impl<'a> Image for BmpImage<'a> {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn format(&self) -> ImageFormat {
        ImageFormat::Bmp
    }

    fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }

        // Special handling for 4-bit images
        if self.bits_per_pixel == 4 {
            let actual_y = if self.bottom_up {
                self.height - 1 - y
            } else {
                y
            };
            
            let byte_offset = actual_y * self.row_stride + x / 2;
            if byte_offset >= self.data.len() {
                return None;
            }
            
            let byte = self.data[byte_offset];
            let nibble = if x % 2 == 0 {
                (byte >> 4) & 0x0F  // High nibble for even x
            } else {
                byte & 0x0F         // Low nibble for odd x
            };
            
            // Look up color in palette
            if let Some(palette) = self.palette {
                let palette_index = nibble as usize * 4;
                if palette_index + 3 < palette.len() {
                    let b = palette[palette_index];
                    let g = palette[palette_index + 1];
                    let r = palette[palette_index + 2];
                    return Some(Color::new(r, g, b));
                }
            }
            
            // Fallback to grayscale if no palette
            let gray = (nibble * 17) as u8; // Scale 0-15 to 0-255
            return Some(Color::new(gray, gray, gray));
        }

        let offset = self.get_pixel_offset(x, y);
        
        if self.bytes_per_pixel > 0 && offset + self.bytes_per_pixel > self.data.len() {
            return None;
        }

        match self.pixel_format {
            PixelFormat::Bgr888 => {
                let b = self.data[offset];
                let g = self.data[offset + 1];
                let r = self.data[offset + 2];
                Some(Color::new(r, g, b))
            }
            PixelFormat::Bgra8888 => {
                let b = self.data[offset];
                let g = self.data[offset + 1];
                let r = self.data[offset + 2];
                let _a = self.data[offset + 3];
                Some(Color::new(r, g, b))
            }
            PixelFormat::Rgb565 => {
                let pixel = u16::from_le_bytes([self.data[offset], self.data[offset + 1]]);
                let r = ((pixel >> 11) & 0x1F) as u8 * 8;
                let g = ((pixel >> 5) & 0x3F) as u8 * 4;
                let b = (pixel & 0x1F) as u8 * 8;
                Some(Color::new(r, g, b))
            }
            PixelFormat::Grayscale8 => {
                let gray = self.data[offset];
                Some(Color::new(gray, gray, gray))
            }
            PixelFormat::Monochrome => {
                let byte_index = offset / 8;
                let bit_index = 7 - (offset % 8);
                if byte_index < self.data.len() {
                    let bit = (self.data[byte_index] >> bit_index) & 1;
                    let value = if bit == 1 { 255 } else { 0 };
                    Some(Color::new(value, value, value))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn get_pixel_data(&self) -> &[u8] {
        self.data
    }
}