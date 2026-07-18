#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub use runtime::{
    GuiEvent, GUI_EVENT_CLOSE, GUI_EVENT_FOCUS_CHANGE, GUI_EVENT_KEY, GUI_EVENT_MOUSE,
    GUI_EVENT_RESIZE, GUI_MOUSE_DOWN, GUI_MOUSE_MOVE, GUI_MOUSE_SCROLL, GUI_MOUSE_UP,
};

pub const COLOR_BLACK: u32 = 0x000000;
pub const COLOR_WHITE: u32 = 0xFFFFFF;
pub const COLOR_TEXT: u32 = 0x202020;
pub const COLOR_PANEL: u32 = 0xF0F0F0;
pub const COLOR_BORDER: u32 = 0x707070;
pub const COLOR_HIGHLIGHT: u32 = 0x0078D7;

pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels.resize(width as usize * height as usize, 0);
    }

    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    pub fn pixel(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[y as usize * self.width as usize + x as usize] = color;
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: u32) {
        let left = x.max(0) as u32;
        let top = y.max(0) as u32;
        let right = (x.saturating_add(width as i32))
            .max(0)
            .min(self.width as i32) as u32;
        let bottom = (y.saturating_add(height as i32))
            .max(0)
            .min(self.height as i32) as u32;
        for row in top..bottom {
            let start = row as usize * self.width as usize + left as usize;
            let end = row as usize * self.width as usize + right as usize;
            self.pixels[start..end].fill(color);
        }
    }

    pub fn horizontal_line(&mut self, x: i32, y: i32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    pub fn vertical_line(&mut self, x: i32, y: i32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    pub fn rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: u32) {
        self.horizontal_line(x, y, width, color);
        self.horizontal_line(x, y + height.saturating_sub(1) as i32, width, color);
        self.vertical_line(x, y, height, color);
        self.vertical_line(x + width.saturating_sub(1) as i32, y, height, color);
    }

    pub fn draw_char(&mut self, x: i32, y: i32, character: char, color: u32) {
        let code = character as u32;
        if !(32..=126).contains(&code) {
            return;
        }
        let glyph = &fontdata::DEFAULT_8X8_FONT_DATA[(code - 32) as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8 {
                if bits & (1 << (7 - column)) != 0 {
                    self.pixel(x + column, y + row as i32, color);
                }
            }
        }
    }

    pub fn draw_text(&mut self, mut x: i32, y: i32, text: &str, color: u32) {
        for character in text.chars() {
            if character == '\n' {
                break;
            }
            self.draw_char(x, y, character, color);
            x += 8;
        }
    }
}

pub struct Window {
    handle: u32,
    canvas: Canvas,
}

impl Window {
    pub fn new(width: u32, height: u32, title: &str) -> Result<Self, i64> {
        let result = runtime::gui_win_create(width, height, title, 0);
        if result < 0 {
            return Err(result);
        }
        Ok(Self {
            handle: result as u32,
            canvas: Canvas::new(width, height),
        })
    }

    pub fn handle(&self) -> u32 {
        self.handle
    }
    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        &mut self.canvas
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.canvas.resize(width, height);
    }

    pub fn present(&self) -> Result<(), i64> {
        let result = runtime::gui_win_present(
            self.handle,
            self.canvas.pixels(),
            self.canvas.width(),
            self.canvas.height(),
        );
        if result < 0 {
            Err(result)
        } else {
            Ok(())
        }
    }

    pub fn destroy(&mut self) {
        if self.handle != 0 {
            let _ = runtime::gui_win_destroy(self.handle);
            self.handle = 0;
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.destroy();
    }
}

pub fn next_event() -> Result<GuiEvent, i64> {
    let mut event = GuiEvent::default();
    let result = runtime::gui_next_event(&mut event, 0);
    if result < 0 {
        Err(result)
    } else {
        Ok(event)
    }
}

/// `-EAGAIN`, returned by `gui_next_event` under `GUI_NONBLOCK` when the
/// per-process event queue is empty.
const EAGAIN: i64 = -11;

