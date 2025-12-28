//! Common types used throughout the window system

use core::sync::atomic::{AtomicUsize, Ordering};

/// Minimum window width for resizing
pub const MIN_WINDOW_WIDTH: u32 = 100;
/// Minimum window height for resizing
pub const MIN_WINDOW_HEIGHT: u32 = 50;

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

    /// Alias for intersects() - checks if rectangles overlap
    #[inline]
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.intersects(other)
    }

    /// Calculate the bounding box that contains both rectangles
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let x2 = (self.x + self.width as i32).max(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).max(other.y + other.height as i32);

        Rect::new(x, y, (x2 - x) as u32, (y2 - y) as u32)
    }

    /// Get the area of the rectangle
    #[inline]
    pub fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Check if this rectangle is empty (zero width or height)
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Get the right edge (x + width)
    #[inline]
    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    /// Get the bottom edge (y + height)
    #[inline]
    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    /// Calculate new bounds when resizing by dragging an edge.
    /// Enforces minimum size constraints.
    pub fn resize_edge(
        &self,
        edge: ResizeEdge,
        delta_x: i32,
        delta_y: i32,
        min_width: u32,
        min_height: u32,
    ) -> Rect {
        let (mut x, mut y, mut w, mut h) = (self.x, self.y, self.width, self.height);

        match edge {
            ResizeEdge::Top => {
                // Top edge: moving up increases height, moving down decreases
                let new_height = (h as i32 - delta_y).max(min_height as i32) as u32;
                let actual_delta = h as i32 - new_height as i32;
                y += actual_delta;
                h = new_height;
            }
            ResizeEdge::Bottom => {
                // Bottom edge: moving down increases height
                h = (h as i32 + delta_y).max(min_height as i32) as u32;
            }
            ResizeEdge::Left => {
                // Left edge: moving left increases width, moving right decreases
                let new_width = (w as i32 - delta_x).max(min_width as i32) as u32;
                let actual_delta = w as i32 - new_width as i32;
                x += actual_delta;
                w = new_width;
            }
            ResizeEdge::Right => {
                // Right edge: moving right increases width
                w = (w as i32 + delta_x).max(min_width as i32) as u32;
            }
            ResizeEdge::TopLeft => {
                // Corner: combine top and left behaviors
                let new_width = (w as i32 - delta_x).max(min_width as i32) as u32;
                let new_height = (h as i32 - delta_y).max(min_height as i32) as u32;
                let actual_dx = w as i32 - new_width as i32;
                let actual_dy = h as i32 - new_height as i32;
                x += actual_dx;
                y += actual_dy;
                w = new_width;
                h = new_height;
            }
            ResizeEdge::TopRight => {
                // Corner: combine top and right behaviors
                let new_height = (h as i32 - delta_y).max(min_height as i32) as u32;
                let actual_dy = h as i32 - new_height as i32;
                y += actual_dy;
                h = new_height;
                w = (w as i32 + delta_x).max(min_width as i32) as u32;
            }
            ResizeEdge::BottomLeft => {
                // Corner: combine bottom and left behaviors
                let new_width = (w as i32 - delta_x).max(min_width as i32) as u32;
                let actual_dx = w as i32 - new_width as i32;
                x += actual_dx;
                w = new_width;
                h = (h as i32 + delta_y).max(min_height as i32) as u32;
            }
            ResizeEdge::BottomRight => {
                // Corner: combine bottom and right behaviors
                w = (w as i32 + delta_x).max(min_width as i32) as u32;
                h = (h as i32 + delta_y).max(min_height as i32) as u32;
            }
        }

        Rect::new(x, y, w, h)
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

/// Cached layout information for a window.
///
/// This stores the computed global bounds so we don't need to recalculate
/// parent offsets on every operation.
#[derive(Debug, Clone, Copy)]
pub struct WindowLayout {
    /// The window's local bounds (relative to parent)
    pub local_bounds: Rect,
    /// The window's global bounds (absolute screen coordinates)
    pub global_bounds: Rect,
    /// Whether the layout needs recalculation
    pub dirty: bool,
}

impl WindowLayout {
    /// Create a new layout with the given local bounds.
    pub fn new(local_bounds: Rect) -> Self {
        WindowLayout {
            local_bounds,
            global_bounds: local_bounds, // Initially same as local
            dirty: true,
        }
    }

    /// Update the global bounds based on parent offset.
    pub fn update_global(&mut self, parent_x: i32, parent_y: i32) {
        self.global_bounds = Rect::new(
            self.local_bounds.x + parent_x,
            self.local_bounds.y + parent_y,
            self.local_bounds.width,
            self.local_bounds.height,
        );
        self.dirty = false;
    }

    /// Mark the layout as needing recalculation.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }
}

/// Which edge of a window is being resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Current interaction state for window dragging/resizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionState {
    /// No active interaction
    Idle,
    /// Dragging a window by its title bar
    Dragging {
        /// Window being dragged
        window: WindowId,
        /// Where the drag started (mouse position)
        start_mouse: Point,
        /// Original window position when drag started
        start_window: Point,
    },
    /// Resizing a window by dragging an edge
    Resizing {
        /// Window being resized
        window: WindowId,
        /// Which edge is being dragged
        edge: ResizeEdge,
        /// Where the resize started (mouse position)
        start_mouse: Point,
        /// Original window bounds when resize started
        start_bounds: Rect,
    },
}

impl Default for InteractionState {
    fn default() -> Self {
        InteractionState::Idle
    }
}

/// Hit test result for determining what part of a window was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTestResult {
    /// Not on this window
    None,
    /// On the title bar (draggable)
    TitleBar,
    /// On the client/content area
    Client,
    /// On a border (resizable)
    Border(ResizeEdge),
    /// On the close button
    CloseButton,
    /// On the minimize button
    MinimizeButton,
    /// On the maximize button
    MaximizeButton,
}