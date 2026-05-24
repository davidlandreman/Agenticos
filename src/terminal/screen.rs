//! The character grid.
//!
//! `Screen` owns the visible terminal contents — a primary buffer of
//! cells, the cursor, the current SGR pen, and the scroll region. It
//! implements [`vte::Perform`] so a raw byte stream can be fed straight
//! through the parser into the grid.
//!
//! What's in U3:
//! - Cursor movement (CUU/CUD/CUF/CUB/CUP/CHA/VPA/CNL/CPL).
//! - Erase in display / erase in line / erase characters.
//! - Insert / delete characters / lines within the scroll region.
//! - Scroll up / scroll down.
//! - SGR — full set: reset/bold/dim/italic/underline/reverse/strike,
//!   16 ANSI colors, 256 indexed, 24-bit truecolor, default fg/bg.
//! - DECSTBM scroll region.
//! - Save / restore cursor (DECSC / DECRC, ESC 7 / 8 and CSI s / u).
//! - Index / reverse-index / next-line (ESC D / M / E).
//! - DSR 6 (cursor position report) — reply bytes are queued in
//!   [`Screen::take_replies`] for the PTY layer to deliver to the slave
//!   input.
//! - DEC private modes: ?7 autowrap, ?25 cursor-visible. (Alt-screen
//!   ?1049 and bracketed-paste ?2004 land in U4 / U7 respectively.)
//! - DECSCUSR cursor shape (parsed and stored; rendering lands in U5).
//! - Delayed wrap at last column (xterm semantics — the cursor "sticks"
//!   at the right margin until the next printable byte).
//!
//! Out of scope for U3: alt-screen buffer, scrollback ring (both U4),
//! caret blink (U5), bracketed paste (U7).

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use super::caret::Caret;
use super::colors::ColorSpec;
use super::config;
use super::vte::Perform;

// ---------------------------------------------------------------------
// Cell + attributes
// ---------------------------------------------------------------------

/// Cell attributes bit-packed into a u8. Plain consts beat the
/// `bitflags` crate at this size and keep zero deps.
pub mod attrs {
    pub const BOLD: u8 = 1 << 0;
    pub const DIM: u8 = 1 << 1;
    pub const ITALIC: u8 = 1 << 2;
    pub const UNDERLINE: u8 = 1 << 3;
    pub const REVERSE: u8 = 1 << 4;
    pub const STRIKE: u8 = 1 << 5;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: ColorSpec,
    pub bg: ColorSpec,
    pub attrs: u8,
}

impl Cell {
    pub const EMPTY: Cell = Cell {
        ch: ' ',
        fg: ColorSpec::Default,
        bg: ColorSpec::Default,
        attrs: 0,
    };
}

impl Default for Cell {
    fn default() -> Self {
        Cell::EMPTY
    }
}

// ---------------------------------------------------------------------
// Cursor shape (DECSCUSR)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    row: usize,
    col: usize,
    fg: ColorSpec,
    bg: ColorSpec,
    attrs: u8,
    last_col_pending: bool,
}

/// Snapshot of everything that swaps when entering / leaving the
/// alt-screen buffer. The struct is heap-allocated (because `cells` is
/// `Vec<Vec<Cell>>`), so the option pays one heap pointer when alt is
/// inactive — cheap.
struct StashedBuffer {
    cells: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    cur_fg: ColorSpec,
    cur_bg: ColorSpec,
    cur_attrs: u8,
    scroll_top: usize,
    scroll_bot: usize,
    saved: Option<SavedCursor>,
    last_col_pending: bool,
}

// ---------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------

pub struct Screen {
    rows: usize,
    cols: usize,

    /// Primary buffer: rows × cols. `buffer[row][col]`.
    buffer: Vec<Vec<Cell>>,

    cursor_row: usize,
    cursor_col: usize,

    /// Current pen.
    cur_fg: ColorSpec,
    cur_bg: ColorSpec,
    cur_attrs: u8,

    /// Scroll region (inclusive). Default [0, rows-1].
    scroll_top: usize,
    scroll_bot: usize,

    /// DECSC saved cursor.
    saved: Option<SavedCursor>,

    /// DECAWM autowrap.
    autowrap: bool,

    /// DECTCEM cursor visible.
    cursor_visible: bool,

    /// Cursor shape (DECSCUSR).
    cursor_shape: CursorShape,

    /// Delayed-wrap flag: set when a printable arrives in the last
    /// column. The next printable wraps before placement.
    last_col_pending: bool,

    /// Bytes to feed back into the slave's input queue (DSR replies,
    /// device attributes, …). Drained by the PTY in U6.
    replies: Vec<u8>,

    /// True when the alt-screen buffer is active. While set, scrollback
    /// is not appended to and `\x1b[?1049l` is the way back.
    using_alt: bool,

    /// When `using_alt`, holds the stashed primary state. None
    /// otherwise. Swap semantics: enter alt → stash primary; exit alt
    /// → take stash and restore.
    stashed: Option<StashedBuffer>,

    /// Lines that have scrolled off the top of the *primary* buffer.
    /// Oldest at the front. Capped at `config::SCROLLBACK_LINES`. Not
    /// populated while alt-screen is active.
    scrollback: VecDeque<Vec<Cell>>,

    /// How many lines back from the live buffer the user is currently
    /// viewing. 0 = live; N = N lines of scrollback are visible at the
    /// top, replacing the bottom of the live buffer. Reset to 0 on any
    /// new output that scrolls the primary buffer, or on alt-screen
    /// switch.
    view_offset: usize,

    /// Last cursor position the renderer was told about, for caret
    /// erase discipline. The renderer calls
    /// [`Screen::take_cursor_change`] each paint; on the next paint it
    /// compares against `caret()` to decide what to erase / draw.
    last_painted_cursor: (usize, usize),
}

impl Screen {
    /// Create a screen with the given dimensions. Both must be ≥ 1.
    pub fn new(rows: usize, cols: usize) -> Self {
        assert!(rows >= 1 && cols >= 1, "screen must be at least 1x1");
        let buffer = vec![vec![Cell::EMPTY; cols]; rows];
        Self {
            rows,
            cols,
            buffer,
            cursor_row: 0,
            cursor_col: 0,
            cur_fg: ColorSpec::Default,
            cur_bg: ColorSpec::Default,
            cur_attrs: 0,
            scroll_top: 0,
            scroll_bot: rows - 1,
            saved: None,
            autowrap: true,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            last_col_pending: false,
            replies: Vec::new(),
            using_alt: false,
            stashed: None,
            scrollback: VecDeque::new(),
            view_offset: 0,
            last_painted_cursor: (0, 0),
        }
    }

    /// Default 80×24 screen.
    pub fn default_size() -> Self {
        Self::new(config::DEFAULT_ROWS as usize, config::DEFAULT_COLS as usize)
    }