/// Non-blocking variant of [`next_event`]. Returns `Ok(None)` when the event
/// queue is empty instead of parking the process. Lets a self-driven app
/// (e.g. an animation) drain input each frame and keep rendering on its own
/// clock via `runtime::nanosleep`, rather than blocking until the next event.
pub fn try_next_event() -> Result<Option<GuiEvent>, i64> {
    let mut event = GuiEvent::default();
    let result = runtime::gui_next_event(&mut event, runtime::GUI_NONBLOCK);
    if result == EAGAIN {
        Ok(None)
    } else if result < 0 {
        Err(result)
    } else {
        Ok(Some(event))
    }
}

pub struct MenuBar<'a> {
    pub label: &'a str,
    pub items: &'a [&'a str],
    pub open: bool,
}

impl<'a> MenuBar<'a> {
    pub const HEIGHT: u32 = 24;
    pub const ITEM_HEIGHT: u32 = 22;

    pub fn draw(&self, canvas: &mut Canvas) {
        canvas.fill_rect(0, 0, canvas.width(), Self::HEIGHT, COLOR_PANEL);
        canvas.horizontal_line(0, Self::HEIGHT as i32 - 1, canvas.width(), COLOR_BORDER);
        if self.open {
            canvas.fill_rect(4, 3, 48, 18, 0xD8E8F8);
        }
        canvas.draw_text(10, 8, self.label, COLOR_TEXT);
        if self.open {
            let width = 128;
            let height = Self::ITEM_HEIGHT * self.items.len() as u32;
            canvas.fill_rect(4, Self::HEIGHT as i32, width, height, COLOR_PANEL);
            canvas.rect(4, Self::HEIGHT as i32, width, height, COLOR_BORDER);
            for (index, item) in self.items.iter().enumerate() {
                canvas.draw_text(
                    12,
                    Self::HEIGHT as i32 + index as i32 * Self::ITEM_HEIGHT as i32 + 7,
                    item,
                    COLOR_TEXT,
                );
            }
        }
    }

    pub fn click(&mut self, x: i32, y: i32) -> Option<usize> {
        if y >= 0 && y < Self::HEIGHT as i32 && x >= 4 && x < 60 {
            self.open = !self.open;
            return None;
        }
        if self.open && x >= 4 && x < 132 && y >= Self::HEIGHT as i32 {
            let index = ((y - Self::HEIGHT as i32) / Self::ITEM_HEIGHT as i32) as usize;
            if index < self.items.len() {
                self.open = false;
                return Some(index);
            }
        }
        self.open = false;
        None
    }
}

/// Byte index of the character boundary immediately before `index`.
///
/// Shared caret helper used by `TextField` and text-editing apps; moves one
/// grapheme-free `char` to the left, clamped at 0.
pub fn previous_boundary(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// Byte index of the character boundary immediately after `index`.
///
/// Moves one `char` to the right, clamped at `text.len()`.
pub fn next_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(next, _)| index + next)
        .unwrap_or(text.len())
}

/// A clickable push button: filled panel + border + centered 8×8 label.
///
/// Positioned manually by the caller (no layout engine), matching the
/// `MenuBar` idiom. `draw(canvas, hot)` renders the default/hot variant;
/// `hit(x, y)` hit-tests a click. Mouse + accelerator keys only — no focus
/// traversal.
pub struct Button {
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Button {
    pub fn new(label: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            label: String::from(label),
            x,
            y,
            w,
            h,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.w as i32
            && y >= self.y
            && y < self.y + self.h as i32
    }

    pub fn draw(&self, canvas: &mut Canvas, hot: bool) {
        let fill = if hot { COLOR_HIGHLIGHT } else { COLOR_PANEL };
        let text_color = if hot { COLOR_WHITE } else { COLOR_TEXT };
        canvas.fill_rect(self.x, self.y, self.w, self.h, fill);
        canvas.rect(self.x, self.y, self.w, self.h, COLOR_BORDER);
        let text_width = self.label.chars().count() as i32 * 8;
        let text_x = self.x + (self.w as i32 - text_width) / 2;
        let text_y = self.y + (self.h as i32 - 8) / 2;
        canvas.draw_text(text_x.max(self.x + 2), text_y, &self.label, text_color);
    }
}

/// A single-line editable text field with a byte-index caret.
///
/// Owns its `text` and renders a box, clipped/scrolled text (so the caret is
/// always visible), and a caret line. No selection in v1. Feed keyboard with
/// [`TextField::key`] and clicks with [`TextField::click`].
pub struct TextField {
    pub text: String,
    pub caret: usize,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    scroll: usize,
}

impl TextField {
    const PAD: i32 = 5;

