//! Compositor for managing dirty regions and overlays.
//!
//! The compositor handles:
//! - Dirty rectangle tracking and merging
//! - Cursor overlay with proper background save/restore
//! - Frame lifecycle (begin_frame/end_frame)

use crate::window::types::Rect;
use alloc::vec::Vec;

/// Maximum number of dirty regions to track before forcing full repaint.
const MAX_DIRTY_REGIONS: usize = 16;

/// Threshold area ratio - if dirty area exceeds this fraction of screen, do full repaint.
///
/// Only consulted when there are *multiple* dirty rects: a single big rect is
/// always cheaper to paint as a partial than a full-screen clear-and-redraw,
/// so it bypasses this check (see `check_area_threshold`). The threshold
/// guards against the multi-rect degenerate case where many small rects
/// scattered across the screen would each pay per-rect overhead during the
/// render walk; at that point, just clearing once and repainting is cheaper.
const FULL_REPAINT_THRESHOLD: f32 = 0.85;

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
    /// If full repaint is needed, yields a single rect covering the screen
    /// and then ends. Otherwise yields each tracked dirty rect once. The
    /// iterator yields owned `Rect` values (cheap — `Rect` is `Copy`) so
    /// callers can hold the iterator across borrows of the manager.
    pub fn dirty_regions(&self) -> impl Iterator<Item = Rect> + '_ {
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
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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
    ///
    /// Skipped when there's only one rect: a single dirty region — even if
    /// it covers most of the screen — is always cheaper to paint as a
    /// partial than to clear-and-redraw the whole framebuffer, because the
    /// partial path skips windows whose bounds don't intersect the rect.
    /// The threshold exists to bound per-rect overhead in the multi-rect
    /// case, not to short-circuit a single big update (e.g. a window drag).
    fn check_area_threshold(&mut self) {
        if self.regions.len() <= 1 {
            return;
        }

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
    type Item = Rect;

    fn next(&mut self) -> Option<Self::Item> {
        if self.manager.force_full_repaint {
            if self.returned_full {
                None
            } else {
                self.returned_full = true;
                Some(Rect::new(
                    0,
                    0,
                    self.manager.screen_width,
                    self.manager.screen_height,
                ))
            }
        } else if self.index < self.manager.regions.len() {
            let rect = self.manager.regions[self.index];
            self.index += 1;
            Some(rect)
        } else {
            None
        }
    }
}

/// Compositor that coordinates dirty tracking and overlays.
pub struct Compositor {
    /// Dirty region manager
    pub dirty: DirtyRectManager,
    cursor_position: (usize, usize),
}

impl Compositor {
    /// Create a new compositor for the given screen dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Compositor {
            dirty: DirtyRectManager::new(width, height),
            cursor_position: (0, 0),
        }
    }

    /// End the current frame.
    ///
    /// Call this after all drawing is complete.
    pub fn end_frame(&mut self) {
        self.dirty.clear();
    }

    /// Check if anything needs rendering this frame.
    pub fn needs_render(&self) -> bool {
        self.dirty.is_dirty()
    }

    /// Return the last pointer position recorded by the compositor.
    pub const fn cursor_position(&self) -> (usize, usize) {
        self.cursor_position
    }

    /// Record that the cursor has moved.
    ///
    /// Returns true if the cursor actually moved. Does **not** mark the
    /// old/new cursor footprints dirty: `CursorRenderer` already restores
    /// the saved-background pixels at the old position and re-saves at the
    /// new position every frame, so the windows underneath the cursor do
    /// not need to repaint to keep their pixels intact. Marking the
    /// cursor's footprint dirty was a regression — it caused every mouse
    /// motion to trigger a wallpaper-blit + child-window-repaint chain and,
    /// after dropping eager parent repaint propagation, left wallpaper
    /// bleeding through clean child windows.
    pub fn update_cursor(&mut self, new_x: usize, new_y: usize) -> bool {
        let new_position = (new_x, new_y);
        if self.cursor_position == new_position {
            return false;
        }
        self.cursor_position = new_position;
        true
    }
}
