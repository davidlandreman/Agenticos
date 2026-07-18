//! Text window for grid-based text rendering

use alloc::{vec, vec::Vec};
use alloc::string::ToString;
use crate::window::{Window, Rect, Event, EventResult, GraphicsDevice};
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use super::base::WindowBase;

/// A character cell in the text grid
#[derive(Clone, Copy)]
struct CharCell {
    ch: char,
    fg_color: Color,
    bg_color: Color,
}

impl Default for CharCell {
    fn default() -> Self {
        CharCell {
            ch: ' ',
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
        }
    }
}

/// A window that renders text in a grid
pub struct TextWindow {
    /// Base window functionality
    base: WindowBase,
    /// Text buffer organized as rows and columns
    buffer: Vec<Vec<CharCell>>,
    /// Number of columns
    cols: usize,
    /// Number of rows
    rows: usize,
    /// Cursor position
    cursor_x: usize,
    cursor_y: usize,
    /// Current foreground color
    current_fg: Color,
    /// Current background color
    current_bg: Color,
    /// Character dimensions
    char_width: usize,
    char_height: usize,
    /// Track which cells have been modified since last paint
    dirty_cells: Vec<(usize, usize)>,
    /// Whether to do incremental updates or full repaint
    incremental_updates: bool,
    /// Suppress invalidation (used during paint to prevent re-invalidation)
    suppress_invalidation: bool,
}

impl TextWindow {
    /// Process any pending console output
    pub fn process_console_output(&mut self) {
        let (lines, pending) = crate::window::console::take_output();
        if !lines.is_empty() || !pending.is_empty() {
            crate::debug_info!("TextWindow: Processing {} lines and pending: '{}'", lines.len(), pending);

            // Suppress invalidation during console output processing
            // This prevents re-invalidation during paint
            self.suppress_invalidation = true;

            for (i, line) in lines.iter().enumerate() {
                crate::debug_info!("  Line {}: '{}'", i, line);
                self.write_str(&line);
                self.newline();
            }
            if !pending.is_empty() {
                crate::debug_info!("  Pending: '{}'", pending);
                self.write_str(&pending);
            }

            self.suppress_invalidation = false;

            // Don't re-invalidate here - we're already painting!
            // The dirty_cells tracking will handle what needs updating
        }
    }
    
    /// Create a new text window with a specific ID
    pub fn new_with_id(id: crate::window::WindowId, bounds: Rect) -> Self {
        let font = get_default_font();
        let char_width = font.cell_width() as usize;
        let char_height = font.line_height() as usize;

        // Calculate grid dimensions
        let cols = (bounds.width as usize) / char_width;
        let rows = (bounds.height as usize) / char_height;

        // Initialize buffer
        let buffer = vec![vec![CharCell::default(); cols]; rows];

        let mut base = WindowBase::new_with_id(id, bounds);
        base.set_can_focus(true); // Text windows can receive focus

        TextWindow {
            base,
            buffer,
            cols,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            current_fg: Color::WHITE,
            current_bg: Color::BLACK,
            char_width,
            char_height,
            dirty_cells: Vec::new(),
            // Start in full-repaint mode so the first paint fills the
            // dark-grey terminal background across the whole bounds, not
            // just the cells of any startup text the caller writes
            // before the first frame. Once the full repaint runs once,
            // it flips this to true and incremental updates take over.
            incremental_updates: false,
            suppress_invalidation: false,
        }
    }

    /// Create a new text window (generates its own ID)

    /// Write a character at the cursor position
    pub fn write_char(&mut self, ch: char) {
        crate::debug_trace!("TextWindow::write_char called with '{}'", ch);
        
        if ch == '\n' {
            self.newline();
            return;
        }
        
        if ch == '\r' {
            self.cursor_x = 0;
            return;
        }
        
        if self.cursor_x < self.cols && self.cursor_y < self.rows {
            crate::debug_trace!("Writing '{}' at ({}, {})", ch, self.cursor_x, self.cursor_y);
            self.buffer[self.cursor_y][self.cursor_x] = CharCell {
                ch,
                fg_color: self.current_fg,
                bg_color: self.current_bg,
            };
            
            // Track this cell as dirty for incremental updates
            if self.incremental_updates {
                self.dirty_cells.push((self.cursor_x, self.cursor_y));
                // Only invalidate if we have a reasonable number of dirty cells
                // Otherwise do a full repaint
                if self.dirty_cells.len() > 100 {
                    self.incremental_updates = false;
                    self.dirty_cells.clear();
                }
            }
            
            self.cursor_x += 1;

            if self.cursor_x >= self.cols {
                self.newline();
            }

            // Only invalidate if not suppressed (e.g., during paint)
            if !self.suppress_invalidation {
                self.base.invalidate();
                crate::debug_trace!("Window invalidated after write_char");
            }
        } else {
            crate::debug_warn!("Cursor out of bounds: ({}, {}) max: ({}, {})", 
                self.cursor_x, self.cursor_y, self.cols, self.rows);
        }
    }
    