    pub fn new(x: i32, y: i32, w: u32, h: u32, text: &str) -> Self {
        let text = String::from(text);
        let caret = text.len();
        Self {
            text,
            caret,
            x,
            y,
            w,
            h,
            scroll: 0,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = String::from(text);
        self.caret = self.text.len();
        self.scroll = 0;
    }

    /// Number of `char`s in `text[..self.caret]` — the caret's column.
    fn caret_column(&self) -> usize {
        self.text[..self.caret].chars().count()
    }

    fn visible_columns(&self) -> usize {
        ((self.w as i32 - Self::PAD * 2) / 8).max(1) as usize
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.w as i32
            && y >= self.y
            && y < self.y + self.h as i32
    }

    /// Place the caret nearest the pixel column of `x` (window coordinates).
    pub fn click(&mut self, x: i32) {
        let column = ((x - self.x - Self::PAD).max(0) / 8) as usize + self.scroll;
        self.caret = self
            .text
            .char_indices()
            .nth(column)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len());
    }

    /// Handle a key press. Returns `true` if the text content changed.
    pub fn key(&mut self, key: u32, character: char) -> bool {
        match key {
            runtime::KEY_LEFT => {
                if self.caret > 0 {
                    self.caret = previous_boundary(&self.text, self.caret);
                }
                false
            }
            runtime::KEY_RIGHT => {
                if self.caret < self.text.len() {
                    self.caret = next_boundary(&self.text, self.caret);
                }
                false
            }
            runtime::KEY_HOME => {
                self.caret = 0;
                false
            }
            runtime::KEY_END => {
                self.caret = self.text.len();
                false
            }
            runtime::KEY_BACKSPACE => {
                if self.caret > 0 {
                    let previous = previous_boundary(&self.text, self.caret);
                    self.text.replace_range(previous..self.caret, "");
                    self.caret = previous;
                    true
                } else {
                    false
                }
            }
            runtime::KEY_DELETE => {
                if self.caret < self.text.len() {
                    let next = next_boundary(&self.text, self.caret);
                    self.text.replace_range(self.caret..next, "");
                    true
                } else {
                    false
                }
            }
            _ if character >= ' ' && character != '\u{7f}' => {
                self.text.insert(self.caret, character);
                self.caret += character.len_utf8();
                true
            }
            _ => false,
        }
    }

    pub fn draw(&mut self, canvas: &mut Canvas, focused: bool) {
        let column = self.caret_column();
        let visible = self.visible_columns();
        if column < self.scroll {
            self.scroll = column;
        } else if column >= self.scroll + visible {
            self.scroll = column + 1 - visible;
        }
        canvas.fill_rect(self.x, self.y, self.w, self.h, COLOR_WHITE);
        canvas.rect(self.x, self.y, self.w, self.h, COLOR_BORDER);
        let text_y = self.y + (self.h as i32 - 8) / 2;
        let mut pixel_x = self.x + Self::PAD;
        for character in self.text.chars().skip(self.scroll).take(visible) {
            canvas.draw_char(pixel_x, text_y, character, COLOR_TEXT);
            pixel_x += 8;
        }
        if focused {
            let caret_x = self.x + Self::PAD + (column - self.scroll) as i32 * 8;
            canvas.vertical_line(caret_x, self.y + 4, self.h.saturating_sub(8), COLOR_TEXT);
        }
    }
}

/// Result of routing a click or key into a [`ListView`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListEvent {
    /// Nothing happened (click outside the rows, no selection to move).
    None,
    /// The selection moved to this row.
    Selected(usize),
    /// This row was activated (Enter on a selection, or a second click on the
    /// already-selected row).
    Activated(usize),
}

