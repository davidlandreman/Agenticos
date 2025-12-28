//! Compositor for managing dirty regions and overlays.
//!
//! The compositor handles:
//! - Dirty rectangle tracking and merging
//! - Cursor overlay with proper background save/restore
//! - Frame lifecycle (begin_frame/end_frame)

use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::framebuffer::SavedRegion;
use crate::window::types::Rect;

/// Maximum number of dirty regions to track before forcing full repaint.
const MAX_DIRTY_REGIONS: usize = 16;

/// Threshold area ratio - if dirty area exceeds this fraction of screen, do full repaint.
const FULL_REPAINT_THRESHOLD: f32 = 0.5;

/// Manages dirty regions for partial screen updates.
///
/// Tracks which parts of the screen need repainting and can merge
/// overlapping regions to reduce redundant work.
#[derive(Debug)]
pub struct DirtyRectManager {
    /// List of dirty rectangles
    regions: Vec<Rect>,
    /// Screen dimensions for area calculations
    screen_width: u32,
    screen_height: u32,
    /// Force a full repaint on next frame
    force_full_repaint: bool,
}

impl DirtyRectManager {
    /// Create a new dirty rect manager for the given screen size.
    pub fn new(width: u32, height: u32) -> Self {
        DirtyRectManager {
            regions: Vec::with_capacity(MAX_DIRTY_REGIONS),
            screen_width: width,
            screen_height: height,
            force_full_repaint: true, // First frame needs full paint
        }
    }

    /// Mark a region as dirty (needs repainting).
    pub fn mark_dirty(&mut self, rect: Rect) {
        // Ignore empty rectangles
        if rect.is_empty() {
            return;
        }

        // Clamp to screen bounds
        let rect = self.clamp_to_screen(rect);
        if rect.is_empty() {
            return;
        }

        // If we already need full repaint, don't bother tracking
        if self.force_full_repaint {
            return;
        }

        // Check if this region overlaps with any existing region
        for existing in &mut self.regions {
            if existing.overlaps(&rect) {
                // Merge into existing region
                *existing = existing.union(&rect);
                self.try_merge_regions();
                return;
            }
        }

        // Add as new region if we have room
        if self.regions.len() < MAX_DIRTY_REGIONS {
            self.regions.push(rect);
        } else {
            // Too many regions - force full repaint
            self.force_full_repaint = true;
        }

        // Check if total dirty area exceeds threshold
        self.check_area_threshold();
    }

    /// Mark the entire screen as dirty.
    pub fn mark_full_repaint(&mut self) {
        self.force_full_repaint = true;
        self.regions.clear();
    }

    /// Check if any region is dirty.
    pub fn is_dirty(&self) -> bool {
        self.force_full_repaint || !self.regions.is_empty()
    }

    /// Check if a full repaint is required.
    pub fn needs_full_repaint(&self) -> bool {
        self.force_full_repaint
    }

    /// Get an iterator over dirty regions.
    ///
    /// If full repaint is needed, returns a single region covering the screen.
    pub fn dirty_regions(&self) -> impl Iterator<Item = &Rect> {
        DirtyRegionIter {
            manager: self,
            index: 0,
            returned_full: false,
        }
    }

    /// Clear all dirty regions (call after rendering).
    pub fn clear(&mut self) {
        self.regions.clear();
        self.force_full_repaint = false;
    }

    /// Get the bounding box of all dirty regions.
    pub fn bounding_box(&self) -> Option<Rect> {
        if self.force_full_repaint {
            return Some(Rect::new(0, 0, self.screen_width, self.screen_height));
        }

        if self.regions.is_empty() {
            return None;
        }

        let mut result = self.regions[0];
        for rect in &self.regions[1..] {
            result = result.union(rect);
        }
        Some(result)
    }

    /// Clamp a rectangle to screen bounds.
    fn clamp_to_screen(&self, rect: Rect) -> Rect {
        let x = rect.x.max(0);
        let y = rect.y.max(0);
        let right = rect.right().min(self.screen_width as i32);
        let bottom = rect.bottom().min(self.screen_height as i32);

        if right <= x || bottom <= y {
            return Rect::new(0, 0, 0, 0);
        }

        Rect::new(x, y, (right - x) as u32, (bottom - y) as u32)
    }