    /// Write a string at the cursor position
    pub fn write_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.write_char(ch);
        }
    }
    
    /// Move to a new line
    pub fn newline(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        
        if self.cursor_y >= self.rows {
            // Scroll up
            self.scroll_up();
            self.cursor_y = self.rows - 1;
        }
    }
    
    /// Scroll the buffer up by one line
    fn scroll_up(&mut self) {
        // Remove first line and add empty line at bottom
        self.buffer.remove(0);
        self.buffer.push(vec![CharCell::default(); self.cols]);
        // Scrolling requires full repaint
        self.incremental_updates = false;
        self.dirty_cells.clear();
        if !self.suppress_invalidation {
            self.base.invalidate();
        }
    }
    
    /// Clear the text window

    /// Grid dimensions in cells: `Some((rows, cols))`. Returned as
    /// `Option` to match the `Window::grid_size` trait signature so
    /// callers can use either form interchangeably.
    pub fn grid_size_opt(&self) -> Option<(u16, u16)> {
        Some((self.rows as u16, self.cols as u16))
    }

    /// Set cursor position

    /// Set text colors

    /// Overwrite a single cell with explicit content + colors. Bypasses
    /// `write_char`'s cursor-advance / autowrap behavior — used by the
    /// `Screen` → `TextWindow` sync path (U9) where the terminal's
    /// `Screen` is the source of truth and TextWindow is just the
    /// renderer. The cursor position is not touched; callers manage it
    /// separately via `set_cursor_position`.
    pub fn set_cell(&mut self, row: usize, col: usize, ch: char, fg: Color, bg: Color) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        self.buffer[row][col] = CharCell {
            ch,
            fg_color: fg,
            bg_color: bg,
        };
        // Forcing full repaint is the simplest correct policy when an
        // external source overwrites many cells in a single batch.
        // The sync path repaints the entire grid each frame, so granular
        // dirty tracking would be lost work.
        self.incremental_updates = false;
        self.dirty_cells.clear();
        if !self.suppress_invalidation {
            self.base.invalidate();
        }
    }
    
    /// Get current cursor position
    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }
    
    /// Set cursor position
    pub fn set_cursor_position(&mut self, x: usize, y: usize) {
        if x < self.cols && y < self.rows {
            self.cursor_x = x;
            self.cursor_y = y;
            if !self.suppress_invalidation {
                self.base.invalidate();
            }
        }
    }
    
    }