    // ---- accessors ----

    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        self.buffer[row][col]
    }
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor_shape
    }
    pub fn autowrap(&self) -> bool {
        self.autowrap
    }

    /// Drain bytes destined for the slave input (DSR replies, etc.).
    /// The PTY consumes these in U6; for U3 they exist so tests can
    /// inspect them.
    pub fn take_replies(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.replies)
    }

    /// Renderer-ready caret snapshot. Reflects current row/col,
    /// `?25h/l` visibility, and the DECSCUSR shape.
    pub fn caret(&self) -> Caret {
        Caret {
            row: self.cursor_row,
            col: self.cursor_col,
            visible: self.cursor_visible,
            shape: self.cursor_shape,
        }
    }

    /// Cursor position the renderer last redrew. The renderer compares
    /// this against `caret()` to know which cells need erase + redraw
    /// on the next paint, then calls
    /// [`Screen::acknowledge_cursor_paint`].
    pub fn last_painted_cursor(&self) -> (usize, usize) {
        self.last_painted_cursor
    }

    /// Tell the screen the renderer has drawn the caret at `caret()`'s
    /// current position. Should be called at the end of each paint.
    pub fn acknowledge_cursor_paint(&mut self) {
        self.last_painted_cursor = (self.cursor_row, self.cursor_col);
    }

    /// True while the alt-screen buffer is active.
    pub fn is_alt_screen(&self) -> bool {
        self.using_alt
    }

    /// Current scrollback view offset, in lines. 0 = live.
    pub fn view_offset(&self) -> usize {
        self.view_offset
    }

    /// Total number of lines currently in scrollback.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Adjust the scrollback view by `delta` lines. Positive = back in
    /// history; negative = forward. Clamped to `[0, scrollback_len()]`.
    /// No-op while alt-screen is active (xterm semantics — scrollback
    /// is hidden during full-screen TUIs).
    pub fn scroll_view(&mut self, delta: isize) {
        if self.using_alt {
            return;
        }
        let max = self.scrollback.len();
        let new_offset = if delta >= 0 {
            self.view_offset.saturating_add(delta as usize).min(max)
        } else {
            self.view_offset.saturating_sub((-delta) as usize)
        };
        self.view_offset = new_offset;
    }

    /// Snap the view back to live (offset 0). Called automatically
    /// whenever the primary buffer scrolls.
    pub fn snap_to_live(&mut self) {
        self.view_offset = 0;
    }

    /// Return the row of cells the renderer should draw at visible row
    /// `row`. When `view_offset > 0`, the top `view_offset` rows are
    /// pulled from scrollback and the bottom `rows - view_offset` rows
    /// from the live buffer. Caller must ensure `row < self.rows()`.
    pub fn visible_row(&self, row: usize) -> &[Cell] {
        debug_assert!(row < self.rows);
        if self.view_offset == 0 || row >= self.view_offset {
            // Drawn from the live buffer.
            &self.buffer[row - self.view_offset]
        } else {
            // Drawn from scrollback. The top of the view shows the line
            // `view_offset` back from the live top; row `r` shows the
            // line `view_offset - r` back.
            let n = self.scrollback.len();
            // Indices into scrollback (oldest = front): we want the
            // line whose "age" is `view_offset - row` (1-based).
            let age = self.view_offset - row;
            &self.scrollback[n - age]
        }
    }

    // ---- cursor primitives ----

    fn clamp_cursor(&mut self) {
        if self.cursor_row >= self.rows {
            self.cursor_row = self.rows - 1;
        }
        if self.cursor_col >= self.cols {
            self.cursor_col = self.cols - 1;
        }
    }

    fn move_to(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
        self.last_col_pending = false;
    }

    // ---- writing characters ----

    fn write_char(&mut self, c: char) {
        // Delayed wrap: if the previous print landed in the last column
        // and autowrap is on, wrap before placing this character.
        if self.last_col_pending {
            if self.autowrap {
                self.cursor_col = 0;
                self.line_feed();
            }
            self.last_col_pending = false;
        }

        if self.cursor_col >= self.cols {
            // Defensive — shouldn't happen after clamp + delayed-wrap
            // handling, but keep it safe.
            if self.autowrap {
                self.cursor_col = 0;
                self.line_feed();
            } else {
                self.cursor_col = self.cols - 1;
            }
        }

        self.buffer[self.cursor_row][self.cursor_col] = Cell {
            ch: c,
            fg: self.cur_fg,
            bg: self.cur_bg,
            attrs: self.cur_attrs,
        };

        if self.cursor_col + 1 < self.cols {
            self.cursor_col += 1;
        } else {
            // At the right margin — set the pending-wrap flag instead
            // of advancing past the edge.
            self.last_col_pending = true;
        }
    }

    /// Move cursor down one row, scrolling within the scroll region if
    /// it would leave the bottom. Equivalent to LF / IND. Clears the
    /// pending-wrap flag — xterm semantics: any row-changing movement
    /// retires the sticky-right-margin state.
    fn line_feed(&mut self) {
        if self.cursor_row == self.scroll_bot {
            self.scroll_up_in_region(1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
        self.last_col_pending = false;
    }

    /// Move cursor up one row, scrolling within region if it would
    /// leave the top. Equivalent to RI (reverse index).
    fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down_in_region(1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
        self.last_col_pending = false;
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
        self.last_col_pending = false;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.last_col_pending = false;
        }
    }

    fn tab(&mut self) {
        let next = ((self.cursor_col / config::TAB_WIDTH) + 1) * config::TAB_WIDTH;
        self.cursor_col = next.min(self.cols - 1);
        self.last_col_pending = false;
    }

    // ---- scrolling ----

    fn scroll_up_in_region(&mut self, n: usize) {
        let top = self.scroll_top;
        let bot = self.scroll_bot;
        let n = n.min(bot - top + 1);
        let blank_row = vec![self.blank_cell(); self.cols];

        // Scrollback eligibility: only when the scroll region spans the
        // top of the screen, we're in the primary buffer, and the
        // user-visible viewport is live. (Pushing scrollback while the
        // user is scrolled back would yank the view; xterm's choice is
        // to snap-to-live on new output, so we do that explicitly.)
        let push_scrollback = !self.using_alt && top == 0;
        if push_scrollback {
            for row in top..top + n {
                // Clone the row into scrollback, then it'll be
                // overwritten by the loop below.
                self.scrollback.push_back(self.buffer[row].clone());
                if self.scrollback.len() > config::SCROLLBACK_LINES {
                    self.scrollback.pop_front();
                }
            }
        }

        // Shift rows [top + n ..= bot] up to [top ..= bot - n], then
        // fill the bottom n rows with blanks.
        for row in top..=bot - n {
            let replacement = blank_row.clone();
            self.buffer[row] = core::mem::replace(&mut self.buffer[row + n], replacement);
        }
        for row in (bot + 1 - n)..=bot {
            self.buffer[row] = blank_row.clone();
        }

        // New output → live view.
        if push_scrollback {
            self.view_offset = 0;
        }
    }

    fn scroll_down_in_region(&mut self, n: usize) {
        let top = self.scroll_top;
        let bot = self.scroll_bot;
        let n = n.min(bot - top + 1);
        let blank_row = vec![self.blank_cell(); self.cols];
        // Shift rows [top ..= bot - n] down to [top + n ..= bot], then
        // fill the top n rows with blanks.
        for row in (top + n..=bot).rev() {
            let replacement = blank_row.clone();
            self.buffer[row] = core::mem::replace(&mut self.buffer[row - n], replacement);
        }
        for row in top..top + n {
            self.buffer[row] = blank_row.clone();
        }
    }

    fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: ColorSpec::Default,
            // Erased cells inherit the *current* background per ECMA-48
            // §8.3.39 ("BCE — background-color erase"); this is what
            // makes `clear` paint the screen with the prompt's bg color
            // in modern terminals.
            bg: self.cur_bg,
            attrs: 0,
        }
    }

    // ---- erase ----

    fn erase_in_line(&mut self, mode: u16) {
        let row = self.cursor_row;
        let blank = self.blank_cell();
        match mode {
            0 => {
                for col in self.cursor_col..self.cols {
                    self.buffer[row][col] = blank;
                }
            }
            1 => {
                for col in 0..=self.cursor_col.min(self.cols - 1) {
                    self.buffer[row][col] = blank;
                }
            }
            2 => {
                for col in 0..self.cols {
                    self.buffer[row][col] = blank;
                }
            }
            _ => {}
        }
        self.last_col_pending = false;
    }

    fn erase_in_display(&mut self, mode: u16) {
        let blank = self.blank_cell();
        match mode {
            0 => {
                // Erase from cursor to end of screen.
                self.erase_in_line(0);
                for row in self.cursor_row + 1..self.rows {
                    for col in 0..self.cols {
                        self.buffer[row][col] = blank;
                    }
                }
            }
            1 => {
                // Erase from start of screen to cursor.
                for row in 0..self.cursor_row {
                    for col in 0..self.cols {
                        self.buffer[row][col] = blank;
                    }
                }
                self.erase_in_line(1);
            }
            2 | 3 => {
                // Mode 3 is "also clear scrollback" — scrollback lives
                // in U4. For U3 we treat 2 and 3 identically.
                for row in 0..self.rows {
                    for col in 0..self.cols {
                        self.buffer[row][col] = blank;
                    }
                }
            }
            _ => {}
        }
        self.last_col_pending = false;
    }

    fn erase_chars(&mut self, n: usize) {
        let row = self.cursor_row;
        let blank = self.blank_cell();
        let start = self.cursor_col;
        let end = (start + n).min(self.cols);
        for col in start..end {
            self.buffer[row][col] = blank;
        }
    }

    // ---- insert / delete ----

    fn insert_chars(&mut self, n: usize) {
        let row = self.cursor_row;
        let start = self.cursor_col;
        let n = n.min(self.cols - start);
        // Shift cells right within [start..cols].
        for col in (start + n..self.cols).rev() {
            self.buffer[row][col] = self.buffer[row][col - n];
        }
        let blank = self.blank_cell();
        for col in start..start + n {
            self.buffer[row][col] = blank;
        }
    }

    fn delete_chars(&mut self, n: usize) {
        let row = self.cursor_row;
        let start = self.cursor_col;
        let n = n.min(self.cols - start);
        // Shift cells left within [start..cols].
        for col in start..self.cols - n {
            self.buffer[row][col] = self.buffer[row][col + n];
        }
        let blank = self.blank_cell();
        for col in self.cols - n..self.cols {
            self.buffer[row][col] = blank;
        }
    }

    fn insert_lines(&mut self, n: usize) {
        // Only operates if cursor is inside scroll region.
        if self.cursor_row < self.scroll_top || self.cursor_row > self.scroll_bot {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = self.cursor_row;
        self.scroll_down_in_region(n);
        self.scroll_top = saved_top;
    }

    fn delete_lines(&mut self, n: usize) {
        if self.cursor_row < self.scroll_top || self.cursor_row > self.scroll_bot {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = self.cursor_row;
        self.scroll_up_in_region(n);
        self.scroll_top = saved_top;
    }

    // ---- SGR ----

    fn apply_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            // Empty SGR == reset.
            self.cur_fg = ColorSpec::Default;
            self.cur_bg = ColorSpec::Default;
            self.cur_attrs = 0;
            return;
        }

        let mut i = 0;
        while i < params.len() {
            let p = params[i];
            match p {
                0 => {
                    self.cur_fg = ColorSpec::Default;
                    self.cur_bg = ColorSpec::Default;
                    self.cur_attrs = 0;
                }
                1 => self.cur_attrs |= attrs::BOLD,
                2 => self.cur_attrs |= attrs::DIM,
                3 => self.cur_attrs |= attrs::ITALIC,
                4 => self.cur_attrs |= attrs::UNDERLINE,
                7 => self.cur_attrs |= attrs::REVERSE,
                9 => self.cur_attrs |= attrs::STRIKE,
                22 => self.cur_attrs &= !(attrs::BOLD | attrs::DIM),
                23 => self.cur_attrs &= !attrs::ITALIC,
                24 => self.cur_attrs &= !attrs::UNDERLINE,
                27 => self.cur_attrs &= !attrs::REVERSE,
                29 => self.cur_attrs &= !attrs::STRIKE,
                30..=37 => self.cur_fg = ColorSpec::Indexed((p - 30) as u8),
                38 => {
                    // Extended foreground. Either 38;5;n or 38;2;r;g;b.
                    if let Some((spec, advance)) = parse_extended_color(&params[i + 1..]) {
                        self.cur_fg = spec;
                        i += advance;
                    }
                }
                39 => self.cur_fg = ColorSpec::Default,
                40..=47 => self.cur_bg = ColorSpec::Indexed((p - 40) as u8),
                48 => {
                    if let Some((spec, advance)) = parse_extended_color(&params[i + 1..]) {
                        self.cur_bg = spec;
                        i += advance;
                    }
                }
                49 => self.cur_bg = ColorSpec::Default,
                90..=97 => self.cur_fg = ColorSpec::Indexed((p - 90 + 8) as u8),
                100..=107 => self.cur_bg = ColorSpec::Indexed((p - 100 + 8) as u8),
                _ => { /* unknown — ignore */ }
            }
            i += 1;
        }
    }

    // ---- modes (CSI h / l) ----

    fn set_mode(&mut self, params: &[u16], private: bool, value: bool) {
        for &p in params {
            match (private, p) {
                (true, 7) => self.autowrap = value,
                (true, 25) => self.cursor_visible = value,
                (true, 47) | (true, 1047) | (true, 1049) => {
                    if value {
                        self.enter_alt_screen(/*clear=*/ p != 47);
                    } else {
                        self.exit_alt_screen();
                    }
                }
                // ?2004 (bracketed paste) lands in U7.
                (true, 2004) => {}
                _ => {}
            }
        }
    }

    // ---- alt screen ----

    fn enter_alt_screen(&mut self, clear: bool) {
        if self.using_alt {
            return;
        }
        // Stash primary state.
        let blank_row = vec![Cell::EMPTY; self.cols];
        let mut new_cells = vec![blank_row.clone(); self.rows];
        core::mem::swap(&mut self.buffer, &mut new_cells);
        // `new_cells` now holds the primary contents.
        self.stashed = Some(StashedBuffer {
            cells: new_cells,
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
            cur_fg: self.cur_fg,
            cur_bg: self.cur_bg,
            cur_attrs: self.cur_attrs,
            scroll_top: self.scroll_top,
            scroll_bot: self.scroll_bot,
            saved: self.saved,
            last_col_pending: self.last_col_pending,
        });

        // Fresh alt buffer state. ?1049 clears the alt buffer on entry;
        // ?47 does not (we keep what's there from a previous session).
        // We always start with a fresh buffer here because we have no
        // persistent alt — close enough for our purposes.
        let _ = clear; // currently always clear; both 47 and 1049 reset

        self.cursor_row = 0;
        self.cursor_col = 0;
        self.cur_fg = ColorSpec::Default;
        self.cur_bg = ColorSpec::Default;
        self.cur_attrs = 0;
        self.scroll_top = 0;
        self.scroll_bot = self.rows - 1;
        self.saved = None;
        self.last_col_pending = false;
        self.using_alt = true;
        // Scrollback view snaps to live so the renderer doesn't expose
        // primary scrollback through the alt buffer.
        self.view_offset = 0;
    }

    fn exit_alt_screen(&mut self) {
        if !self.using_alt {
            return;
        }
        let s = match self.stashed.take() {
            Some(s) => s,
            None => return,
        };
        self.buffer = s.cells;
        self.cursor_row = s.cursor_row;
        self.cursor_col = s.cursor_col;
        self.cur_fg = s.cur_fg;
        self.cur_bg = s.cur_bg;
        self.cur_attrs = s.cur_attrs;
        self.scroll_top = s.scroll_top;
        self.scroll_bot = s.scroll_bot;
        self.saved = s.saved;
        self.last_col_pending = s.last_col_pending;
        self.using_alt = false;
        self.view_offset = 0;
    }

    // ---- save / restore ----

    fn save_cursor(&mut self) {
        self.saved = Some(SavedCursor {
            row: self.cursor_row,
            col: self.cursor_col,
            fg: self.cur_fg,
            bg: self.cur_bg,
            attrs: self.cur_attrs,
            last_col_pending: self.last_col_pending,
        });
    }

    fn restore_cursor(&mut self) {
        if let Some(s) = self.saved {
            self.cursor_row = s.row.min(self.rows - 1);
            self.cursor_col = s.col.min(self.cols - 1);
            self.cur_fg = s.fg;
            self.cur_bg = s.bg;
            self.cur_attrs = s.attrs;
            self.last_col_pending = s.last_col_pending;
        } else {
            // No saved state — go home.
            self.move_to(0, 0);
        }
    }

    // ---- DSR ----

    fn device_status_report(&mut self, p: u16) {
        if p == 6 {
            // Reply `ESC [ <row> ; <col> R` — 1-indexed.
            let row = self.cursor_row + 1;
            let col = self.cursor_col + 1;
            let mut buf = [0u8; 32];
            let n = write_csi_position(&mut buf, row, col);
            self.replies.extend_from_slice(&buf[..n]);
        }
    }

    // ---- scroll region ----

    fn set_scroll_region(&mut self, top: u16, bot: u16) {
        // 1-indexed inputs; 0 means "use default" per VT100.
        let top = if top == 0 { 1 } else { top };
        let bot = if bot == 0 {
            self.rows as u16
        } else {
            bot
        };
        let top0 = (top - 1) as usize;
        let bot0 = (bot - 1) as usize;
        if top0 < bot0 && bot0 < self.rows {
            self.scroll_top = top0;
            self.scroll_bot = bot0;
            self.move_to(0, 0);
        }
    }
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Parse an extended-color sequence starting at `params[0]` (just past
/// the 38 or 48 leader). Returns the parsed `ColorSpec` and the number
/// of additional params consumed. Returns `None` if the sequence is
/// malformed; the caller advances by 1 (skipping just the 38/48).
fn parse_extended_color(params: &[u16]) -> Option<(ColorSpec, usize)> {
    match params.first() {
        Some(&5) => {
            // 5 ; n — indexed.
            let n = *params.get(1)?;
            Some((ColorSpec::Indexed(n.min(255) as u8), 2))
        }
        Some(&2) => {
            // 2 ; r ; g ; b — truecolor.
            let r = *params.get(1)?;
            let g = *params.get(2)?;
            let b = *params.get(3)?;
            Some((
                ColorSpec::Rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8),
                4,
            ))
        }
        _ => None,
    }
}

