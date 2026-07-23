#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use dialogs::{
    DialogStatus, FileDialog, FileDialogOptions, FileFilter, MessageBox, MessageChoice, Modal,
    ModalOutcome,
};
use gui::{
    decode_control_input, theme, ControlInput, MenuBar, Rect, TextArea, TextAreaAction,
    TextAreaOptions, Window, GUI_EVENT_CLOSE, GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_RESIZE,
    GUI_MOUSE_DOWN, GUI_MOUSE_MOVE,
};

const FILE_ITEMS: &[&str] = &["New", "Open...", "Save", "Save As...", "Exit"];
const STATUS_HEIGHT: u32 = 18;

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
    editor: TextArea,
    menu: MenuBar<'static>,
    path: Option<String>,
    dirty: bool,
    focused: bool,
    modal: Option<Modal>,
    modal_purpose: ModalPurpose,
    last_dialog_dir: String,
}

impl Notepad {
    fn new(initial_path: Option<String>) -> Result<Self, i64> {
        let mut app = Self {
            window: Window::new(720, 480, "Notepad")?,
            editor: TextArea::new(
                Rect::new(
                    0,
                    MenuBar::HEIGHT as i32,
                    720,
                    480 - MenuBar::HEIGHT - STATUS_HEIGHT,
                ),
                TextAreaOptions::default(),
            ),
            menu: MenuBar::new("File", FILE_ITEMS),
            path: None,
            dirty: false,
            focused: true,
            modal: None,
            modal_purpose: ModalPurpose::Dismiss,
            last_dialog_dir: "/host".to_string(),
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
            if event.kind == gui::GUI_EVENT_THEME_CHANGED {
                self.render();
                if let Some(modal) = self.modal.as_mut() {
                    modal.refresh_theme();
                }
                continue;
            }
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
        let width = self.window.canvas().width();
        let height = self.window.canvas().height();
        let editor_bottom = height.saturating_sub(STATUS_HEIGHT);
        let path = self.path.as_deref().unwrap_or("Untitled");
        let dirty = self.dirty;
        let focused = self.focused && self.modal.is_none();
        let (line, column) = self.editor.line_col();
        let position = format!("Ln {}, Col {}", line + 1, column + 1);
        self.editor.set_bounds(Rect::new(
            0,
            MenuBar::HEIGHT as i32,
            width,
            editor_bottom.saturating_sub(MenuBar::HEIGHT),
        ));
        let palette = theme::palette();
        let canvas = self.window.canvas_mut();
        canvas.clear(palette.content_bg);
        self.editor.draw(canvas, focused);
        canvas.fill_rect(
            0,
            editor_bottom as i32,
            canvas.width(),
            STATUS_HEIGHT,
            palette.content_bg,
        );
        canvas.horizontal_line(0, editor_bottom as i32, canvas.width(), palette.border);
        canvas.draw_text(6, editor_bottom as i32 + 5, path, palette.text);
        let position_x = canvas.width() as i32
            - position.len() as i32 * gui::FONT_CELL_WIDTH
            - if dirty { 92 } else { 8 };
        canvas.draw_text(
            position_x.max(160),
            editor_bottom as i32 + 5,
            &position,
            palette.text,
        );
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
            gui::GUI_EVENT_FOCUS_CHANGE => {
                self.focused = event.payload[0] != 0;
                if !self.focused {
                    self.editor.cancel_interaction();
                }
            }
            GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
            }
            GUI_EVENT_KEY if event.payload[3] != 0 && self.modal.is_none() => {
                if self.handle_key(&event) {
                    return true;
                }
            }
            GUI_EVENT_MOUSE if self.modal.is_none() => {
                if self.handle_mouse(&event) {
                    return true;
                }
            }
            _ => return false,
        }
        self.render();
        false
    }

    fn handle_key(&mut self, event: &runtime::GuiEvent) -> bool {
        let character = char::from_u32(event.payload[1]).unwrap_or('\0');
        let shift = event.payload[2] & 1 != 0;
        let ctrl = event.payload[2] & 2 != 0;
        if ctrl {
            match character.to_ascii_lowercase() {
                'n' => self.new_file(),
                'o' => self.open_dialog(),
                's' if shift => self.save_as_dialog(),
                's' => self.save(),
                _ => {}
            }
            if matches!(character.to_ascii_lowercase(), 'n' | 'o' | 's') {
                return false;
            }
        }
        let Some(ControlInput::Key(input)) = decode_control_input(event) else {
            return false;
        };
        let response = self.editor.handle_input(ControlInput::Key(input), true);
        if response.action == Some(TextAreaAction::Changed) {
            self.dirty = true;
        }
        false
    }

    fn handle_mouse(&mut self, event: &runtime::GuiEvent) -> bool {
        let x = event.payload[0] as i32;
        let y = event.payload[1] as i32;
        let cursor = self
            .editor
            .cursor_icon_at(x, y)
            .unwrap_or(gui::CursorIcon::Arrow);
        let _ = self.window.set_cursor(cursor);
        if event.payload[3] == GUI_MOUSE_MOVE {
            self.menu.pointer_move(x, y);
        }
        if event.payload[3] == GUI_MOUSE_DOWN && y < MenuBar::HEIGHT as i32 {
            if let Some(index) = self.menu.click(x, y) {
                match index {
                    0 => self.new_file(),
                    1 => self.open_dialog(),
                    2 => self.save(),
                    3 => self.save_as_dialog(),
                    4 => return self.request_exit(),
                    _ => {}
                }
            }
            return false;
        }
        if event.payload[3] == GUI_MOUSE_DOWN && self.menu.open {
            if let Some(index) = self.menu.click(x, y) {
                match index {
                    0 => self.new_file(),
                    1 => self.open_dialog(),
                    2 => self.save(),
                    3 => self.save_as_dialog(),
                    4 => return self.request_exit(),
                    _ => {}
                }
            }
            return false;
        }
        if let Some(input) = decode_control_input(event) {
            let response = self.editor.handle_input(input, self.focused);
            if response.action == Some(TextAreaAction::Changed) {
                self.dirty = true;
            }
        }
        false
    }

    fn new_file(&mut self) {
        self.editor.set_text("");
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
        let bytes = self.editor.text().as_bytes();
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
        self.editor.set_modified(false);
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
                self.editor.set_text(&text);
                self.path = Some(path.to_string());
                self.last_dialog_dir = dialogs::path::parent_directory(path).to_string();
                self.dirty = false;
                self.editor.set_modified(false);
            }
            Err(_) => self.show_error("File is not UTF-8 text"),
        }
    }

    fn open_dialog(&mut self) {
        if self.modal.is_some() {
            return;
        }
        let start = self
            .path
            .as_deref()
            .map(dialogs::path::parent_directory)
            .unwrap_or(&self.last_dialog_dir);
        let options = FileDialogOptions::new(start).with_filter(text_file_filter());
        match FileDialog::open_with(options) {
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
        let suggested = self.path.clone().unwrap_or_else(|| {
            dialogs::path::join_path(&self.last_dialog_dir, "UNTITLED.TXT", false)
        });
        let options = FileDialogOptions::new(&suggested)
            .with_filter(text_file_filter())
            .with_default_extension("txt");
        match FileDialog::save_with(options) {
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
            if let Modal::File(dialog) = modal {
                self.last_dialog_dir = dialog.current_directory().to_string();
            }
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

fn text_file_filter() -> FileFilter {
    FileFilter::new(
        "Text documents",
        &[
            "txt", "md", "rs", "toml", "json", "log", "conf", "sh", "c", "h", "cpp",
        ],
    )
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
