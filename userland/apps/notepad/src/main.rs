#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use dialogs::{DialogStatus, FileDialog, MessageBox, MessageChoice, Modal, ModalOutcome};
use gui::{
    next_boundary, previous_boundary, Canvas, MenuBar, Window, COLOR_BORDER, COLOR_HIGHLIGHT,
    COLOR_PANEL, COLOR_TEXT, COLOR_WHITE, GUI_EVENT_CLOSE, GUI_EVENT_KEY, GUI_EVENT_MOUSE,
    GUI_EVENT_RESIZE, GUI_MOUSE_DOWN, GUI_MOUSE_SCROLL,
};

const FILE_ITEMS: &[&str] = &["New", "Open...", "Save", "Save As...", "Exit"];
const STATUS_HEIGHT: u32 = 18;
const LINE_HEIGHT: u32 = 10;

struct Editor {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    first_line: usize,
}

impl Editor {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: None,
            first_line: 0,
        }
    }

    fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = 0;
        self.anchor = None;
        self.first_line = 0;
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            None
        } else {
            Some((anchor.min(self.cursor), anchor.max(self.cursor)))
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    fn begin_move(&mut self, shift: bool) {
        if shift {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
    }

    fn insert(&mut self, character: char) {
        self.delete_selection();
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let previous = previous_boundary(&self.text, self.cursor);
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        true
    }

    fn delete(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor == self.text.len() {
            return false;
        }
        let next = next_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
        true
    }

    fn move_horizontal(&mut self, right: bool, shift: bool) {
        self.begin_move(shift);
        self.cursor = if right {
            if self.cursor < self.text.len() {
                next_boundary(&self.text, self.cursor)
            } else {
                self.cursor
            }
        } else if self.cursor > 0 {
            previous_boundary(&self.text, self.cursor)
        } else {
            0
        };
    }

    fn line_col(&self) -> (usize, usize) {
        line_col_at(&self.text, self.cursor)
    }

    fn move_vertical(&mut self, delta: isize, shift: bool) {
        let (line, column) = self.line_col();
        let target = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            line.saturating_add(delta as usize)
        };
        self.begin_move(shift);
        self.cursor = index_for_line_col(&self.text, target, column);
    }

    fn home_end(&mut self, end: bool, shift: bool) {
        let (line, _) = self.line_col();
        self.begin_move(shift);
        self.cursor = index_for_line_col(&self.text, line, if end { usize::MAX } else { 0 });
    }

    fn ensure_visible(&mut self, visible_lines: usize) {
        let (line, _) = self.line_col();
        if line < self.first_line {
            self.first_line = line;
        }
        if line >= self.first_line.saturating_add(visible_lines.max(1)) {
            self.first_line = line + 1 - visible_lines.max(1);
        }
    }

    fn click(&mut self, row: usize, column: usize) {
        self.cursor = index_for_line_col(&self.text, self.first_line + row, column);
        self.anchor = None;
    }

    fn draw(&self, canvas: &mut Canvas, top: u32, bottom: u32, focused: bool) {
        canvas.fill_rect(
            0,
            top as i32,
            canvas.width(),
            bottom.saturating_sub(top),
            COLOR_WHITE,
        );
        let selected = self.selection();
        let mut line = 0usize;
        let mut column = 0usize;
        for (index, character) in self.text.char_indices() {
            if character == '\n' {
                line += 1;
                column = 0;
                continue;
            }
            if line >= self.first_line {
                let visible_line = line - self.first_line;
                let y = top as i32 + visible_line as i32 * LINE_HEIGHT as i32 + 1;
                if y + 8 >= bottom as i32 {
                    break;
                }
                let x = 4 + column as i32 * 8;
                if selected
                    .map(|(start, end)| index >= start && index < end)
                    .unwrap_or(false)
                {
                    canvas.fill_rect(x, y - 1, 8, LINE_HEIGHT, COLOR_HIGHLIGHT);
                    canvas.draw_char(x, y, character, COLOR_WHITE);
                } else {
                    canvas.draw_char(x, y, character, COLOR_TEXT);
                }
            }
            column += 1;
        }
        if focused {
            let (cursor_line, cursor_column) = self.line_col();
            if cursor_line >= self.first_line {
                let y = top as i32 + (cursor_line - self.first_line) as i32 * LINE_HEIGHT as i32;
                if y + LINE_HEIGHT as i32 <= bottom as i32 {
                    canvas.vertical_line(4 + cursor_column as i32 * 8, y + 1, 9, COLOR_TEXT);
                }
            }
        }
    }
}