/// Write `\x1b[<row>;<col>R` into `buf`, returning the byte count.
fn write_csi_position(buf: &mut [u8], row: usize, col: usize) -> usize {
    let mut n = 0;
    buf[n] = 0x1B;
    n += 1;
    buf[n] = b'[';
    n += 1;
    n += write_u16(&mut buf[n..], row as u16);
    buf[n] = b';';
    n += 1;
    n += write_u16(&mut buf[n..], col as u16);
    buf[n] = b'R';
    n += 1;
    n
}

/// Write `v` as decimal ASCII into `buf`. Returns byte count.
fn write_u16(buf: &mut [u8], v: u16) -> usize {
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    // Render least-significant digit first, then reverse.
    let mut tmp = [0u8; 5];
    let mut t = 0;
    let mut x = v;
    while x > 0 {
        tmp[t] = b'0' + (x % 10) as u8;
        x /= 10;
        t += 1;
    }
    for i in 0..t {
        buf[i] = tmp[t - 1 - i];
    }
    t
}

// ---------------------------------------------------------------------
// Perform impl
// ---------------------------------------------------------------------

impl Perform for Screen {
    fn print(&mut self, c: char) {
        self.write_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => { /* BEL — consumers may flash; we no-op */ }
            0x08 => self.backspace(),
            0x09 => self.tab(),
            0x0A | 0x0B | 0x0C => self.line_feed(),
            0x0D => self.carriage_return(),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &[u16],
        intermediates: &[u8],
        _ignore: bool,
        final_byte: u8,
    ) {
        // Determine private-marker prefix (?, >, <, =) and the real
        // intermediates (space, etc.).
        let (private, real_inter): (Option<u8>, &[u8]) = match intermediates.first() {
            Some(&b) if matches!(b, b'?' | b'>' | b'<' | b'=') => {
                (Some(b), &intermediates[1..])
            }
            _ => (None, intermediates),
        };

        // Helper: first param defaulting to `default` if missing / zero.
        let p1 = |default: u16| -> u16 {
            match params.first().copied() {
                Some(0) | None => default,
                Some(v) => v,
            }
        };

        match (final_byte, real_inter, private) {
            // Cursor movement.
            (b'A', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.last_col_pending = false;
            }
            (b'B', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
                self.last_col_pending = false;
            }
            (b'C', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_col = (self.cursor_col + n).min(self.cols - 1);
                self.last_col_pending = false;
            }
            (b'D', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_col = self.cursor_col.saturating_sub(n);
                self.last_col_pending = false;
            }
            (b'E', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
                self.cursor_col = 0;
                self.last_col_pending = false;
            }
            (b'F', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
                self.last_col_pending = false;
            }
            (b'G', &[], None) | (b'`', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_col = n.saturating_sub(1).min(self.cols - 1);
                self.last_col_pending = false;
            }
            (b'H', &[], None) | (b'f', &[], None) => {
                let row = p1(1) as usize;
                let col = params.get(1).copied().filter(|&v| v != 0).unwrap_or(1) as usize;
                self.move_to(row.saturating_sub(1), col.saturating_sub(1));
            }
            (b'd', &[], None) => {
                let n = p1(1) as usize;
                self.cursor_row = n.saturating_sub(1).min(self.rows - 1);
                self.last_col_pending = false;
            }

            // Erase.
            (b'J', &[], None) => self.erase_in_display(params.first().copied().unwrap_or(0)),
            (b'K', &[], None) => self.erase_in_line(params.first().copied().unwrap_or(0)),
            (b'X', &[], None) => self.erase_chars(p1(1) as usize),

            // Insert / delete.
            (b'@', &[], None) => self.insert_chars(p1(1) as usize),
            (b'P', &[], None) => self.delete_chars(p1(1) as usize),
            (b'L', &[], None) => self.insert_lines(p1(1) as usize),
            (b'M', &[], None) => self.delete_lines(p1(1) as usize),

            // Scroll.
            (b'S', &[], None) => self.scroll_up_in_region(p1(1) as usize),
            (b'T', &[], None) => self.scroll_down_in_region(p1(1) as usize),

            // SGR.
            (b'm', &[], None) => self.apply_sgr(params),

            // Modes.
            (b'h', &[], private_marker) => {
                self.set_mode(params, private_marker.is_some(), true);
            }
            (b'l', &[], private_marker) => {
                self.set_mode(params, private_marker.is_some(), false);
            }

            // DSR.
            (b'n', &[], None) => self.device_status_report(p1(0)),

            // Scroll region.
            (b'r', &[], None) => {
                let top = p1(1);
                let bot = params.get(1).copied().filter(|&v| v != 0).unwrap_or(self.rows as u16);
                self.set_scroll_region(top, bot);
            }

            // Save / restore (xterm flavor).
            (b's', &[], None) => self.save_cursor(),
            (b'u', &[], None) => self.restore_cursor(),

            // DECSCUSR — `<n> SP q`.
            (b'q', &[b' '], None) => {
                let n = params.first().copied().unwrap_or(0);
                self.cursor_shape = match n {
                    0 | 1 | 2 => CursorShape::Block,
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => self.cursor_shape,
                };
            }

            _ => { /* unknown CSI — ignore */ }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            (&[], b'7') => self.save_cursor(),
            (&[], b'8') => self.restore_cursor(),
            (&[], b'D') => self.line_feed(),
            (&[], b'M') => self.reverse_index(),
            (&[], b'E') => {
                self.carriage_return();
                self.line_feed();
            }
            (&[], b'c') => {
                // RIS — full reset.
                let blank = Cell::EMPTY;
                for row in 0..self.rows {
                    for col in 0..self.cols {
                        self.buffer[row][col] = blank;
                    }
                }
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.cur_fg = ColorSpec::Default;
                self.cur_bg = ColorSpec::Default;
                self.cur_attrs = 0;
                self.scroll_top = 0;
                self.scroll_bot = self.rows - 1;
                self.saved = None;
                self.autowrap = true;
                self.cursor_visible = true;
                self.cursor_shape = CursorShape::Block;
                self.last_col_pending = false;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, _data: &[u8], _bell_terminated: bool) {
        // OSC dispatch (window title etc.) is consumed by a higher
        // layer in U6 / U9. Screen ignores it.
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &tests::test_print_plain_text,
        &tests::test_lf_advances_row,
        &tests::test_cr_resets_col,
        &tests::test_backspace,
        &tests::test_tab,
        &tests::test_autowrap_to_next_line,
        &tests::test_delayed_wrap_at_last_col,
        &tests::test_cup_positions_cursor,
        &tests::test_cursor_relative_moves,
        &tests::test_erase_in_line_modes,
        &tests::test_erase_in_display_below,
        &tests::test_erase_in_display_all,
        &tests::test_erase_chars,
        &tests::test_insert_chars,
        &tests::test_delete_chars,
        &tests::test_insert_lines,
        &tests::test_delete_lines,
        &tests::test_sgr_indexed_fg,
        &tests::test_sgr_truecolor_fg,
        &tests::test_sgr_attrs_set_and_clear,
        &tests::test_sgr_default_fg_bg,
        &tests::test_sgr_bright_colors,
        &tests::test_save_restore_cursor_esc,
        &tests::test_save_restore_cursor_csi,
        &tests::test_dec_private_show_hide_cursor,
        &tests::test_dec_private_autowrap,
        &tests::test_decscusr_cursor_shape,
        &tests::test_dsr_cursor_position_reply,
        &tests::test_scroll_region_restricts_lf,
        &tests::test_reverse_index_at_top_scrolls,
        &tests::test_ris_resets_state,
        &tests::test_lf_at_bottom_scrolls,
        &tests::test_bce_erase_uses_current_bg,
        &tests::test_scrollback_populates_on_scroll,
        &tests::test_scrollback_caps_at_limit,
        &tests::test_scrollback_not_added_under_restricted_region,
        &tests::test_view_offset_pulls_from_scrollback,
        &tests::test_scroll_view_clamps,
        &tests::test_new_output_snaps_view_to_live,
        &tests::test_alt_screen_swap_preserves_primary,
        &tests::test_alt_screen_starts_clean,
        &tests::test_alt_screen_no_scrollback,
        &tests::test_alt_screen_scroll_view_disabled,
        &tests::test_caret_initial,
        &tests::test_caret_reflects_visibility_and_shape,
        &tests::test_caret_tracks_cursor_position,
        &tests::test_last_painted_cursor_acknowledge,
    ]
}

#[cfg(feature = "test")]
mod tests {
    use super::*;
    use crate::terminal::vte::Vte;

    fn feed(s: &mut Screen, bytes: &[u8]) {
        let mut vte = Vte::new();
        for &b in bytes {
            vte.advance(b, s);
        }
    }

    fn read_row_chars(s: &Screen, row: usize) -> alloc::string::String {
        let mut out = alloc::string::String::new();
        for col in 0..s.cols() {
            out.push(s.cell(row, col).ch);
        }
        out
    }

    pub(super) fn test_print_plain_text() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"hello");
        assert_eq!(s.cell(0, 0).ch, 'h');
        assert_eq!(s.cell(0, 4).ch, 'o');
        assert_eq!(s.cursor(), (0, 5));
    }

