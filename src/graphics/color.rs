#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Color { red, green, blue }
    }
    
    pub const BLACK: Color = Color { red: 0, green: 0, blue: 0 };
    pub const WHITE: Color = Color { red: 255, green: 255, blue: 255 };
    pub const RED: Color = Color { red: 255, green: 0, blue: 0 };
    pub const GREEN: Color = Color { red: 0, green: 255, blue: 0 };
    pub const BLUE: Color = Color { red: 0, green: 0, blue: 255 };
    pub const YELLOW: Color = Color { red: 255, green: 255, blue: 0 };
    pub const CYAN: Color = Color { red: 0, green: 255, blue: 255 };
    pub const MAGENTA: Color = Color { red: 255, green: 0, blue: 255 };
    pub const GRAY: Color = Color { red: 128, green: 128, blue: 128 };
    pub const LIGHT_GRAY: Color = Color { red: 192, green: 192, blue: 192 };
    pub const DARK_GRAY: Color = Color { red: 64, green: 64, blue: 64 };
    
    pub const fn from_hex(hex: u32) -> Self {
        Color {
            red: ((hex >> 16) & 0xFF) as u8,
            green: ((hex >> 8) & 0xFF) as u8,
            blue: (hex & 0xFF) as u8,
        }
    }
    
    pub const fn to_hex(&self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | (self.blue as u32)
    }
    
    pub fn blend(&self, other: &Color, alpha: u8) -> Self {
        let inv_alpha = 255 - alpha;
        Color {
            red: ((self.red as u16 * inv_alpha as u16 + other.red as u16 * alpha as u16) / 255) as u8,
            green: ((self.green as u16 * inv_alpha as u16 + other.green as u16 * alpha as u16) / 255) as u8,
            blue: ((self.blue as u16 * inv_alpha as u16 + other.blue as u16 * alpha as u16) / 255) as u8,
        }
    }
}