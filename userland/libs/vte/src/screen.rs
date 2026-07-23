//! The character grid.
//!
//! `Screen` owns the visible terminal contents — a primary buffer of
//! cells, the cursor, the current SGR pen, and the scroll region. It
//! implements [`vte::Perform`] so a raw byte stream can be fed straight
//! through the parser into the grid.
//!
//! Feature set:
//! - Cursor movement (CUU/CUD/CUF/CUB/CUP/CHA/VPA/CNL/CPL).
//! - Erase in display / erase in line / erase characters (ED 3 also
//!   clears scrollback, xterm-style).
//! - Insert / delete characters / lines within the scroll region.
//! - Scroll up / scroll down; DECSTBM scroll region.
//! - SGR — full set: reset/bold/dim/italic/underline/reverse/strike,
//!   16 ANSI colors, 256 indexed, 24-bit truecolor, default fg/bg.
//! - Save / restore cursor (DECSC / DECRC, ESC 7 / 8 and CSI s / u).
//! - Index / reverse-index / next-line (ESC D / M / E).
//! - DSR 6 (cursor position report) — reply bytes are queued in
//!   [`Screen::take_replies`] for the app to write back to the master.
//! - DEC private modes: ?7 autowrap, ?25 cursor-visible, ?47/?1047/?1049
//!   alt-screen buffer.
//! - DECSCUSR cursor shape (parsed and stored).
//! - Scrollback ring plus a scroll view (`scroll_view`/`visible_row`)
//!   the app drives from Shift+PgUp/PgDn.
//! - Delayed wrap at last column (xterm semantics — the cursor "sticks"
//!   at the right margin until the next printable byte).
//!
//! Out of scope: bracketed paste (?2004 is parsed and ignored).

use alloc::collections::VecDeque;
use alloc::string::String;
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

    /// Most recent OSC 0/2 window-title request. The hosting terminal window
    /// drains this and applies it to its enclosing frame.
    pending_title: Option<String>,

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
            pending_title: None,
            using_alt: false,
            stashed: None,
            scrollback: VecDeque::new(),
            view_offset: 0,
        }
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
    /// The app writes these back to the pty master fd.
    pub fn take_replies(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.replies)
    }

    /// Drain the latest OSC 0/2 window-title request.
    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
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

        // Shift rows [top + n ..= bot] up by n — iterated over the
        // *source* rows so the range is empty when n spans the whole
        // region (`top..=bot - n` would underflow there) — then fill
        // the vacated bottom n rows with blanks.
        for row in top + n..=bot {
            let replacement = blank_row.clone();
            self.buffer[row - n] = core::mem::replace(&mut self.buffer[row], replacement);
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
                for row in 0..self.rows {
                    for col in 0..self.cols {
                        self.buffer[row][col] = blank;
                    }
                }
                if mode == 3 {
                    // xterm: ED 3 also clears the scrollback ring.
                    self.scrollback.clear();
                    self.view_offset = 0;
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
                        self.enter_alt_screen();
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

    fn enter_alt_screen(&mut self) {
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

        // Fresh alt buffer state. There is no persistent alt buffer, so
        // ?47 (which xterm would leave dirty from a prior session) and
        // ?1047/?1049 (which clear on entry) all start clean here.
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
        let bot = if bot == 0 { self.rows as u16 } else { bot };
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
            Some(&b) if matches!(b, b'?' | b'>' | b'<' | b'=') => (Some(b), &intermediates[1..]),
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
                let bot = params
                    .get(1)
                    .copied()
                    .filter(|&v| v != 0)
                    .unwrap_or(self.rows as u16);
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

    fn osc_dispatch(&mut self, data: &[u8], _bell_terminated: bool) {
        let Some(separator) = data.iter().position(|&byte| byte == b';') else {
            return;
        };
        if !matches!(&data[..separator], b"0" | b"2") {
            return;
        }
        let Ok(title) = core::str::from_utf8(&data[separator + 1..]) else {
            return;
        };
        let sanitized: String = title
            .chars()
            .filter(|ch| !ch.is_control())
            .take(256)
            .collect();
        self.pending_title = Some(sanitized);
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