/// Why the current modal is open, so `on_modal_done` knows how to act on its
/// outcome. The library dialog itself is purpose-agnostic.
#[derive(Clone, Copy)]
enum ModalPurpose {
    Open,
    SaveAs,
    ExitConfirm,
    Dismiss,
}

struct Notepad {
    window: Window,
    editor: Editor,
    menu: MenuBar<'static>,
    path: Option<String>,
    dirty: bool,
    focused: bool,
    modal: Option<Modal>,
    modal_purpose: ModalPurpose,
}

impl Notepad {
    fn new(initial_path: Option<String>) -> Result<Self, i64> {
        let mut app = Self {
            window: Window::new(720, 480, "Notepad")?,
            editor: Editor::new(),
            menu: MenuBar {
                label: "File",
                items: FILE_ITEMS,
                open: false,
            },
            path: None,
            dirty: false,
            focused: true,
            modal: None,
            modal_purpose: ModalPurpose::Dismiss,
        };
        if let Some(path) = initial_path {
            app.load_from(&path);
        }
        Ok(app)
    }

    fn run(&mut self) -> i64 {
        self.render();
        loop {
            let event = match gui::next_event() {
                Ok(event) => event,
                Err(error) => return error,
            };
            let exit = if event.window == self.window.handle() {
                self.handle_main(event)
            } else if self.modal.as_ref().map(Modal::window_handle) == Some(event.window) {
                self.handle_modal(&event)
            } else {
                false
            };
            if exit {
                return 0;
            }
        }
    }

    fn render(&mut self) {
        let height = self.window.canvas().height();
        let editor_bottom = height.saturating_sub(STATUS_HEIGHT);
        let path = self.path.as_deref().unwrap_or("Untitled");
        let dirty = self.dirty;
        let focused = self.focused && self.modal.is_none();
        let canvas = self.window.canvas_mut();
        canvas.clear(COLOR_PANEL);
        self.editor
            .draw(canvas, MenuBar::HEIGHT, editor_bottom, focused);
        canvas.fill_rect(
            0,
            editor_bottom as i32,
            canvas.width(),
            STATUS_HEIGHT,
            COLOR_PANEL,
        );
        canvas.horizontal_line(0, editor_bottom as i32, canvas.width(), COLOR_BORDER);
        canvas.draw_text(6, editor_bottom as i32 + 5, path, COLOR_TEXT);
        if dirty {
            canvas.draw_text(
                canvas.width() as i32 - 80,
                editor_bottom as i32 + 5,
                "Modified",
                0xA03020,
            );
        }
        self.menu.draw(canvas);
        let _ = self.window.present();
    }

