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
