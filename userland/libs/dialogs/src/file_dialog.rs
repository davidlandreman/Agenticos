//! Modal file Open / Save dialog.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use gui::{Button, DirEntry, ListEvent, ListView, TextField, Window, COLOR_PANEL, COLOR_TEXT};

use crate::path::{directory_for_input, join_path, parent_directory};
use crate::DialogStatus;

/// Open vs Save behavior for a [`FileDialog`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Open,
    Save,
}

const MARGIN: i32 = 12;

/// A modal file chooser with a scrollable directory listing, `..` navigation,
/// an editable name field, and Open/Save + Cancel buttons.
///
/// Result is an absolute path (`String`). A field value starting with `/` is
/// taken verbatim (type-a-full-path escape hatch); otherwise it is joined to
/// the current directory.
pub struct FileDialog {
    window: Window,
    current_dir: String,
    entries: Vec<DirEntry>,
    list: ListView,
    name: TextField,
    confirm: Button,
    cancel: Button,
    error: bool,
}

impl FileDialog {
    /// Open dialog rooted at `start_dir`.
    pub fn open(start_dir: &str) -> Result<Self, i64> {
        Self::new(FileMode::Open, start_dir)
    }

    /// Save dialog pre-filled from `suggested_path` (directory + filename).
    pub fn save(suggested_path: &str) -> Result<Self, i64> {
        Self::new(FileMode::Save, suggested_path)
    }

    fn new(mode: FileMode, input: &str) -> Result<Self, i64> {
        let title = match mode {
            FileMode::Open => "Open File",
            FileMode::Save => "Save File As",
        };
        let window = Window::new(560, 380, title)?;
        let current_dir = normalize_dir(directory_for_input(input));
        let suggested_name = if input.ends_with('/') {
            String::new()
        } else {
            input.rsplit('/').next().unwrap_or("").to_string()
        };
        let mut dialog = Self {
            window,
            current_dir,
            entries: Vec::new(),
            list: ListView::new(0, 0, 0, 0),
            name: TextField::new(0, 0, 0, 0, &suggested_name),
            confirm: Button::new(
                match mode {
                    FileMode::Open => "Open",
                    FileMode::Save => "Save",
                },
                0,
                0,
                88,
                26,
            ),
            cancel: Button::new("Cancel", 0, 0, 88, 26),
            error: false,
        };
        dialog.relayout();
        dialog.relist();
        dialog.render();
        Ok(dialog)
    }

    pub fn window_handle(&self) -> u32 {
        self.window.handle()
    }

    fn relayout(&mut self) {
        let width = self.window.canvas().width().max(360) as i32;
        let height = self.window.canvas().height().max(220) as i32;
        let list_top = 30;
        let button_h = 26;
        let field_h = 24;
        let bottom_block = button_h + 10 + field_h + 10;
        let list_h = (height - list_top - bottom_block).max(48);
        self.list.x = MARGIN;
        self.list.y = list_top;
        self.list.w = (width - MARGIN * 2) as u32;
        self.list.h = list_h as u32;

        let field_y = list_top + list_h + 8;
        let field_label = MARGIN + 48;
        self.name.x = field_label;
        self.name.y = field_y;
        self.name.w = (width - field_label - MARGIN) as u32;
        self.name.h = field_h as u32;

        let button_y = field_y + field_h + 8;
        self.cancel.x = width - MARGIN - self.cancel.w as i32;
        self.cancel.y = button_y;
        self.confirm.x = self.cancel.x - 12 - self.confirm.w as i32;
        self.confirm.y = button_y;
    }

    fn relist(&mut self) {
        let mut entries: Vec<DirEntry> = Vec::new();
        match gui::list_dir(&self.current_dir) {
            Ok(mut listed) => {
                self.error = false;
                listed.sort_by(|a, b| (!a.is_dir, &a.name).cmp(&(!b.is_dir, &b.name)));
                if self.current_dir != "/" {
                    entries.push(DirEntry {
                        name: "..".to_string(),
                        is_dir: true,
                    });
                }
                entries.extend(listed);
            }
            Err(_) => self.error = true,
        }
        let rows: Vec<String> = if self.error {
            vec!["(cannot list directory)".to_string()]
        } else {
            entries
                .iter()
                .map(|entry| {
                    if entry.is_dir && entry.name != ".." {
                        format!("[DIR] {}", entry.name)
                    } else {
                        entry.name.clone()
                    }
                })
                .collect()
        };
        self.entries = entries;
        self.list.set_rows(rows);
    }