/// A scrollable single-column list over `Vec<String>` rows.
///
/// Handles wheel scroll, click-to-select, keyboard selection movement, and
/// activation. The selected row is drawn inverted; a minimal scrollbar gutter
/// appears when the rows overflow the visible page.
pub struct ListView {
    pub rows: Vec<String>,
    pub first_row: usize,
    pub selected: Option<usize>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl ListView {
    pub const ROW_HEIGHT: u32 = 16;

    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            rows: Vec::new(),
            first_row: 0,
            selected: None,
            x,
            y,
            w,
            h,
        }
    }

    pub fn set_rows(&mut self, rows: Vec<String>) {
        self.rows = rows;
        self.first_row = 0;
        self.selected = None;
    }

    pub fn visible_rows(&self) -> usize {
        (self.h / Self::ROW_HEIGHT).max(1) as usize
    }

    fn ensure_visible(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let visible = self.visible_rows();
        if selected < self.first_row {
            self.first_row = selected;
        } else if selected >= self.first_row + visible {
            self.first_row = selected + 1 - visible;
        }
    }

    pub fn scroll(&mut self, delta: i32) {
        if delta < 0 {
            self.first_row = self.first_row.saturating_sub((-delta) as usize);
        } else {
            let max_first = self.rows.len().saturating_sub(self.visible_rows());
            self.first_row = (self.first_row + delta as usize).min(max_first);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = match self.selected {
            None => {
                if delta < 0 {
                    last
                } else {
                    0
                }
            }
            Some(current) => {
                if delta < 0 {
                    current.saturating_sub((-delta) as usize)
                } else {
                    (current + delta as usize).min(last)
                }
            }
        };
        self.selected = Some(next);
        self.ensure_visible();
    }

    /// Handle a key press. Returns [`ListEvent::Activated`] on Enter over a
    /// selected row, [`ListEvent::Selected`] on a movement, else `None`.
    pub fn key(&mut self, key: u32) -> ListEvent {
        let page = self.visible_rows() as isize;
        match key {
            runtime::KEY_UP => self.move_selection(-1),
            runtime::KEY_DOWN => self.move_selection(1),
            runtime::KEY_PAGE_UP => self.move_selection(-page),
            runtime::KEY_PAGE_DOWN => self.move_selection(page),
            runtime::KEY_HOME => self.move_selection(-(self.rows.len() as isize)),
            runtime::KEY_END => self.move_selection(self.rows.len() as isize),
            runtime::KEY_ENTER => {
                if let Some(selected) = self.selected {
                    return ListEvent::Activated(selected);
                }
                return ListEvent::None;
            }
            _ => return ListEvent::None,
        }
        self.selected.map(ListEvent::Selected).unwrap_or(ListEvent::None)
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.w as i32
            && y >= self.y
            && y < self.y + self.h as i32
    }

    /// Route a click. A click on the already-selected row activates it
    /// (second-click idiom); a click on a different row selects it.
    pub fn click(&mut self, x: i32, y: i32) -> ListEvent {
        if !self.hit(x, y) {
            return ListEvent::None;
        }
        let row = self.first_row + ((y - self.y) / Self::ROW_HEIGHT as i32) as usize;
        if row >= self.rows.len() {
            return ListEvent::None;
        }
        if self.selected == Some(row) {
            ListEvent::Activated(row)
        } else {
            self.selected = Some(row);
            ListEvent::Selected(row)
        }
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, COLOR_WHITE);
        let visible = self.visible_rows();
        let overflow = self.rows.len() > visible;
        let gutter = if overflow { 6 } else { 0 };
        let text_width = self.w as i32 - gutter - 4;
        let max_chars = (text_width / 8).max(1) as usize;
        for slot in 0..visible {
            let row = self.first_row + slot;
            let Some(text) = self.rows.get(row) else {
                break;
            };
            let row_y = self.y + slot as i32 * Self::ROW_HEIGHT as i32;
            let (bg, fg) = if self.selected == Some(row) {
                (COLOR_HIGHLIGHT, COLOR_WHITE)
            } else {
                (COLOR_WHITE, COLOR_TEXT)
            };
            if bg != COLOR_WHITE {
                canvas.fill_rect(self.x, row_y, self.w - gutter as u32, Self::ROW_HEIGHT, bg);
            }
            let clipped: String = text.chars().take(max_chars).collect();
            canvas.draw_text(self.x + 4, row_y + 4, &clipped, fg);
        }
        canvas.rect(self.x, self.y, self.w, self.h, COLOR_BORDER);
        if overflow {
            let gutter_x = self.x + self.w as i32 - gutter;
            canvas.vertical_line(gutter_x, self.y, self.h, COLOR_BORDER);
            let total = self.rows.len();
            let track = self.h as i32 - 2;
            let thumb_h = ((visible * track as usize) / total).max(8) as i32;
            let max_first = total.saturating_sub(visible).max(1);
            let thumb_y = self.y + 1 + (self.first_row as i32 * (track - thumb_h)) / max_first as i32;
            canvas.fill_rect(gutter_x + 1, thumb_y, gutter as u32 - 2, thumb_h as u32, COLOR_BORDER);
        }
    }
}