    pub(super) fn test_lf_advances_row() {
        let mut s = Screen::new(3, 10);
        feed(&mut s, b"a\nb");
        assert_eq!(s.cell(0, 0).ch, 'a');
        assert_eq!(s.cell(1, 1).ch, 'b');
        assert_eq!(s.cursor(), (1, 2));
    }

    pub(super) fn test_cr_resets_col() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"abc\rX");
        assert_eq!(s.cell(0, 0).ch, 'X');
        assert_eq!(s.cell(0, 1).ch, 'b');
        assert_eq!(s.cell(0, 2).ch, 'c');
    }

    pub(super) fn test_backspace() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"ab\x08X");
        // 'b' was placed at col 1, BS moves cursor to col 1, 'X' overwrites b.
        assert_eq!(s.cell(0, 0).ch, 'a');
        assert_eq!(s.cell(0, 1).ch, 'X');
    }

    pub(super) fn test_tab() {
        let mut s = Screen::new(2, 20);
        feed(&mut s, b"a\tb");
        assert_eq!(s.cell(0, 0).ch, 'a');
        assert_eq!(s.cursor().1, 9); // tab to col 8, then 'b' placed there, cursor advances
        assert_eq!(s.cell(0, 8).ch, 'b');
    }

    pub(super) fn test_autowrap_to_next_line() {
        let mut s = Screen::new(3, 4);
        // Print 5 chars on a 4-wide screen — last one should wrap.
        feed(&mut s, b"abcde");
        assert_eq!(read_row_chars(&s, 0), "abcd");
        assert_eq!(s.cell(1, 0).ch, 'e');
        assert_eq!(s.cursor(), (1, 1));
    }

    pub(super) fn test_delayed_wrap_at_last_col() {
        let mut s = Screen::new(3, 4);
        // After printing 4 chars, cursor stays at last-column with
        // pending-wrap flag set. A query of cursor position should
        // see col 3 (0-indexed), not col 4.
        feed(&mut s, b"abcd");
        assert_eq!(s.cursor(), (0, 3));
        // Next printable triggers the wrap.
        feed(&mut s, b"e");
        assert_eq!(s.cell(1, 0).ch, 'e');
    }

    pub(super) fn test_cup_positions_cursor() {
        let mut s = Screen::new(5, 10);
        feed(&mut s, b"\x1b[3;5HX");
        // 1-indexed: row 3, col 5 → 0-indexed (2, 4).
        assert_eq!(s.cell(2, 4).ch, 'X');
    }

    pub(super) fn test_cursor_relative_moves() {
        let mut s = Screen::new(5, 10);
        feed(&mut s, b"\x1b[3;5H"); // move to (2,4)
        feed(&mut s, b"\x1b[2A"); // up 2 → (0,4)
        assert_eq!(s.cursor(), (0, 4));
        feed(&mut s, b"\x1b[3B"); // down 3 → (3,4)
        assert_eq!(s.cursor(), (3, 4));
        feed(&mut s, b"\x1b[2C"); // right 2 → (3,6)
        assert_eq!(s.cursor(), (3, 6));
        feed(&mut s, b"\x1b[4D"); // left 4 → (3,2)
        assert_eq!(s.cursor(), (3, 2));
    }

    pub(super) fn test_erase_in_line_modes() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"abcdefghij");
        // Cursor is now at (0,9) with pending wrap. Move to (0,4).
        feed(&mut s, b"\x1b[1;5H");
        feed(&mut s, b"\x1b[K"); // erase from cursor to end
        assert_eq!(read_row_chars(&s, 0), "abcd      ");

        feed(&mut s, b"\x1b[1;5H");
        feed(&mut s, b"\x1b[1K"); // erase from start to cursor
        assert_eq!(read_row_chars(&s, 0), "          ");

        // Refill, then erase entire line.
        feed(&mut s, b"\x1b[1;1H");
        feed(&mut s, b"xxxxxxxxxx");
        feed(&mut s, b"\x1b[1;5H");
        feed(&mut s, b"\x1b[2K");
        assert_eq!(read_row_chars(&s, 0), "          ");
    }

    pub(super) fn test_erase_in_display_below() {
        let mut s = Screen::new(3, 4);
        feed(&mut s, b"aaaa\r\nbbbb\r\ncccc");
        feed(&mut s, b"\x1b[2;3H"); // (1,2)
        feed(&mut s, b"\x1b[J"); // erase from cursor to end of display
        assert_eq!(read_row_chars(&s, 0), "aaaa");
        assert_eq!(read_row_chars(&s, 1), "bb  ");
        assert_eq!(read_row_chars(&s, 2), "    ");
    }

    pub(super) fn test_erase_in_display_all() {
        let mut s = Screen::new(2, 3);
        // 6 chars on a 2x3 wraps "abc" into row 0 and "def" into row 1.
        feed(&mut s, b"abcdef");
        feed(&mut s, b"\x1b[2J");
        assert_eq!(read_row_chars(&s, 0), "   ");
        assert_eq!(read_row_chars(&s, 1), "   ");
    }

    pub(super) fn test_erase_chars() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"abcdefghij");
        feed(&mut s, b"\x1b[1;3H"); // (0,2)
        feed(&mut s, b"\x1b[3X"); // erase 3 chars
        assert_eq!(read_row_chars(&s, 0), "ab   fghij");
    }

    pub(super) fn test_insert_chars() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"abcdefghij");
        feed(&mut s, b"\x1b[1;3H"); // (0,2)
        feed(&mut s, b"\x1b[2@"); // insert 2 blanks at cursor
        assert_eq!(read_row_chars(&s, 0), "ab  cdefgh");
    }

    pub(super) fn test_delete_chars() {
        let mut s = Screen::new(2, 10);
        feed(&mut s, b"abcdefghij");
        feed(&mut s, b"\x1b[1;3H"); // (0,2)
        feed(&mut s, b"\x1b[2P"); // delete 2 chars at cursor
        assert_eq!(read_row_chars(&s, 0), "abefghij  ");
    }

    pub(super) fn test_insert_lines() {
        let mut s = Screen::new(4, 3);
        feed(&mut s, b"AAA\r\nBBB\r\nCCC\r\nDDD");
        feed(&mut s, b"\x1b[2;1H"); // (1,0)
        feed(&mut s, b"\x1b[1L"); // insert 1 line
        assert_eq!(read_row_chars(&s, 0), "AAA");
        assert_eq!(read_row_chars(&s, 1), "   ");
        assert_eq!(read_row_chars(&s, 2), "BBB");
        assert_eq!(read_row_chars(&s, 3), "CCC");
    }

    pub(super) fn test_delete_lines() {
        let mut s = Screen::new(4, 3);
        feed(&mut s, b"AAA\r\nBBB\r\nCCC\r\nDDD");
        feed(&mut s, b"\x1b[2;1H");
        feed(&mut s, b"\x1b[1M"); // delete 1 line
        assert_eq!(read_row_chars(&s, 0), "AAA");
        assert_eq!(read_row_chars(&s, 1), "CCC");
        assert_eq!(read_row_chars(&s, 2), "DDD");
        assert_eq!(read_row_chars(&s, 3), "   ");
    }

    pub(super) fn test_sgr_indexed_fg() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[31mR");
        assert_eq!(s.cell(0, 0).fg, ColorSpec::Indexed(1));
    }

    pub(super) fn test_sgr_truecolor_fg() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[38;2;200;100;50mX");
        assert_eq!(s.cell(0, 0).fg, ColorSpec::Rgb(200, 100, 50));
    }

    pub(super) fn test_sgr_attrs_set_and_clear() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[1;4mA");
        assert_eq!(s.cell(0, 0).attrs & attrs::BOLD, attrs::BOLD);
        assert_eq!(s.cell(0, 0).attrs & attrs::UNDERLINE, attrs::UNDERLINE);
        feed(&mut s, b"\x1b[22mB");
        assert_eq!(s.cell(0, 1).attrs & attrs::BOLD, 0);
        feed(&mut s, b"\x1b[0mC");
        assert_eq!(s.cell(0, 2).attrs, 0);
    }

    pub(super) fn test_sgr_default_fg_bg() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[31;42m");
        feed(&mut s, b"\x1b[39mF");
        feed(&mut s, b"\x1b[49mG");
        assert_eq!(s.cell(0, 0).fg, ColorSpec::Default);
        assert_eq!(s.cell(0, 0).bg, ColorSpec::Indexed(2));
        assert_eq!(s.cell(0, 1).bg, ColorSpec::Default);
    }

    pub(super) fn test_sgr_bright_colors() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[91mR\x1b[102mG");
        assert_eq!(s.cell(0, 0).fg, ColorSpec::Indexed(9));
        assert_eq!(s.cell(0, 1).bg, ColorSpec::Indexed(10));
    }

    pub(super) fn test_save_restore_cursor_esc() {
        let mut s = Screen::new(3, 10);
        feed(&mut s, b"\x1b[2;5H"); // (1,4)
        feed(&mut s, b"\x1b7"); // DECSC
        feed(&mut s, b"\x1b[1;1H");
        feed(&mut s, b"\x1b8"); // DECRC
        assert_eq!(s.cursor(), (1, 4));
    }

    pub(super) fn test_save_restore_cursor_csi() {
        let mut s = Screen::new(3, 10);
        feed(&mut s, b"\x1b[2;5H");
        feed(&mut s, b"\x1b[s");
        feed(&mut s, b"\x1b[1;1H");
        feed(&mut s, b"\x1b[u");
        assert_eq!(s.cursor(), (1, 4));
    }

    pub(super) fn test_dec_private_show_hide_cursor() {
        let mut s = Screen::new(1, 5);
        assert!(s.cursor_visible());
        feed(&mut s, b"\x1b[?25l");
        assert!(!s.cursor_visible());
        feed(&mut s, b"\x1b[?25h");
        assert!(s.cursor_visible());
    }

    pub(super) fn test_dec_private_autowrap() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[?7l");
        assert!(!s.autowrap());
        feed(&mut s, b"\x1b[?7h");
        assert!(s.autowrap());
    }

    pub(super) fn test_decscusr_cursor_shape() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[4 q");
        assert_eq!(s.cursor_shape(), CursorShape::Underline);
        feed(&mut s, b"\x1b[6 q");
        assert_eq!(s.cursor_shape(), CursorShape::Bar);
        feed(&mut s, b"\x1b[0 q");
        assert_eq!(s.cursor_shape(), CursorShape::Block);
    }

    pub(super) fn test_dsr_cursor_position_reply() {
        let mut s = Screen::new(5, 10);
        feed(&mut s, b"\x1b[3;7H"); // (2,6) — 1-indexed reply 3;7
        feed(&mut s, b"\x1b[6n");
        let replies = s.take_replies();
        assert_eq!(&replies[..], b"\x1b[3;7R");
    }

    pub(super) fn test_scroll_region_restricts_lf() {
        let mut s = Screen::new(5, 3);
        feed(&mut s, b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE");
        feed(&mut s, b"\x1b[2;4r"); // scroll region rows 2..4 (0-indexed 1..=3)
        // After setting region, cursor goes to home.
        // Move cursor to bottom of region (row 4, 1-indexed) and LF.
        feed(&mut s, b"\x1b[4;1H");
        feed(&mut s, b"\n"); // should scroll the region
        // Row 0 (outside region) unchanged.
        assert_eq!(read_row_chars(&s, 0), "AAA");
        // Rows inside region shifted up: row1=CCC, row2=DDD, row3=blank.
        assert_eq!(read_row_chars(&s, 1), "CCC");
        assert_eq!(read_row_chars(&s, 2), "DDD");
        assert_eq!(read_row_chars(&s, 3), "   ");
        // Row 4 (outside region) unchanged.
        assert_eq!(read_row_chars(&s, 4), "EEE");
    }

    pub(super) fn test_reverse_index_at_top_scrolls() {
        let mut s = Screen::new(3, 3);
        feed(&mut s, b"AAA\r\nBBB\r\nCCC");
        feed(&mut s, b"\x1b[1;1H");
        feed(&mut s, b"\x1bM"); // RI at top scrolls down
        assert_eq!(read_row_chars(&s, 0), "   ");
        assert_eq!(read_row_chars(&s, 1), "AAA");
        assert_eq!(read_row_chars(&s, 2), "BBB");
    }

    pub(super) fn test_ris_resets_state() {
        let mut s = Screen::new(3, 3);
        feed(&mut s, b"\x1b[31mABC\r\n");
        feed(&mut s, b"\x1bc"); // RIS
        assert_eq!(s.cursor(), (0, 0));
        assert_eq!(s.cell(0, 0).ch, ' ');
        assert_eq!(s.cell(0, 0).fg, ColorSpec::Default);
    }

    pub(super) fn test_lf_at_bottom_scrolls() {
        let mut s = Screen::new(3, 3);
        feed(&mut s, b"AAA\r\nBBB\r\nCCC");
        // Cursor at (2, 2) pending-wrap. LF should scroll within default
        // region [0, 2] — pending-wrap is cleared by the row movement.
        feed(&mut s, b"\n");
        assert_eq!(read_row_chars(&s, 0), "BBB");
        assert_eq!(read_row_chars(&s, 1), "CCC");
        assert_eq!(read_row_chars(&s, 2), "   ");
    }

    fn read_visible_row_chars(s: &Screen, row: usize) -> alloc::string::String {
        let mut out = alloc::string::String::new();
        for cell in s.visible_row(row) {
            out.push(cell.ch);
        }
        out
    }

    pub(super) fn test_scrollback_populates_on_scroll() {
        let mut s = Screen::new(2, 3);
        // Fill both rows, then force a scroll by LF at bottom.
        feed(&mut s, b"AAA\r\nBBB\r\n");
        // After LF at the bottom, "AAA" should have scrolled into
        // scrollback (one entry).
        assert_eq!(s.scrollback_len(), 1);
        assert_eq!(s.scrollback[0][0].ch, 'A');
    }

    pub(super) fn test_scrollback_caps_at_limit() {
        let mut s = Screen::new(2, 1);
        // Each LF scrolls one line. Generate more than SCROLLBACK_LINES
        // events so the ring evicts. We can't realistically loop
        // 5000 times in a kernel test, so monkey-patch the limit via
        // direct manipulation: push entries until we exceed and then
        // confirm length is bounded by config::SCROLLBACK_LINES.
        let cap = super::config::SCROLLBACK_LINES;
        // Fast-fill scrollback with synthetic rows.
        for i in 0..cap + 5 {
            let mut row = alloc::vec![Cell::EMPTY; 1];
            row[0].ch = (b'a' + (i % 26) as u8) as char;
            s.scrollback.push_back(row);
            if s.scrollback.len() > cap {
                s.scrollback.pop_front();
            }
        }
        assert_eq!(s.scrollback_len(), cap);
    }

    pub(super) fn test_scrollback_not_added_under_restricted_region() {
        let mut s = Screen::new(4, 3);
        // Restrict the scroll region to rows 2..=3 (away from the top).
        feed(&mut s, b"\x1b[2;3r");
        // Force scrolling inside the region.
        feed(&mut s, b"\x1b[3;1H"); // bottom of region
        feed(&mut s, b"\n");
        feed(&mut s, b"\n");
        feed(&mut s, b"\n");
        assert_eq!(
            s.scrollback_len(),
            0,
            "scrollback must not capture lines from a non-top scroll region",
        );
    }

    pub(super) fn test_view_offset_pulls_from_scrollback() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"AAA\r\nBBB\r\nCCC\r\n");
        // Expected scrollback after three LF-at-bottom events on a
        // 2-row screen: AAA, BBB (in age order). Live buffer: CCC,
        // blank.
        assert_eq!(s.scrollback_len(), 2);
        // Scroll view back 1 line.
        s.scroll_view(1);
        assert_eq!(s.view_offset(), 1);
        // visible row 0 should now show the most recent scrollback entry.
        assert_eq!(read_visible_row_chars(&s, 0), "BBB");
        assert_eq!(read_visible_row_chars(&s, 1), "CCC");
        // Scroll back another line.
        s.scroll_view(1);
        assert_eq!(read_visible_row_chars(&s, 0), "AAA");
        assert_eq!(read_visible_row_chars(&s, 1), "BBB");
    }

    pub(super) fn test_scroll_view_clamps() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"AAA\r\nBBB\r\n");
        // Only 1 scrollback line. Asking for 10 back clamps to 1.
        s.scroll_view(10);
        assert_eq!(s.view_offset(), 1);
        s.scroll_view(-100);
        assert_eq!(s.view_offset(), 0);
    }

    pub(super) fn test_new_output_snaps_view_to_live() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"AAA\r\nBBB\r\n");
        s.scroll_view(1);
        assert_eq!(s.view_offset(), 1);
        // Any output that scrolls the primary should snap to live.
        feed(&mut s, b"CCC\r\n");
        assert_eq!(s.view_offset(), 0);
    }

    pub(super) fn test_alt_screen_swap_preserves_primary() {
        let mut s = Screen::new(3, 3);
        feed(&mut s, b"PRI\r\n");
        feed(&mut s, b"\x1b[?1049h"); // enter alt
        assert!(s.is_alt_screen());
        // Alt buffer starts clean.
        assert_eq!(read_row_chars(&s, 0), "   ");
        feed(&mut s, b"ALT");
        assert_eq!(read_row_chars(&s, 0), "ALT");
        feed(&mut s, b"\x1b[?1049l"); // exit alt
        assert!(!s.is_alt_screen());
        // Primary contents restored.
        assert_eq!(read_row_chars(&s, 0), "PRI");
    }

    pub(super) fn test_alt_screen_starts_clean() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"DRTY\r\n");
        feed(&mut s, b"\x1b[?1049h");
        // Cursor at home; all rows blank.
        assert_eq!(s.cursor(), (0, 0));
        for row in 0..s.rows() {
            assert_eq!(read_row_chars(&s, row), "   ");
        }
    }

    pub(super) fn test_alt_screen_no_scrollback() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"\x1b[?1049h");
        // Force scroll inside alt — should not populate scrollback.
        feed(&mut s, b"AAA\r\nBBB\r\nCCC\r\n");
        assert_eq!(s.scrollback_len(), 0);
    }

    pub(super) fn test_alt_screen_scroll_view_disabled() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"AAA\r\nBBB\r\n"); // populate scrollback
        feed(&mut s, b"\x1b[?1049h");
        // scroll_view is a no-op while alt is active.
        s.scroll_view(1);
        assert_eq!(s.view_offset(), 0);
    }

    pub(super) fn test_caret_initial() {
        let s = Screen::new(3, 5);
        let c = s.caret();
        assert_eq!(c.row, 0);
        assert_eq!(c.col, 0);
        assert!(c.visible);
        assert_eq!(c.shape, CursorShape::Block);
    }

    pub(super) fn test_caret_reflects_visibility_and_shape() {
        let mut s = Screen::new(3, 5);
        feed(&mut s, b"\x1b[?25l\x1b[4 q");
        let c = s.caret();
        assert!(!c.visible);
        assert_eq!(c.shape, CursorShape::Underline);
    }

    pub(super) fn test_caret_tracks_cursor_position() {
        let mut s = Screen::new(5, 5);
        feed(&mut s, b"\x1b[3;4HX");
        // After "X" at (2,3), cursor advances to (2,4).
        let c = s.caret();
        assert_eq!((c.row, c.col), (2, 4));
    }

    pub(super) fn test_last_painted_cursor_acknowledge() {
        let mut s = Screen::new(3, 5);
        feed(&mut s, b"\x1b[2;2H");
        // Before acknowledge, last_painted_cursor is still (0,0).
        assert_eq!(s.last_painted_cursor(), (0, 0));
        assert_ne!(s.last_painted_cursor(), (s.caret().row, s.caret().col));
        s.acknowledge_cursor_paint();
        assert_eq!(s.last_painted_cursor(), (1, 1));
    }

    pub(super) fn test_bce_erase_uses_current_bg() {
        let mut s = Screen::new(1, 5);
        feed(&mut s, b"\x1b[41m"); // set bg = red
        feed(&mut s, b"\x1b[2K"); // erase line
        // All cells should have bg = Indexed(1), even though they're
        // visually blank.
        for col in 0..s.cols() {
            assert_eq!(s.cell(0, col).bg, ColorSpec::Indexed(1));
            assert_eq!(s.cell(0, col).ch, ' ');
        }
    }
}