    fn render(&mut self) {
        let dir_line = format!("Directory: {}", self.current_dir);
        let name_label_y = self.name.y + 8;
        let canvas = self.window.canvas_mut();
        canvas.clear(COLOR_PANEL);
        canvas.draw_text(MARGIN, 12, &dir_line, COLOR_TEXT);
        self.list.draw(canvas);
        canvas.draw_text(MARGIN, name_label_y, "Name:", COLOR_TEXT);
        self.name.draw(canvas, true);
        self.confirm.draw(canvas, true);
        self.cancel.draw(canvas, false);
        let _ = self.window.present();
    }

    /// Resolve the final path from the name field per the escape-hatch rule.
    fn resolve(&self) -> String {
        let field = self.name.text.trim();
        if field.starts_with('/') {
            field.to_string()
        } else {
            join_path(&self.current_dir, field, false)
        }
    }

    /// Act on a row: navigate into directories / `..`, confirm files.
    fn activate(&mut self, index: usize) -> DialogStatus<String> {
        let Some(entry) = self.entries.get(index).cloned() else {
            return DialogStatus::Pending;
        };
        if entry.name == ".." {
            self.current_dir = normalize_dir(parent_directory(&self.current_dir));
            self.relist();
            self.render();
            return DialogStatus::Pending;
        }
        if entry.is_dir {
            self.current_dir = normalize_dir(&join_path(&self.current_dir, &entry.name, true));
            self.relist();
            self.render();
            return DialogStatus::Pending;
        }
        // A file row: fill the name and confirm.
        self.name.set_text(&entry.name);
        DialogStatus::Done(Some(join_path(&self.current_dir, &entry.name, false)))
    }

    /// Copy a selected file's name into the field (no confirm).
    fn select(&mut self, index: usize) {
        if let Some(entry) = self.entries.get(index) {
            if !entry.is_dir {
                self.name.set_text(&entry.name);
            }
        }
    }

    pub fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<String> {
        match event.kind {
            runtime::GUI_EVENT_CLOSE => return DialogStatus::Done(None),
            runtime::GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                self.relayout();
                self.render();
            }
            runtime::GUI_EVENT_KEY if event.payload[3] != 0 => {
                let key = event.payload[0];
                let character = char::from_u32(event.payload[1]).unwrap_or('\0');
                match key {
                    runtime::KEY_ESCAPE => return DialogStatus::Done(None),
                    runtime::KEY_ENTER => {
                        // Enter over a directory / `..` navigates; otherwise confirm.
                        if let Some(selected) = self.list.selected {
                            if self
                                .entries
                                .get(selected)
                                .map(|entry| entry.is_dir)
                                .unwrap_or(false)
                            {
                                return self.activate(selected);
                            }
                        }
                        return DialogStatus::Done(Some(self.resolve()));
                    }
                    runtime::KEY_UP
                    | runtime::KEY_DOWN
                    | runtime::KEY_PAGE_UP
                    | runtime::KEY_PAGE_DOWN => {
                        if let ListEvent::Selected(index) = self.list.key(key) {
                            self.select(index);
                        }
                        self.render();
                    }
                    _ => {
                        self.name.key(key, character);
                        self.render();
                    }
                }
            }
            runtime::GUI_EVENT_MOUSE => {
                let x = event.payload[0] as i32;
                let y = event.payload[1] as i32;
                match event.payload[3] {
                    runtime::GUI_MOUSE_SCROLL if self.list.hit(x, y) => {
                        self.list.scroll(event.payload[5] as i32);
                        self.render();
                    }
                    runtime::GUI_MOUSE_DOWN => {
                        if self.confirm.hit(x, y) {
                            return DialogStatus::Done(Some(self.resolve()));
                        }
                        if self.cancel.hit(x, y) {
                            return DialogStatus::Done(None);
                        }
                        if self.name.hit(x, y) {
                            self.name.click(x);
                            self.render();
                        } else if self.list.hit(x, y) {
                            match self.list.click(x, y) {
                                ListEvent::Activated(index) => return self.activate(index),
                                ListEvent::Selected(index) => {
                                    self.select(index);
                                    self.render();
                                }
                                ListEvent::None => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        DialogStatus::Pending
    }
}

/// Collapse a path to a canonical directory string (no trailing slash except root).
fn normalize_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}
