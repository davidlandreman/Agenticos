//! Common types used throughout the window system

use core::sync::atomic::{AtomicUsize, Ordering};

/// Unique identifier for a window
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub usize);

/// Unique identifier for a screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScreenId(pub usize);

/// Generator for unique window IDs
static NEXT_WINDOW_ID: AtomicUsize = AtomicUsize::new(1);

/// Generator for unique screen IDs
static NEXT_SCREEN_ID: AtomicUsize = AtomicUsize::new(1);

impl WindowId {
    /// Generate a new unique window ID
    pub fn new() -> Self {
        WindowId(NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst))
    }
}

impl ScreenId {
    /// Generate a new unique screen ID
    pub fn new() -> Self {
        ScreenId(NEXT_SCREEN_ID.fetch_add(1, Ordering::SeqCst))
    }
}

/// A point in 2D space
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }
}

/// A rectangle defined by position and size
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Rect { x, y, width, height }
    }
    
    /// Check if a point is inside this rectangle
    pub fn contains_point(&self, point: Point) -> bool {
        point.x >= self.x 
            && point.x < self.x + self.width as i32
            && point.y >= self.y 
            && point.y < self.y + self.height as i32
    }
    
    /// Check if this rectangle intersects with another
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.width as i32
            && self.x + self.width as i32 > other.x
            && self.y < other.y + other.height as i32
            && self.y + self.height as i32 > other.y
    }
    
    /// Calculate the intersection of two rectangles
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        if !self.intersects(other) {
            return None;
        }
        
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let x2 = (self.x + self.width as i32).min(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).min(other.y + other.height as i32);
        
        Some(Rect::new(x, y, (x2 - x) as u32, (y2 - y) as u32))
    }
}

/// Color depth supported by the graphics device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    Bit8,
    Bit16,
    Bit24,
    Bit32,
}