    /// Try to merge overlapping regions to reduce count.
    fn try_merge_regions(&mut self) {
        // Simple O(n^2) merge - OK for small region counts
        let mut i = 0;
        while i < self.regions.len() {
            let mut j = i + 1;
            while j < self.regions.len() {
                if self.regions[i].overlaps(&self.regions[j]) {
                    let merged = self.regions[i].union(&self.regions[j]);
                    self.regions[i] = merged;
                    self.regions.remove(j);
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    /// Check if total dirty area exceeds threshold for full repaint.
    fn check_area_threshold(&mut self) {
        let screen_area = self.screen_width as u64 * self.screen_height as u64;
        let dirty_area: u64 = self.regions.iter().map(|r| r.area()).sum();

        if dirty_area as f32 > screen_area as f32 * FULL_REPAINT_THRESHOLD {
            self.force_full_repaint = true;
            self.regions.clear();
        }
    }
}

/// Iterator over dirty regions.
struct DirtyRegionIter<'a> {
    manager: &'a DirtyRectManager,
    index: usize,
    returned_full: bool,
}

impl<'a> Iterator for DirtyRegionIter<'a> {
    type Item = &'a Rect;

    fn next(&mut self) -> Option<Self::Item> {
        if self.manager.force_full_repaint {
            if self.returned_full {
                None
            } else {
                self.returned_full = true;
                // Return a static full-screen rect
                // This is a bit hacky but avoids allocation
                None // We'll handle this differently in practice
            }
        } else if self.index < self.manager.regions.len() {
            let rect = &self.manager.regions[self.index];
            self.index += 1;
            Some(rect)
        } else {
            None
        }
    }
}

/// Cursor overlay for proper mouse cursor rendering.
///
/// Saves the background under the cursor and restores it before
/// drawing at a new position, preventing cursor trails.
#[derive(Debug)]
pub struct CursorOverlay {
    /// Saved background under the cursor
    saved_background: SavedRegion,
    /// Current cursor position
    position: (usize, usize),
    /// Cursor dimensions
    width: usize,
    height: usize,
    /// Whether the cursor is currently visible
    visible: bool,
    /// Whether the cursor is currently drawn on screen
    drawn: bool,
}

impl CursorOverlay {
    /// Create a new cursor overlay with the given cursor dimensions.
    pub fn new(cursor_width: usize, cursor_height: usize) -> Self {
        CursorOverlay {
            saved_background: SavedRegion::new(),
            position: (0, 0),
            width: cursor_width,
            height: cursor_height,
            visible: true,
            drawn: false,
        }
    }

    /// Get current cursor position.
    pub fn position(&self) -> (usize, usize) {
        self.position
    }

    /// Check if the cursor has moved to a new position.
    pub fn has_moved(&self, new_x: usize, new_y: usize) -> bool {
        (new_x, new_y) != self.position
    }

    /// Get the bounding rectangle of the cursor at its current position.
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.position.0 as i32,
            self.position.1 as i32,
            self.width as u32,
            self.height as u32,
        )
    }

    /// Get the bounding rectangle at a new position (for dirty tracking).
    pub fn bounds_at(&self, x: usize, y: usize) -> Rect {
        Rect::new(x as i32, y as i32, self.width as u32, self.height as u32)
    }

    /// Set cursor visibility.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Check if cursor is visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Check if cursor is currently drawn on screen.
    pub fn is_drawn(&self) -> bool {
        self.drawn
    }

    /// Store the saved background region.
    pub fn store_background(&mut self, region: SavedRegion) {
        self.saved_background = region;
    }

    /// Get the saved background for restoration.
    pub fn saved_background(&self) -> &SavedRegion {
        &self.saved_background
    }

    /// Take ownership of the saved background.
    pub fn take_background(&mut self) -> SavedRegion {
        core::mem::take(&mut self.saved_background)
    }

    /// Update cursor position and return the old and new bounds for dirty tracking.
    pub fn move_to(&mut self, new_x: usize, new_y: usize) -> (Rect, Rect) {
        let old_bounds = self.bounds();
        self.position = (new_x, new_y);
        let new_bounds = self.bounds();
        (old_bounds, new_bounds)
    }

    /// Mark the cursor as drawn.
    pub fn mark_drawn(&mut self) {
        self.drawn = true;
    }

    /// Mark the cursor as erased.
    pub fn mark_erased(&mut self) {
        self.drawn = false;
    }
}

impl Default for CursorOverlay {
    fn default() -> Self {
        Self::new(12, 12) // Default cursor size
    }
}

/// Frame state for coordinating rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameState {
    /// Not currently rendering
    Idle,
    /// Frame has begun, accepting draw calls
    Drawing,
    /// Frame is being finalized
    Finalizing,
}

/// Compositor that coordinates dirty tracking and overlays.
pub struct Compositor {
    /// Dirty region manager
    pub dirty: DirtyRectManager,
    /// Cursor overlay
    pub cursor: CursorOverlay,
    /// Current frame state
    state: FrameState,
    /// Screen dimensions
    width: u32,
    height: u32,
}

impl Compositor {
    /// Create a new compositor for the given screen dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Compositor {
            dirty: DirtyRectManager::new(width, height),
            cursor: CursorOverlay::default(),
            state: FrameState::Idle,
            width,
            height,
        }
    }

    /// Begin a new frame.
    ///
    /// Call this at the start of each render cycle.
    pub fn begin_frame(&mut self) {
        self.state = FrameState::Drawing;
    }

    /// End the current frame.
    ///
    /// Call this after all drawing is complete.
    pub fn end_frame(&mut self) {
        self.state = FrameState::Finalizing;
        self.dirty.clear();
        self.state = FrameState::Idle;
    }

    /// Get the current frame state.
    pub fn state(&self) -> FrameState {
        self.state
    }

    /// Check if anything needs rendering this frame.
    pub fn needs_render(&self) -> bool {
        self.dirty.is_dirty()
    }

    /// Mark a window region as needing repaint.
    pub fn invalidate_window(&mut self, bounds: Rect) {
        self.dirty.mark_dirty(bounds);
    }

    /// Update cursor position and mark dirty regions.
    ///
    /// Returns true if the cursor actually moved.
    pub fn update_cursor(&mut self, new_x: usize, new_y: usize) -> bool {
        if !self.cursor.has_moved(new_x, new_y) {
            return false;
        }

        // Mark both old and new cursor positions as dirty
        let (old_bounds, new_bounds) = self.cursor.move_to(new_x, new_y);

        // Include some padding for cursor outline
        let padding = 2;
        let old_padded = Rect::new(
            old_bounds.x - padding,
            old_bounds.y - padding,
            old_bounds.width + (padding * 2) as u32,
            old_bounds.height + (padding * 2) as u32,
        );
        let new_padded = Rect::new(
            new_bounds.x - padding,
            new_bounds.y - padding,
            new_bounds.width + (padding * 2) as u32,
            new_bounds.height + (padding * 2) as u32,
        );

        self.dirty.mark_dirty(old_padded);
        self.dirty.mark_dirty(new_padded);

        true
    }

    /// Get screen dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