#[derive(Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

pub fn c_path(path: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(path.len() + 1);
    bytes.extend_from_slice(path.as_bytes());
    bytes.push(0);
    bytes
}

pub fn list_dir(path: &str) -> Result<Vec<DirEntry>, i64> {
    let directory = path;
    let path = c_path(directory);
    let fd = runtime::openat(
        runtime::AT_FDCWD,
        &path,
        runtime::O_RDONLY | runtime::O_DIRECTORY,
        0,
    );
    if fd < 0 {
        return Err(fd);
    }
    let fd = fd as i32;
    let mut output = Vec::new();
    let mut buffer = vec![0u8; 4096];
    loop {
        let count = runtime::getdents64(fd, &mut buffer);
        if count < 0 {
            let _ = runtime::close(fd);
            return Err(count);
        }
        if count == 0 {
            break;
        }
        let mut offset = 0usize;
        while offset + 19 <= count as usize {
            let reclen = u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
            if reclen < 20 || offset + reclen > count as usize {
                break;
            }
            let kind = buffer[offset + 18];
            let name_start = offset + 19;
            let name_end = buffer[name_start..offset + reclen]
                .iter()
                .position(|byte| *byte == 0)
                .map(|end| name_start + end)
                .unwrap_or(offset + reclen);
            if let Ok(name) = String::from_utf8(buffer[name_start..name_end].to_vec()) {
                if name != "." && name != ".." {
                    let is_dir = if kind == 4 {
                        true
                    } else if kind == 0 {
                        stat_is_dir(directory, &name)
                    } else {
                        false
                    };
                    output.push(DirEntry { name, is_dir });
                }
            }
            offset += reclen;
        }
    }
    let _ = runtime::close(fd);
    Ok(output)
}

fn stat_is_dir(directory: &str, name: &str) -> bool {
    let mut full_path = String::from(directory.trim_end_matches('/'));
    if full_path.is_empty() {
        full_path.push('/');
    } else if !full_path.ends_with('/') {
        full_path.push('/');
    }
    full_path.push_str(name);
    let path = c_path(&full_path);
    let fd = runtime::openat(runtime::AT_FDCWD, &path, runtime::O_RDONLY, 0);
    if fd < 0 {
        return false;
    }
    let mut stat = runtime::LinuxStat::default();
    let result = runtime::fstat(fd as i32, &mut stat);
    let _ = runtime::close(fd as i32);
    result == 0 && stat.st_mode & 0o170000 == 0o040000
}