    fn handle_main(&mut self, event: runtime::GuiEvent) -> bool {
        match event.kind {
            GUI_EVENT_CLOSE => return self.request_exit(),
            gui::GUI_EVENT_FOCUS_CHANGE => self.focused = event.payload[0] != 0,
            GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                let lines = self.visible_lines();
                self.editor.ensure_visible(lines);
            }
            GUI_EVENT_KEY if event.payload[3] != 0 && self.modal.is_none() => {
                if self.handle_key(event.payload) {
                    return true;
                }
            }
            GUI_EVENT_MOUSE if self.modal.is_none() => {
                // Pointer motion and button-up do not change notepad state.
                // Avoid turning those high-frequency events into full-surface
                // presents; clicks and scrolling still need a repaint.
                if event.payload[3] != GUI_MOUSE_DOWN && event.payload[3] != GUI_MOUSE_SCROLL {
                    return false;
                }
                if self.handle_mouse(event.payload) {
                    return true;
                }
            }
            _ => return false,
        }
        self.render();
        false
    }

    fn handle_key(&mut self, payload: [u32; 6]) -> bool {
        let key = payload[0];
        let character = char::from_u32(payload[1]).unwrap_or('\0');
        let shift = payload[2] & 1 != 0;
        let ctrl = payload[2] & 2 != 0;
        if ctrl {
            match character.to_ascii_lowercase() {
                'n' => self.new_file(),
                'o' => self.open_dialog(),
                's' if shift => self.save_as_dialog(),
                's' => self.save(),
                _ => {}
            }
            return false;
        }
        let changed = match key {
            runtime::KEY_BACKSPACE => self.editor.backspace(),
            runtime::KEY_DELETE => self.editor.delete(),
            runtime::KEY_LEFT => {
                self.editor.move_horizontal(false, shift);
                false
            }
            runtime::KEY_RIGHT => {
                self.editor.move_horizontal(true, shift);
                false
            }
            runtime::KEY_UP => {
                self.editor.move_vertical(-1, shift);
                false
            }
            runtime::KEY_DOWN => {
                self.editor.move_vertical(1, shift);
                false
            }
            runtime::KEY_HOME => {
                self.editor.home_end(false, shift);
                false
            }
            runtime::KEY_END => {
                self.editor.home_end(true, shift);
                false
            }
            runtime::KEY_ENTER => {
                self.editor.insert('\n');
                true
            }
            runtime::KEY_TAB => {
                for _ in 0..4 {
                    self.editor.insert(' ');
                }
                true
            }
            _ if character >= ' ' && character != '\u{7f}' => {
                self.editor.insert(character);
                true
            }
            _ => false,
        };
        if changed {
            self.dirty = true;
        }
        self.editor.ensure_visible(self.visible_lines());
        false
    }

    fn handle_mouse(&mut self, payload: [u32; 6]) -> bool {
        let x = payload[0] as i32;
        let y = payload[1] as i32;
        if payload[3] == GUI_MOUSE_SCROLL {
            let delta = payload[5] as i32;
            if delta < 0 {
                self.editor.first_line = self.editor.first_line.saturating_sub((-delta) as usize);
            } else {
                self.editor.first_line = self.editor.first_line.saturating_add(delta as usize);
            }
            return false;
        }
        if payload[3] != GUI_MOUSE_DOWN {
            return false;
        }
        if let Some(index) = self.menu.click(x, y) {
            match index {
                0 => self.new_file(),
                1 => self.open_dialog(),
                2 => self.save(),
                3 => self.save_as_dialog(),
                4 => return self.request_exit(),
                _ => {}
            }
        } else if y >= MenuBar::HEIGHT as i32 {
            let row = ((y - MenuBar::HEIGHT as i32) / LINE_HEIGHT as i32).max(0) as usize;
            let column = ((x - 4) / 8).max(0) as usize;
            self.editor.click(row, column);
        }
        false
    }

    fn visible_lines(&self) -> usize {
        self.window
            .canvas()
            .height()
            .saturating_sub(MenuBar::HEIGHT + STATUS_HEIGHT) as usize
            / LINE_HEIGHT as usize
    }

    fn new_file(&mut self) {
        self.editor.set_text(String::new());
        self.path = None;
        self.dirty = false;
    }

    fn save(&mut self) {
        if let Some(path) = self.path.clone() {
            self.save_to(&path);
        } else {
            self.save_as_dialog();
        }
    }

    fn save_to(&mut self, path: &str) {
        let cpath = gui::c_path(path);
        let fd = runtime::openat(
            runtime::AT_FDCWD,
            &cpath,
            runtime::O_WRONLY | runtime::O_CREAT | runtime::O_TRUNC,
            0o644,
        );
        if fd < 0 {
            self.show_error(&format!("Save failed ({fd})"));
            return;
        }
        let fd = fd as i32;
        let bytes = self.editor.text.as_bytes();
        let mut offset = 0usize;
        while offset < bytes.len() {
            let written = runtime::write(fd, &bytes[offset..]);
            if written <= 0 {
                let _ = runtime::close(fd);
                self.show_error(&format!("Write failed ({written})"));
                return;
            }
            offset += written as usize;
        }
        let _ = runtime::close(fd);
        self.path = Some(path.to_string());
        self.dirty = false;
    }

    fn load_from(&mut self, path: &str) {
        let cpath = gui::c_path(path);
        let fd = runtime::openat(runtime::AT_FDCWD, &cpath, runtime::O_RDONLY, 0);
        if fd < 0 {
            self.show_error(&format!("Open failed ({fd})"));
            return;
        }
        let fd = fd as i32;
        let mut bytes = Vec::new();
        let mut chunk = vec![0u8; 4096];
        loop {
            let count = runtime::read(fd, &mut chunk);
            if count < 0 {
                let _ = runtime::close(fd);
                self.show_error(&format!("Read failed ({count})"));
                return;
            }
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count as usize]);
        }
        let _ = runtime::close(fd);
        match String::from_utf8(bytes) {
            Ok(text) => {
                self.editor.set_text(text);
                self.path = Some(path.to_string());
                self.dirty = false;
            }
            Err(_) => self.show_error("File is not UTF-8 text"),
        }
    }

    fn open_dialog(&mut self) {
        if self.modal.is_some() {
            return;
        }
        match FileDialog::open("/host/") {
            Ok(dialog) => {
                self.modal_purpose = ModalPurpose::Open;
                self.modal = Some(Modal::File(dialog));
            }
            Err(error) => self.show_error(&format!("Dialog failed ({error})")),
        }
    }

    fn save_as_dialog(&mut self) {
        if self.modal.is_some() {
            return;
        }
        let suggested = self.path.as_deref().unwrap_or("/UNTITLED.TXT");
        match FileDialog::save(suggested) {
            Ok(dialog) => {
                self.modal_purpose = ModalPurpose::SaveAs;
                self.modal = Some(Modal::File(dialog));
            }
            Err(error) => self.show_error(&format!("Dialog failed ({error})")),
        }
    }

    fn show_error(&mut self, text: &str) {
        if let Ok(dialog) = MessageBox::error(text) {
            self.modal_purpose = ModalPurpose::Dismiss;
            self.modal = Some(Modal::Message(dialog));
        }
    }

    fn request_exit(&mut self) -> bool {
        if !self.dirty {
            return true;
        }
        if self.modal.is_none() {
            if let Ok(dialog) = MessageBox::confirm(
                "Unsaved Changes",
                "This document has unsaved changes. Exit without saving?",
            ) {
                self.modal_purpose = ModalPurpose::ExitConfirm;
                self.modal = Some(Modal::Message(dialog));
            }
        }
        false
    }

    fn handle_modal(&mut self, event: &runtime::GuiEvent) -> bool {
        let Some(modal) = self.modal.as_mut() else {
            return false;
        };
        if let DialogStatus::Done(outcome) = modal.handle_event(event) {
            self.modal = None;
            let exit = self.on_modal_done(outcome);
            self.render();
            return exit;
        }
        false
    }

    /// Act on a finished modal according to why it was opened. Returns `true`
    /// when the app should exit (dirty-close confirmed).
    fn on_modal_done(&mut self, outcome: Option<ModalOutcome>) -> bool {
        match (self.modal_purpose, outcome) {
            (ModalPurpose::Open, Some(ModalOutcome::Path(path))) => self.load_from(&path),
            (ModalPurpose::SaveAs, Some(ModalOutcome::Path(path))) => self.save_to(&path),
            (ModalPurpose::ExitConfirm, Some(ModalOutcome::Choice(MessageChoice::Yes))) => {
                return true
            }
            _ => {}
        }
        false
    }
}

