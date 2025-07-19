use core::mem::size_of;
use crate::graphics::color::Color;
use super::image::{Image, ImageFormat, PixelFormat};
use crate::{debug_info, debug_debug, debug_error};

#[derive(Debug)]
pub enum PngError {
    InvalidSignature,
    InvalidChunk,
    UnsupportedColorType,
    UnsupportedBitDepth,
    UnsupportedCompression,
    UnsupportedFilter,
    UnsupportedInterlace,
    MissingRequiredChunk,
    InvalidDimensions,
    InsufficientData,
    DecompressionError,
    FilterError,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct PngSignature {
    bytes: [u8; 8],
}

const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct ChunkHeader {
    length: [u8; 4],
    chunk_type: [u8; 4],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct IhdrChunk {
    width: [u8; 4],
    height: [u8; 4],
    bit_depth: u8,
    color_type: u8,
    compression_method: u8,
    filter_method: u8,
    interlace_method: u8,
}

#[derive(Debug, Clone, Copy)]
enum ColorType {
    Grayscale = 0,
    Rgb = 2,
    Palette = 3,
    GrayscaleAlpha = 4,
    RgbAlpha = 6,
}

impl ColorType {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ColorType::Grayscale),
            2 => Some(ColorType::Rgb),
            3 => Some(ColorType::Palette),
            4 => Some(ColorType::GrayscaleAlpha),
            6 => Some(ColorType::RgbAlpha),
            _ => None,
        }
    }
}

pub struct PngImage {
    width: usize,
    height: usize,
    pixel_format: PixelFormat,
    data: &'static [u8],
    bit_depth: u8,
    color_type: ColorType,
    // Decompressed image data will be stored here once we implement decompression
    decompressed_data: Option<&'static [u8]>,
}

impl PngImage {
    pub fn from_bytes(data: &'static [u8]) -> Result<Self, PngError> {
        debug_info!("PNG: Parsing PNG file, data size: {} bytes", data.len());
        
        // Check minimum size for signature
        if data.len() < 8 {
            return Err(PngError::InsufficientData);
        }
        
        // Verify PNG signature
        let signature = &data[0..8];
        if signature != PNG_SIGNATURE {
            debug_error!("PNG: Invalid signature: {:?}", signature);
            return Err(PngError::InvalidSignature);
        }
        
        // Parse IHDR chunk (must be first chunk after signature)
        let mut offset = 8;
        
        // Read chunk header
        if offset + size_of::<ChunkHeader>() > data.len() {
            return Err(PngError::InsufficientData);
        }
        
        let chunk_length = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
        ]);
        let chunk_type = &data[offset + 4..offset + 8];
        
        if chunk_type != b"IHDR" {
            debug_error!("PNG: First chunk is not IHDR");
            return Err(PngError::MissingRequiredChunk);
        }
        
        offset += 8; // Skip chunk header
        
        // Parse IHDR data
        if offset + size_of::<IhdrChunk>() > data.len() {
            return Err(PngError::InsufficientData);
        }
        
        let width = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
        ]) as usize;
        let height = u32::from_be_bytes([
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]
        ]) as usize;
        let bit_depth = data[offset + 8];
        let color_type_byte = data[offset + 9];
        let compression_method = data[offset + 10];
        let filter_method = data[offset + 11];
        let interlace_method = data[offset + 12];
        
        debug_info!("PNG: Width: {}, Height: {}, Bit depth: {}, Color type: {}", 
                    width, height, bit_depth, color_type_byte);
        
        // Validate dimensions
        if width == 0 || height == 0 {
            return Err(PngError::InvalidDimensions);
        }
        
        // Parse color type
        let color_type = ColorType::from_u8(color_type_byte)
            .ok_or(PngError::UnsupportedColorType)?;
        
        // Validate bit depth based on color type
        let is_valid_bit_depth = match color_type {
            ColorType::Grayscale => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
            ColorType::Rgb => matches!(bit_depth, 8 | 16),
            ColorType::Palette => matches!(bit_depth, 1 | 2 | 4 | 8),
            ColorType::GrayscaleAlpha => matches!(bit_depth, 8 | 16),
            ColorType::RgbAlpha => matches!(bit_depth, 8 | 16),
        };
        
        if !is_valid_bit_depth {
            return Err(PngError::UnsupportedBitDepth);
        }
        
        // Only support compression method 0 (deflate)
        if compression_method != 0 {
            return Err(PngError::UnsupportedCompression);
        }
        
        // Only support filter method 0
        if filter_method != 0 {
            return Err(PngError::UnsupportedFilter);
        }
        
        // Only support no interlacing (0) for now
        if interlace_method != 0 {
            return Err(PngError::UnsupportedInterlace);
        }
        
        // Determine pixel format
        let pixel_format = match (color_type, bit_depth) {
            (ColorType::Grayscale, 8) => PixelFormat::Grayscale8,
            (ColorType::Grayscale, _) => PixelFormat::Grayscale8, // Convert to 8-bit
            (ColorType::Rgb, 8) => PixelFormat::Rgb888,
            (ColorType::RgbAlpha, 8) => PixelFormat::Rgba8888,
            _ => {
                debug_error!("PNG: Unsupported color type/bit depth combination");
                return Err(PngError::UnsupportedColorType);
            }
        };
        
        // TODO: Parse remaining chunks (IDAT, IEND, etc.)
        // TODO: Decompress IDAT chunks
        // TODO: Apply filters to reconstruct image data
        
        Ok(PngImage {
            width,
            height,
            pixel_format,
            data,
            bit_depth,
            color_type,
            decompressed_data: None,
        })
    }
    
    // Helper function to read a 32-bit big-endian integer
    fn read_u32_be(data: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }
}

impl Image for PngImage {
    fn width(&self) -> usize {
        self.width
    }
    
    fn height(&self) -> usize {
        self.height
    }
    
    fn format(&self) -> ImageFormat {
        ImageFormat::Png
    }
    
    fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }
    
    fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        
        // TODO: Implement pixel retrieval once we have decompressed data
        // For now, return a placeholder color
        Some(Color::MAGENTA) // Magenta indicates unimplemented
    }
    
    fn get_pixel_data(&self) -> &[u8] {
        // TODO: Return decompressed pixel data once implemented
        &[]
    }
}