impl Window for TextWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    // Custom override: TextWindow recomputes its grid dimensions and
    // reallocates the buffer when bounds change.
    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        // Recalculate grid dimensions when bounds change
        let bounds = self.base.bounds();
        let font = crate::graphics::fonts::core_font::get_default_font();
        self.char_width = font.cell_width() as usize;
        self.char_height = font.line_height() as usize;
        let new_cols = bounds.width as usize / self.char_width;
        let new_rows = bounds.height as usize / self.char_height;

        // Only reallocate if dimensions actually changed
        let old_rows = self.buffer.len();
        let old_cols = if old_rows > 0 { self.buffer[0].len() } else { 0 };

        if new_rows != old_rows || new_cols != old_cols {
            // Create new buffer and preserve existing content
            let mut new_buffer = vec![vec![CharCell::default(); new_cols]; new_rows];

            // Copy existing content (as much as fits)
            let copy_rows = old_rows.min(new_rows);
            let copy_cols = old_cols.min(new_cols);
            for row in 0..copy_rows {
                for col in 0..copy_cols {
                    new_buffer[row][col] = self.buffer[row][col];
                }
            }

            self.buffer = new_buffer;
            self.cols = new_cols;
            self.rows = new_rows;

            // Adjust cursor if now out of bounds
            if self.cursor_x >= self.cols {
                self.cursor_x = self.cols.saturating_sub(1);
            }
            if self.cursor_y >= self.rows {
                self.cursor_y = self.rows.saturating_sub(1);
            }

            // Clear dirty cells and force full repaint
            self.dirty_cells.clear();
            self.incremental_updates = false;
        }
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }
        
        crate::debug_trace!("TextWindow::paint - bounds: {:?}", self.bounds());
        crate::debug_trace!("TextWindow buffer size: {}x{}, cursor at ({}, {})", 
            self.cols, self.rows, self.cursor_x, self.cursor_y);
        
        // REMOVED: Console output processing - this should be done elsewhere,
        // not on every paint! This was causing the prompt to be reprinted
        // every time the window was painted.
        
        let bounds = self.bounds();
        let font = get_default_font();
        
        // Check if we can do incremental update
        crate::debug_trace!("TextWindow paint: incremental_updates={}, dirty_cells={}, needs_repaint={}",
            self.incremental_updates, self.dirty_cells.len(), self.base.needs_repaint());

        // Choose paint mode. The Window::paint contract requires us to
        // produce correct pixels for everything in the device's clip.
        //
        // Incremental is only safe when the compositor's clip equals
        // (or is a subset of) our own bounds — i.e. the only reason
        // we're being painted is because *we* invalidated. We use
        // `needs_repaint` as a proxy: if it's set, the dirty rect
        // includes our bounds (mark_dirty_for_invalidated_windows added
        // it), and the per-region compositor pass for that rect produces
        // a clip ⊇ all our dirty_cells. If `needs_repaint` is false, the
        // compositor is calling us because some *other* dirty rect
        // intersects our bounds (e.g. a drag passed over us); the clip
        // there can be narrower than our bounds and contain pixels that
        // were just overwritten by the desktop, so only a full repaint
        // produces correct output.
        let can_incremental = self.incremental_updates
            && self.base.needs_repaint()
            && !self.dirty_cells.is_empty();

        if can_incremental {
            crate::debug_info!("TextWindow: Incremental update for {} dirty cells", self.dirty_cells.len());

            // Compute the bounding cell range covered by the changes
            // (and the cursor cell when focused). This is the same range
            // `dirty_rect_hint` published to the compositor; the desktop
            // has already blitted wallpaper over it during this frame's
            // per-region pass, so EVERY cell inside the range — not just
            // the ones in `dirty_cells` — needs to be redrawn or the
            // wallpaper will bleed through any unchanged-but-in-rect
            // cells (e.g. the empty area to the right of a short prompt
            // when the previous line had a longer message).
            let (mut min_col, mut min_row, mut max_col_excl, mut max_row_excl) =
                (usize::MAX, usize::MAX, 0usize, 0usize);
            for &(cx, cy) in &self.dirty_cells {
                min_col = min_col.min(cx);
                min_row = min_row.min(cy);
                max_col_excl = max_col_excl.max(cx + 1);
                max_row_excl = max_row_excl.max(cy + 1);
            }
            if self.has_focus() && self.cursor_x < self.cols && self.cursor_y < self.rows {
                min_col = min_col.min(self.cursor_x);
                min_row = min_row.min(self.cursor_y);
                max_col_excl = max_col_excl.max(self.cursor_x + 1);
                max_row_excl = max_row_excl.max(self.cursor_y + 1);
            }

            // Defensive — `can_incremental` already guarantees a non-empty
            // dirty_cells, so the range is non-empty too. Bail to the full
            // path on any pathological input rather than divide-by-zero.
            if max_col_excl <= min_col || max_row_excl <= min_row {
                self.dirty_cells.clear();
            } else {
                // Repaint the bounding rect background first so any
                // unchanged-cells whose pixels were just clobbered by
                // wallpaper get the dark-grey terminal background back.
                let fill_x = bounds.x + (min_col * self.char_width) as i32;
                let fill_y = bounds.y + (min_row * self.char_height) as i32;
                let fill_w = ((max_col_excl - min_col) * self.char_width) as u32;
                let fill_h = ((max_row_excl - min_row) * self.char_height) as u32;
                device.fill_rect(fill_x, fill_y, fill_w, fill_h, Color::new(32, 32, 32));

                for row in min_row..max_row_excl {
                    for col in min_col..max_col_excl {
                        let x = bounds.x + (col * self.char_width) as i32;
                        let y = bounds.y + (row * self.char_height) as i32;
                        let cell = &self.buffer[row][col];

                        // Cells with bg == BLACK use the default
                        // terminal grey from the bounding-rect fill
                        // above; only paint a per-cell bg when the
                        // cell explicitly overrides it.
                        if cell.bg_color != Color::BLACK {
                            device.fill_rect(
                                x,
                                y,
                                self.char_width as u32,
                                self.char_height as u32,
                                cell.bg_color,
                            );
                        }

                        if cell.ch != ' ' {
                            device.draw_text(x, y, &cell.ch.to_string(), font.as_font(), cell.fg_color);
                        }
                    }
                }

                self.dirty_cells.clear();

                if self.has_focus() && self.cursor_x < self.cols && self.cursor_y < self.rows {
                    let cursor_x = bounds.x + (self.cursor_x * self.char_width) as i32;
                    let cursor_y = bounds.y + (self.cursor_y * self.char_height) as i32;

                    device.fill_rect(
                        cursor_x,
                        cursor_y + self.char_height as i32 - 2,
                        self.char_width as u32,
                        2,
                        Color::WHITE,
                    );
                }

                self.base.clear_needs_repaint();
                return;
            }
        }

        // Full repaint — either we have no internal dirty state to
        // optimize, or we were called for an external reason and must
        // redraw everything in clip.
        crate::debug_info!("TextWindow: Full repaint");

        // Clear dirty cells since we're doing a full repaint
        self.dirty_cells.clear();

        // Clear background with a dark grey instead of black to see if it's rendering
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            Color::new(32, 32, 32),
        );

        // Count non-space characters for debugging
        let mut char_count = 0;

        // Render each character
        for (row, line) in self.buffer.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                let x = bounds.x + (col * self.char_width) as i32;
                let y = bounds.y + (row * self.char_height) as i32;

                // Draw background if not black
                if cell.bg_color != Color::BLACK {
                    device.fill_rect(
                        x,
                        y,
                        self.char_width as u32,
                        self.char_height as u32,
                        cell.bg_color,
                    );
                }

                // Draw character
                if cell.ch != ' ' {
                    char_count += 1;
                    if row < 15 {  // Log more chars for debugging
                        crate::debug_trace!("Drawing '{}' at screen ({}, {}) buffer ({}, {})",
                            cell.ch, x, y, col, row);
                    }
                    device.draw_text(x, y, &cell.ch.to_string(), font.as_font(), cell.fg_color);
                }
            }
        }

        crate::debug_info!("TextWindow: Drew {} non-space characters", char_count);

        // Draw cursor if focused
        if self.has_focus() && self.cursor_x < self.cols && self.cursor_y < self.rows {
            let cursor_x = bounds.x + (self.cursor_x * self.char_width) as i32;
            let cursor_y = bounds.y + (self.cursor_y * self.char_height) as i32;

            // Draw cursor as a filled rectangle
            device.fill_rect(
                cursor_x,
                cursor_y + self.char_height as i32 - 2,
                self.char_width as u32,
                2,
                Color::WHITE,
            );
        }

        // Re-enable incremental updates after full repaint
        self.incremental_updates = true;

        self.base.clear_needs_repaint();
        crate::debug_trace!("TextWindow paint complete, needs_repaint cleared");
    }

    /// Narrow dirty hint when only a few cells changed (typing): the
    /// bounding box of `dirty_cells`, expanded to cover the cursor cell
    /// when focused (so the cursor's old / new positions both fall inside
    /// the dirty region the compositor publishes — otherwise the desktop's
    /// per-region wallpaper blit covers the whole TextWindow bounds and
    /// our incremental paint can't restore the surrounding cells).
    ///
    /// Returns `None` when the incremental path is disabled or there are
    /// no per-cell dirty marks, falling back to the full-bounds default.
    fn dirty_rect_hint(&self) -> Option<Rect> {
        if !self.incremental_updates || self.dirty_cells.is_empty() {
            return None;
        }

        // Bounding cell range of dirty_cells, plus the cursor cell when
        // focused. (cursor_x, cursor_y) may equal an existing dirty cell
        // — that just collapses into the same range.
        let mut min_col = usize::MAX;
        let mut min_row = usize::MAX;
        let mut max_col_excl = 0usize;
        let mut max_row_excl = 0usize;
        for &(cx, cy) in &self.dirty_cells {
            min_col = min_col.min(cx);
            min_row = min_row.min(cy);
            max_col_excl = max_col_excl.max(cx + 1);
            max_row_excl = max_row_excl.max(cy + 1);
        }
        if self.has_focus() && self.cursor_x < self.cols && self.cursor_y < self.rows {
            min_col = min_col.min(self.cursor_x);
            min_row = min_row.min(self.cursor_y);
            max_col_excl = max_col_excl.max(self.cursor_x + 1);
            max_row_excl = max_row_excl.max(self.cursor_y + 1);
        }

        if max_col_excl <= min_col || max_row_excl <= min_row {
            return None;
        }

        Some(Rect::new(
            (min_col * self.char_width) as i32,
            (min_row * self.char_height) as i32,
            ((max_col_excl - min_col) * self.char_width) as u32,
            ((max_row_excl - min_row) * self.char_height) as u32,
        ))
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(key_event) => {
                if key_event.pressed {
                    // TODO: Handle keyboard input
                    // For now, just mark as handled if we have focus
                    if self.has_focus() {
                        return EventResult::Handled;
                    }
                }
            }
            _ => {}
        }

        EventResult::Propagate
    }

    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }

    fn grid_size(&self) -> Option<(u16, u16)> {
        self.grid_size_opt()
    }
}