fn line_col_at(text: &str, index: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut column = 0usize;
    for character in text[..index].chars() {
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn index_for_line_col(text: &str, target_line: usize, target_column: usize) -> usize {
    let mut line = 0usize;
    let mut column = 0usize;
    for (index, character) in text.char_indices() {
        if line == target_line && (column == target_column || character == '\n') {
            return index;
        }
        if character == '\n' {
            line += 1;
            column = 0;
        } else if line == target_line {
            column += 1;
        }
        if line > target_line {
            return index;
        }
    }
    text.len()
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {}",
        "ud2",
        sym notepad_main,
    );
}

unsafe extern "C" fn notepad_main(stack: *const u64) -> ! {
    let startup = runtime::startup_from_stack(stack);
    let initial_path = startup.argv.get(1).and_then(|pointer| c_string(*pointer));
    let code = match Notepad::new(initial_path) {
        Ok(mut app) => app.run(),
        Err(error) => error,
    };
    runtime::exit(if code == 0 { 0 } else { 1 })
}

unsafe fn c_string(pointer: *const u8) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    while length < 4096 && core::ptr::read(pointer.add(length)) != 0 {
        length += 1;
    }
    core::str::from_utf8(core::slice::from_raw_parts(pointer, length))
        .ok()
        .map(ToString::to_string)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}
