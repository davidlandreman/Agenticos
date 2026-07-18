#![no_std]
#![no_main]

//! `FILEMAN.ELF` — a modern standalone ring-3 file manager.

extern crate alloc;

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

use dialogs::{DialogStatus, MessageBox, MessageChoice, Modal, ModalOutcome};
use gui::file_ui::{
    capabilities, component_prefix, draw_clipped, draw_file_icon as shared_draw_file_icon,
    format_modified, format_size, BreadcrumbBar, FileIconKind as EntryKind, FilePlace,
    FileUiColors, IconButton, MountCapabilities as Capabilities, NavIcon, PlaceIcon, PlacesSidebar,
    UiRect,
};
use gui::{Canvas, TextField, Window};

const INITIAL_W: u32 = 920;
const INITIAL_H: u32 = 580;
const TOOLBAR_H: i32 = 48;
const SIDEBAR_W: i32 = 164;
const HEADER_H: i32 = 26;
const STATUS_H: i32 = 24;
const ROW_H: i32 = 28;
const TILE_W: i32 = 112;
const TILE_H: i32 = 84;

const BG: u32 = 0xF6F8FB;
const SURFACE: u32 = 0xFFFFFF;
const TEXT: u32 = 0x20242C;
const MUTED: u32 = 0x687386;
const BORDER: u32 = 0xD9E0EA;
const ACCENT: u32 = 0x2F73DA;
const SELECTION: u32 = 0xDCEBFF;
const FOLDER: u32 = 0xE9B949;
const DANGER: u32 = 0xC83A3A;
const READ_ONLY: u32 = 0xA15B20;

const FILE_COLORS: FileUiColors = FileUiColors {
    background: BG,
    surface: SURFACE,
    text: TEXT,
    muted: MUTED,
    border: BORDER,
    accent: ACCENT,
    selection: SELECTION,
    folder: FOLDER,
    read_only: READ_ONLY,
};

#[derive(Clone)]
struct Entry {
    name: String,
    path: String,
    kind: EntryKind,
    size: u64,
    modified: i64,
    mode: u32,
}

impl Entry {
    fn is_dir(&self) -> bool {
        self.kind == EntryKind::Folder
    }

    fn type_name(&self) -> &'static str {
        self.kind.type_name()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Details,
    Grid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Size,
    Type,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Content,
    Filter,
    Location,
    Name,
}

enum NameAction {
    CreateFolder,
    Rename { old_path: String },
}

struct NameEditor {
    field: TextField,
    action: NameAction,
}

struct Clipboard {
    paths: Vec<String>,
    cut: bool,
}

#[derive(Clone)]
struct Child {
    pid: i32,
    description: String,
}

enum ModalPurpose {
    Dismiss,
    ConfirmDelete(Vec<String>),
}

struct ActiveModal {
    modal: Modal,
    purpose: ModalPurpose,
}

struct ContextMenu {
    x: i32,
    y: i32,
}

struct LastClick {
    path: String,
    tick: u64,
    x: i32,
    y: i32,
}

struct FileManager {
    window: Window,
    current: String,
    home: String,
    entries: Vec<Entry>,
    visible: Vec<usize>,
    selected: BTreeSet<String>,
    anchor: Option<String>,
    back: Vec<String>,
    forward: Vec<String>,
    sort: SortKey,
    descending: bool,
    view: ViewMode,
    scroll: usize,
    filter: TextField,
    location: TextField,
    focus: FocusTarget,
    name_editor: Option<NameEditor>,
    clipboard: Option<Clipboard>,
    children: Vec<Child>,
    envp: Vec<*const u8>,
    modal: Option<ActiveModal>,
    context: Option<ContextMenu>,
    last_click: Option<LastClick>,
    breadcrumbs: BreadcrumbBar,
    places: PlacesSidebar,
    status: String,
    focused: bool,
}

impl FileManager {
    fn new(initial: Option<String>, home: String, envp: Vec<*const u8>) -> Result<Self, i64> {
        let current = initial.unwrap_or_else(|| home.clone());
        let places = PlacesSidebar::new(
            UiRect::new(0, 0, 0, 0),
            vec![
                FilePlace::new("Home", &home, PlaceIcon::Home),
                FilePlace::new("Root", "/", PlaceIcon::Root),
                FilePlace::new("Data", "/data", PlaceIcon::Data),
                FilePlace::new("Host", "/host", PlaceIcon::Host),
            ],
        );
        let mut app = Self {
            window: Window::new(INITIAL_W, INITIAL_H, "File Manager")?,
            current: normalize_path(&current),
            home,
            entries: Vec::new(),
            visible: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            back: Vec::new(),
            forward: Vec::new(),
            sort: SortKey::Name,
            descending: false,
            view: ViewMode::Details,
            scroll: 0,
            filter: TextField::new(0, 0, 180, 28, ""),
            location: TextField::new(0, 0, 300, 28, ""),
            focus: FocusTarget::Content,
            name_editor: None,
            clipboard: None,
            children: Vec::new(),
            envp,
            modal: None,
            context: None,
            last_click: None,
            breadcrumbs: BreadcrumbBar::new(),
            places,
            status: String::new(),
            focused: true,
        };
        if app.reload().is_err() && app.current != "/" {
            app.current = String::from("/");
            app.reload()?;
        }
        Ok(app)
    }

    fn run(&mut self) -> i64 {
        self.render();
        loop {
            self.reap_children();
            let event = match gui::next_event() {
                Ok(event) => event,
                Err(-4) => {
                    self.reap_children();
                    continue;
                }
                Err(error) => return error,
            };
            if event.window == self.window.handle() {
                if self.handle_main(event) {
                    return 0;
                }
            } else if self
                .modal
                .as_ref()
                .map(|active| active.modal.window_handle())
                == Some(event.window)
            {
                self.handle_modal(&event);
            }
        }
    }

    fn reload(&mut self) -> Result<(), i64> {
        let listed = gui::list_dir(&self.current)?;
        let mut entries = Vec::with_capacity(listed.len());
        for item in listed {
            let path = join_path(&self.current, &item.name);
            entries.push(Entry {
                kind: classify(&item.name, item.is_dir, item.mode),
                name: item.name,
                path,
                size: item.size,
                modified: item.modified,
                mode: item.mode,
            });
        }
        self.entries = entries;
        self.sort_entries();
        self.rebuild_visible();
        self.selected
            .retain(|path| self.entries.iter().any(|entry| &entry.path == path));
        self.scroll = self.scroll.min(self.visible.len().saturating_sub(1));
        self.location.set_text(&self.current);
        let folder = if self.current == "/" {
            "/"
        } else {
            basename(&self.current)
        };
        let _ = self.window.set_title(&format!("{folder} - File Manager"));
        self.status = item_count(self.visible.len());
        Ok(())
    }

    fn sort_entries(&mut self) {
        let key = self.sort;
        let descending = self.descending;
        self.entries.sort_by(|a, b| {
            let folder_order = match (a.is_dir(), b.is_dir()) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            };
            if folder_order != Ordering::Equal {
                return folder_order;
            }
            let order = match key {
                SortKey::Name => compare_name(&a.name, &b.name),
                SortKey::Size => a
                    .size
                    .cmp(&b.size)
                    .then_with(|| compare_name(&a.name, &b.name)),
                SortKey::Type => a
                    .type_name()
                    .cmp(b.type_name())
                    .then_with(|| compare_name(&a.name, &b.name)),
                SortKey::Modified => a
                    .modified
                    .cmp(&b.modified)
                    .then_with(|| compare_name(&a.name, &b.name)),
            };
            if descending {
                order.reverse()
            } else {
                order
            }
        });
    }

    fn rebuild_visible(&mut self) {
        let needle = self.filter.text.to_ascii_lowercase();
        self.visible.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            if needle.is_empty() || entry.name.to_ascii_lowercase().contains(&needle) {
                self.visible.push(index);
            }
        }
        self.scroll = self.scroll.min(self.visible.len().saturating_sub(1));
    }

    fn navigate(&mut self, path: &str, record: bool) {
        let target = normalize_path(path);
        if target == self.current {
            return;
        }
        let previous = self.current.clone();
        self.current = target.clone();
        match self.reload() {
            Ok(()) => {
                if record {
                    self.back.push(previous);
                    if self.back.len() > 64 {
                        self.back.remove(0);
                    }
                    self.forward.clear();
                }
                self.selected.clear();
                self.anchor = None;
                self.filter.set_text("");
                self.rebuild_visible();
            }
            Err(error) => {
                self.current = previous;
                let _ = self.reload();
                self.show_error(&format!("Cannot open {target} ({error})"));
            }
        }
    }

    fn go_back(&mut self) {
        let Some(target) = self.back.pop() else {
            return;
        };
        self.forward.push(self.current.clone());
        self.navigate(&target, false);
    }

    fn go_forward(&mut self) {
        let Some(target) = self.forward.pop() else {
            return;
        };
        self.back.push(self.current.clone());
        self.navigate(&target, false);
    }

    fn go_up(&mut self) {
        if let Some(parent) = parent_path(&self.current) {
            self.navigate(&parent, true);
        }
    }

    fn render(&mut self) {
        self.layout_fields();
        self.build_breadcrumbs();
        let width = self.window.canvas().width() as i32;
        let height = self.window.canvas().height() as i32;
        let content_bottom = height - STATUS_H;
        self.places.bounds = UiRect::new(
            0,
            TOOLBAR_H,
            SIDEBAR_W as u32,
            (content_bottom - TOOLBAR_H).max(1) as u32,
        );
        let mut canvas = core::mem::replace(self.window.canvas_mut(), Canvas::new(1, 1));
        canvas.clear(BG);

        draw_toolbar(
            &mut canvas,
            width,
            &self.back,
            &self.forward,
            &self.current,
            capabilities(&self.current).directories,
        );
        if self.focus == FocusTarget::Location {
            self.location.draw(&mut canvas, true);
        } else {
            self.breadcrumbs.draw(&mut canvas, FILE_COLORS);
        }
        if self.filter.text.is_empty() && self.focus != FocusTarget::Filter {
            canvas.fill_rect(
                self.filter.x,
                self.filter.y,
                self.filter.w,
                self.filter.h,
                SURFACE,
            );
            canvas.rect(
                self.filter.x,
                self.filter.y,
                self.filter.w,
                self.filter.h,
                BORDER,
            );
            canvas.draw_text(
                self.filter.x + 8,
                self.filter.y + 10,
                "Filter this folder",
                MUTED,
            );
        } else {
            self.filter
                .draw(&mut canvas, self.focus == FocusTarget::Filter);
        }

        self.places.draw(&mut canvas, &self.current, FILE_COLORS);
        match self.view {
            ViewMode::Details => self.draw_details(&mut canvas, width, content_bottom),
            ViewMode::Grid => self.draw_grid(&mut canvas, width, content_bottom),
        }
        let status_text = self.status_text();
        draw_status(
            &mut canvas,
            width,
            content_bottom,
            &status_text,
            capabilities(&self.current),
            self.view,
        );
        if let Some(editor) = self.name_editor.as_mut() {
            canvas.fill_rect(SIDEBAR_W + 12, TOOLBAR_H + 32, 360, 40, SURFACE);
            canvas.rect(SIDEBAR_W + 12, TOOLBAR_H + 32, 360, 40, ACCENT);
            editor.field.draw(&mut canvas, true);
        }
        if let Some(menu) = &self.context {
            draw_context_menu(&mut canvas, menu.x, menu.y, capabilities(&self.current));
        }
        *self.window.canvas_mut() = canvas;
        let _ = self.window.present();
    }

    fn layout_fields(&mut self) {
        let width = self.window.canvas().width() as i32;
        let search_w = if width >= 760 { 190 } else { 140 };
        self.filter.x = width - search_w - 10;
        self.filter.y = 10;
        self.filter.w = search_w as u32;
        self.filter.h = 28;
        self.location.x = 226;
        self.location.y = 10;
        self.location.w = (self.filter.x - 236).max(120) as u32;
        self.location.h = 28;
        if let Some(editor) = self.name_editor.as_mut() {
            editor.field.x = SIDEBAR_W + 22;
            editor.field.y = TOOLBAR_H + 38;
            editor.field.w = 338;
            editor.field.h = 28;
        }
    }

    fn build_breadcrumbs(&mut self) {
        let right = self.filter.x - 10;
        self.breadcrumbs.rebuild(
            &self.current,
            UiRect::new(226, 10, (right - 226).max(46) as u32, 28),
        );
    }

    fn draw_details(&self, canvas: &mut Canvas, width: i32, bottom: i32) {
        let left = SIDEBAR_W;
        let content_w = width - left;
        canvas.fill_rect(
            left,
            TOOLBAR_H,
            content_w as u32,
            (bottom - TOOLBAR_H) as u32,
            SURFACE,
        );
        canvas.fill_rect(left, TOOLBAR_H, content_w as u32, HEADER_H as u32, BG);
        canvas.horizontal_line(left, TOOLBAR_H + HEADER_H - 1, content_w as u32, BORDER);
        let name_w = (content_w - 300).max(180);
        canvas.draw_text(
            left + 14,
            TOOLBAR_H + 9,
            sort_label("Name", self.sort == SortKey::Name, self.descending).as_str(),
            TEXT,
        );
        canvas.draw_text(
            left + name_w,
            TOOLBAR_H + 9,
            sort_label("Size", self.sort == SortKey::Size, self.descending).as_str(),
            TEXT,
        );
        canvas.draw_text(
            left + name_w + 82,
            TOOLBAR_H + 9,
            sort_label("Type", self.sort == SortKey::Type, self.descending).as_str(),
            TEXT,
        );
        if content_w > 650 {
            canvas.draw_text(left + content_w - 124, TOOLBAR_H + 9, "Modified", TEXT);
        }
        let top = TOOLBAR_H + HEADER_H;
        let rows = ((bottom - top) / ROW_H).max(1) as usize;
        for slot in 0..rows {
            let position = self.scroll + slot;
            let Some(entry_index) = self.visible.get(position) else {
                break;
            };
            let entry = &self.entries[*entry_index];
            let y = top + slot as i32 * ROW_H;
            let selected = self.selected.contains(&entry.path);
            if selected {
                canvas.fill_rect(left + 1, y, (content_w - 2) as u32, ROW_H as u32, SELECTION);
                canvas.rect(left + 1, y, (content_w - 2) as u32, ROW_H as u32, ACCENT);
            } else if slot % 2 == 1 {
                canvas.fill_rect(left + 1, y, (content_w - 2) as u32, ROW_H as u32, 0xFAFBFD);
            }
            draw_entry_icon(canvas, left + 10, y + 6, entry.kind, 16);
            draw_clipped(
                canvas,
                left + 34,
                y + 10,
                &entry.name,
                (name_w - 48).max(24),
                TEXT,
            );
            if !entry.is_dir() {
                canvas.draw_text(left + name_w, y + 10, &format_size(entry.size), MUTED);
            }
            draw_clipped(
                canvas,
                left + name_w + 82,
                y + 10,
                entry.type_name(),
                112,
                MUTED,
            );
            if content_w > 650 {
                let modified = format_modified(entry.modified);
                draw_clipped(
                    canvas,
                    left + content_w - 124,
                    y + 10,
                    &modified,
                    116,
                    MUTED,
                );
            }
        }
    }

    fn draw_grid(&self, canvas: &mut Canvas, width: i32, bottom: i32) {
        let left = SIDEBAR_W;
        let content_w = width - left;
        let content_h = bottom - TOOLBAR_H;
        canvas.fill_rect(left, TOOLBAR_H, content_w as u32, content_h as u32, SURFACE);
        let columns = (content_w / TILE_W).max(1) as usize;
        let rows = (content_h / TILE_H).max(1) as usize;
        for slot in 0..columns * rows {
            let position = self.scroll + slot;
            let Some(entry_index) = self.visible.get(position) else {
                break;
            };
            let entry = &self.entries[*entry_index];
            let col = slot % columns;
            let row = slot / columns;
            let x = left + col as i32 * TILE_W + 6;
            let y = TOOLBAR_H + row as i32 * TILE_H + 6;
            if self.selected.contains(&entry.path) {
                canvas.fill_rect(x, y, (TILE_W - 12) as u32, (TILE_H - 10) as u32, SELECTION);
                canvas.rect(x, y, (TILE_W - 12) as u32, (TILE_H - 10) as u32, ACCENT);
            }
            draw_entry_icon(canvas, x + 35, y + 8, entry.kind, 30);
            draw_clipped(canvas, x + 5, y + 50, &entry.name, TILE_W - 22, TEXT);
            if !entry.is_dir() {
                draw_clipped(
                    canvas,
                    x + 5,
                    y + 64,
                    &format_size(entry.size),
                    TILE_W - 22,
                    MUTED,
                );
            }
        }
    }

    fn status_text(&self) -> String {
        if !self.status.is_empty() && self.status != item_count(self.visible.len()) {
            return self.status.clone();
        }
        if self.selected.is_empty() {
            return item_count(self.visible.len());
        }
        let bytes: u64 = self
            .entries
            .iter()
            .filter(|entry| self.selected.contains(&entry.path) && !entry.is_dir())
            .map(|entry| entry.size)
            .sum();
        format!("{} selected, {}", self.selected.len(), format_size(bytes))
    }

    fn handle_main(&mut self, event: runtime::GuiEvent) -> bool {
        if self.modal.is_some() && matches!(event.kind, gui::GUI_EVENT_KEY | gui::GUI_EVENT_MOUSE) {
            return false;
        }
        match event.kind {
            gui::GUI_EVENT_CLOSE => return true,
            gui::GUI_EVENT_FOCUS_CHANGE => self.focused = event.payload[0] != 0,
            gui::GUI_EVENT_RESIZE => self.window.resize(event.payload[0], event.payload[1]),
            gui::GUI_EVENT_KEY if event.payload[3] != 0 => self.handle_key(event.payload),
            gui::GUI_EVENT_MOUSE => {
                if event.payload[3] == gui::GUI_MOUSE_DOWN
                    || event.payload[3] == gui::GUI_MOUSE_SCROLL
                {
                    self.handle_mouse(event.payload);
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        self.render();
        false
    }

    fn handle_key(&mut self, payload: [u32; 6]) {
        let key = payload[0];
        let character = char::from_u32(payload[1]).unwrap_or('\0');
        let shift = payload[2] & 1 != 0;
        let ctrl = payload[2] & 2 != 0;
        let alt = payload[2] & 4 != 0;
        if self.context.is_some() && key == runtime::KEY_ESCAPE {
            self.context = None;
            return;
        }
        match self.focus {
            FocusTarget::Filter => {
                if key == runtime::KEY_ESCAPE {
                    self.filter.set_text("");
                    self.focus = FocusTarget::Content;
                } else if key == runtime::KEY_ENTER {
                    self.focus = FocusTarget::Content;
                } else {
                    let _ = self.filter.key(key, character);
                }
                self.rebuild_visible();
                return;
            }
            FocusTarget::Location => {
                if key == runtime::KEY_ESCAPE {
                    self.location.set_text(&self.current);
                    self.focus = FocusTarget::Content;
                } else if key == runtime::KEY_ENTER {
                    let target = resolve_location(&self.current, &self.location.text);
                    self.focus = FocusTarget::Content;
                    self.navigate(&target, true);
                } else {
                    let _ = self.location.key(key, character);
                }
                return;
            }
            FocusTarget::Name => {
                if key == runtime::KEY_ESCAPE {
                    self.name_editor = None;
                    self.focus = FocusTarget::Content;
                } else if key == runtime::KEY_ENTER {
                    self.commit_name_editor();
                } else if let Some(editor) = self.name_editor.as_mut() {
                    let _ = editor.field.key(key, character);
                }
                return;
            }
            FocusTarget::Content => {}
        }
        if ctrl {
            match character.to_ascii_lowercase() {
                'l' => {
                    self.location.set_text(&self.current);
                    self.focus = FocusTarget::Location;
                }
                'f' => self.focus = FocusTarget::Filter,
                'c' => self.set_clipboard(false),
                'x' => self.set_clipboard(true),
                'v' => self.paste(),
                'a' => {
                    self.selected = self
                        .visible
                        .iter()
                        .map(|index| self.entries[*index].path.clone())
                        .collect();
                }
                'n' if shift => self.start_create_folder(),
                _ => {}
            }
            return;
        }
        if alt {
            match key {
                runtime::KEY_LEFT => self.go_back(),
                runtime::KEY_RIGHT => self.go_forward(),
                _ => {}
            }
            return;
        }
        match key {
            runtime::KEY_BACKSPACE => self.go_up(),
            runtime::KEY_ENTER => self.activate_selected(),
            runtime::KEY_DELETE => self.request_delete(),
            runtime::KEY_F2 => self.start_rename(),
            runtime::KEY_F5 => {
                if let Err(error) = self.reload() {
                    self.show_error(&format!("Refresh failed ({error})"));
                }
            }
            runtime::KEY_UP => self.move_selection(-1, shift),
            runtime::KEY_DOWN => self.move_selection(1, shift),
            runtime::KEY_HOME => self.select_position(0, shift, false),
            runtime::KEY_END if !self.visible.is_empty() => {
                self.select_position(self.visible.len() - 1, shift, false)
            }
            runtime::KEY_PAGE_UP => self.move_selection(-10, shift),
            runtime::KEY_PAGE_DOWN => self.move_selection(10, shift),
            _ => {}
        }
    }

    fn handle_mouse(&mut self, payload: [u32; 6]) {
        let x = payload[0] as i32;
        let y = payload[1] as i32;
        if payload[3] == gui::GUI_MOUSE_SCROLL {
            let delta = payload[5] as i32;
            let step = match self.view {
                ViewMode::Details => 3,
                ViewMode::Grid => self.grid_columns(),
            };
            if delta < 0 {
                self.scroll = self.scroll.saturating_sub(step);
            } else {
                self.scroll = (self.scroll + step).min(self.visible.len().saturating_sub(1));
            }
            return;
        }
        if let Some(menu) = self.context.take() {
            if let Some(action) = context_hit(menu.x, menu.y, x, y) {
                self.run_context_action(action);
                return;
            }
        }
        if UiRect::new(10, 10, 30, 28).hit(x, y) {
            self.go_back();
            return;
        }
        if UiRect::new(46, 10, 30, 28).hit(x, y) {
            self.go_forward();
            return;
        }
        if UiRect::new(82, 10, 30, 28).hit(x, y) {
            self.go_up();
            return;
        }
        if UiRect::new(118, 10, 30, 28).hit(x, y) {
            let home = self.home.clone();
            self.navigate(&home, true);
            return;
        }
        if UiRect::new(154, 10, 30, 28).hit(x, y) {
            if capabilities(&self.current).directories {
                self.start_create_folder();
            }
            return;
        }
        if UiRect::new(190, 10, 30, 28).hit(x, y) {
            if let Err(error) = self.reload() {
                self.show_error(&format!("Refresh failed ({error})"));
            }
            return;
        }
        if self.filter.hit(x, y) {
            self.filter.click(x);
            self.focus = FocusTarget::Filter;
            return;
        }
        if let Some(target) = self.breadcrumbs.hit(x, y).map(ToString::to_string) {
            self.navigate(&target, true);
            return;
        }
        if y >= 10 && y < 38 && x >= 226 && x < self.filter.x - 10 {
            self.location.set_text(&self.current);
            self.location.click(x);
            self.focus = FocusTarget::Location;
            return;
        }
        if let Some(place) = self.places.hit(x, y).map(ToString::to_string) {
            self.navigate(&place, true);
            return;
        }
        let width = self.window.canvas().width() as i32;
        let height = self.window.canvas().height() as i32;
        if UiRect::new(width - 84, height - STATUS_H + 3, 32, 18).hit(x, y) {
            self.view = ViewMode::Details;
            self.scroll = 0;
            return;
        }
        if UiRect::new(width - 46, height - STATUS_H + 3, 32, 18).hit(x, y) {
            self.view = ViewMode::Grid;
            self.scroll = 0;
            return;
        }
        if self.view == ViewMode::Details && y >= TOOLBAR_H && y < TOOLBAR_H + HEADER_H {
            self.header_click(x);
            return;
        }
        let Some(position) = self.entry_position_at(x, y) else {
            self.selected.clear();
            self.anchor = None;
            self.focus = FocusTarget::Content;
            return;
        };
        let path = self.entries[self.visible[position]].path.clone();
        let modifiers = payload[2] >> 8;
        let ctrl = modifiers & 2 != 0;
        let shift = modifiers & 1 != 0;
        self.select_position(position, shift, ctrl);
        self.focus = FocusTarget::Content;
        // Mouse button state occupies the low bits; keyboard modifiers are
        // shifted into bits 8.. by the GUI event encoder.
        let right = payload[2] & 0b10 != 0;
        if right {
            let menu_x = x.min(width - 146).max(0);
            let menu_y = y.min(height - STATUS_H - 7 * 24 - 2).max(TOOLBAR_H);
            self.context = Some(ContextMenu {
                x: menu_x,
                y: menu_y,
            });
            return;
        }
        let tick = payload[4] as u64 | ((payload[5] as u64) << 32);
        let double = self.last_click.as_ref().map_or(false, |last| {
            last.path == path
                && tick.saturating_sub(last.tick) <= 50
                && (last.x - x).abs() <= 4
                && (last.y - y).abs() <= 4
        });
        self.last_click = Some(LastClick { path, tick, x, y });
        if double {
            self.activate_selected();
            self.last_click = None;
        }
    }

    fn header_click(&mut self, x: i32) {
        let content_w = self.window.canvas().width() as i32 - SIDEBAR_W;
        let name_w = (content_w - 300).max(180);
        let relative = x - SIDEBAR_W;
        let key = if relative < name_w {
            SortKey::Name
        } else if relative < name_w + 82 {
            SortKey::Size
        } else if relative < content_w - 124 {
            SortKey::Type
        } else {
            SortKey::Modified
        };
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = false;
        }
        self.sort_entries();
        self.rebuild_visible();
    }

    fn grid_columns(&self) -> usize {
        ((self.window.canvas().width() as i32 - SIDEBAR_W) / TILE_W).max(1) as usize
    }

    fn entry_position_at(&self, x: i32, y: i32) -> Option<usize> {
        let width = self.window.canvas().width() as i32;
        let height = self.window.canvas().height() as i32;
        if x < SIDEBAR_W || x >= width || y < TOOLBAR_H || y >= height - STATUS_H {
            return None;
        }
        let position = match self.view {
            ViewMode::Details => {
                if y < TOOLBAR_H + HEADER_H {
                    return None;
                }
                self.scroll + ((y - TOOLBAR_H - HEADER_H) / ROW_H) as usize
            }
            ViewMode::Grid => {
                let col = ((x - SIDEBAR_W) / TILE_W) as usize;
                let row = ((y - TOOLBAR_H) / TILE_H) as usize;
                self.scroll + row * self.grid_columns() + col
            }
        };
        (position < self.visible.len()).then_some(position)
    }

    fn select_position(&mut self, position: usize, shift: bool, ctrl: bool) {
        let Some(index) = self.visible.get(position).copied() else {
            return;
        };
        let path = self.entries[index].path.clone();
        if shift {
            let anchor_position = self.anchor.as_ref().and_then(|anchor| {
                self.visible
                    .iter()
                    .position(|entry| self.entries[*entry].path == *anchor)
            });
            if let Some(anchor) = anchor_position {
                if !ctrl {
                    self.selected.clear();
                }
                let start = anchor.min(position);
                let end = anchor.max(position);
                for visible_index in &self.visible[start..=end] {
                    self.selected
                        .insert(self.entries[*visible_index].path.clone());
                }
            } else {
                self.selected.clear();
                self.selected.insert(path.clone());
                self.anchor = Some(path);
            }
        } else if ctrl {
            if !self.selected.remove(&path) {
                self.selected.insert(path.clone());
            }
            self.anchor = Some(path);
        } else {
            self.selected.clear();
            self.selected.insert(path.clone());
            self.anchor = Some(path);
        }
    }

    fn move_selection(&mut self, delta: isize, shift: bool) {
        if self.visible.is_empty() {
            return;
        }
        let current = self.anchor.as_ref().and_then(|anchor| {
            self.visible
                .iter()
                .position(|index| self.entries[*index].path == *anchor)
        });
        let next = match current {
            Some(position) if delta < 0 => position.saturating_sub(delta.unsigned_abs()),
            Some(position) => (position + delta as usize).min(self.visible.len() - 1),
            None => 0,
        };
        self.select_position(next, shift, false);
        self.ensure_position_visible(next);
    }

    fn ensure_position_visible(&mut self, position: usize) {
        let height = self.window.canvas().height() as i32;
        let page = match self.view {
            ViewMode::Details => {
                ((height - TOOLBAR_H - HEADER_H - STATUS_H) / ROW_H).max(1) as usize
            }
            ViewMode::Grid => {
                let rows = ((height - TOOLBAR_H - STATUS_H) / TILE_H).max(1) as usize;
                rows * self.grid_columns()
            }
        };
        if position < self.scroll {
            self.scroll = position;
        } else if position >= self.scroll + page {
            self.scroll = position + 1 - page;
        }
    }

    fn selected_entries(&self) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|entry| self.selected.contains(&entry.path))
            .cloned()
            .collect()
    }

    fn activate_selected(&mut self) {
        let Some(entry) = self.selected_entries().into_iter().next() else {
            return;
        };
        if entry.is_dir() {
            self.navigate(&entry.path, true);
            return;
        }
        match entry.kind {
            EntryKind::Text => {
                self.spawn_program(
                    "/bin/notepad",
                    vec![String::from("notepad"), entry.path.clone()],
                    &entry.name,
                );
            }
            EntryKind::Executable => {
                let cpath = gui::c_path(&entry.path);
                if runtime::access(&cpath, runtime::X_OK) < 0 {
                    self.show_error("The selected ELF is not executable.");
                } else {
                    self.spawn_program(&entry.path, vec![entry.path.clone()], &entry.name);
                }
            }
            _ => self.show_error("No application is registered for this file type."),
        }
    }

    fn spawn_program(&mut self, program: &str, args: Vec<String>, description: &str) {
        let path = gui::c_path(program);
        let arg_bytes: Vec<Vec<u8>> = args.iter().map(|argument| gui::c_path(argument)).collect();
        let mut argv: Vec<*const u8> = arg_bytes.iter().map(|argument| argument.as_ptr()).collect();
        argv.push(core::ptr::null());
        let pid = runtime::fork();
        if pid == 0 {
            let result = runtime::execve(&path, &argv, &self.envp);
            unsafe { runtime::exit(if result < 0 { 126 } else { 0 }) }
        }
        if pid < 0 {
            self.show_error(&format!("Could not launch {description} ({pid})"));
        } else {
            self.children.push(Child {
                pid: pid as i32,
                description: description.to_string(),
            });
            self.status = format!("Opened {description}");
        }
    }

    fn reap_children(&mut self) {
        loop {
            let mut status = 0u32;
            let pid = runtime::wait4(-1, Some(&mut status), runtime::WNOHANG);
            if pid <= 0 {
                break;
            }
            if let Some(position) = self
                .children
                .iter()
                .position(|child| child.pid == pid as i32)
            {
                let child = self.children.remove(position);
                let code = (status >> 8) & 0xff;
                if code == 126 {
                    self.status = format!("Could not launch {}", child.description);
                }
            }
        }
    }

    fn start_create_folder(&mut self) {
        if !capabilities(&self.current).directories {
            self.show_error("This location does not support creating folders.");
            return;
        }
        self.name_editor = Some(NameEditor {
            field: TextField::new(0, 0, 330, 28, "New Folder"),
            action: NameAction::CreateFolder,
        });
        self.focus = FocusTarget::Name;
    }

    fn start_rename(&mut self) {
        if !capabilities(&self.current).rename {
            self.show_error("This location does not support rename.");
            return;
        }
        let selected = self.selected_entries();
        if selected.len() != 1 {
            self.show_error("Select exactly one item to rename.");
            return;
        }
        let entry = &selected[0];
        self.name_editor = Some(NameEditor {
            field: TextField::new(0, 0, 330, 28, &entry.name),
            action: NameAction::Rename {
                old_path: entry.path.clone(),
            },
        });
        self.focus = FocusTarget::Name;
    }

    fn commit_name_editor(&mut self) {
        let Some(editor) = self.name_editor.take() else {
            return;
        };
        self.focus = FocusTarget::Content;
        let name = editor.field.text.trim();
        if !valid_name(name) {
            self.show_error("Names cannot be empty, '.', '..', or contain '/'.");
            return;
        }
        let target = join_path(&self.current, name);
        if path_exists(&target) {
            self.show_error("An item with that name already exists.");
            return;
        }
        let result = match editor.action {
            NameAction::CreateFolder => runtime::mkdir(&gui::c_path(&target), 0o755),
            NameAction::Rename { old_path } => {
                runtime::rename(&gui::c_path(&old_path), &gui::c_path(&target))
            }
        };
        if result < 0 {
            self.show_error(&format!("Operation failed ({result})"));
        } else {
            self.persist_overlay();
            let _ = self.reload();
            self.selected.clear();
            self.selected.insert(target);
        }
    }

    fn request_delete(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        if !capabilities(&self.current).delete_files {
            self.show_error("This location is read-only.");
            return;
        }
        let paths: Vec<String> = self.selected.iter().cloned().collect();
        let body = if paths.len() == 1 {
            format!("Permanently delete {}?", basename(&paths[0]))
        } else {
            format!("Permanently delete {} selected items?", paths.len())
        };
        match MessageBox::confirm("Delete", &body) {
            Ok(dialog) => {
                self.modal = Some(ActiveModal {
                    modal: Modal::Message(dialog),
                    purpose: ModalPurpose::ConfirmDelete(paths),
                })
            }
            Err(error) => self.status = format!("Could not open confirmation ({error})"),
        }
    }

    fn delete_paths(&mut self, paths: Vec<String>) {
        for path in paths {
            let is_dir = self
                .entries
                .iter()
                .any(|entry| entry.path == path && entry.is_dir());
            let result = if is_dir {
                runtime::rmdir(&gui::c_path(&path))
            } else {
                runtime::unlink(&gui::c_path(&path))
            };
            if result < 0 {
                self.show_error(&format!("Could not delete {} ({result})", basename(&path)));
                break;
            }
        }
        self.persist_overlay();
        self.selected.clear();
        let _ = self.reload();
    }

    fn set_clipboard(&mut self, cut: bool) {
        if self.selected.is_empty() {
            return;
        }
        if cut && !capabilities(&self.current).delete_files {
            self.show_error("This location is read-only; copy is still available.");
            return;
        }
        self.clipboard = Some(Clipboard {
            paths: self.selected.iter().cloned().collect(),
            cut,
        });
        self.status = format!(
            "{} item(s) ready to {}",
            self.selected.len(),
            if cut { "move" } else { "copy" }
        );
    }

    fn paste(&mut self) {
        if !capabilities(&self.current).create_files {
            self.show_error("This location is read-only.");
            return;
        }
        let Some(clipboard) = self.clipboard.as_ref() else {
            return;
        };
        let paths = clipboard.paths.clone();
        let cut = clipboard.cut;
        let mut copied = Vec::new();
        for source in paths {
            let source_entry = self
                .entries
                .iter()
                .find(|entry| entry.path == source)
                .cloned()
                .or_else(|| stat_entry(&source));
            let Some(entry) = source_entry else {
                self.show_error(&format!("Source no longer exists: {source}"));
                break;
            };
            if entry.is_dir() {
                self.show_error("Folder copy/move is not supported yet.");
                break;
            }
            let destination = unique_destination(&self.current, &entry.name);
            let mut renamed = false;
            let result = if cut && same_mount(&source, &destination) {
                let rename = runtime::rename(&gui::c_path(&source), &gui::c_path(&destination));
                if rename == 0 {
                    renamed = true;
                    0
                } else if rename == -18 {
                    // `/data` currently maps its unsupported rename to EXDEV;
                    // use the verified copy-then-unlink fallback for files.
                    copy_file(&source, &destination)
                } else {
                    rename
                }
            } else {
                copy_file(&source, &destination)
            };
            if result < 0 {
                self.show_error(&format!("Could not copy {} ({result})", entry.name));
                break;
            }
            if cut && !renamed {
                let removed = runtime::unlink(&gui::c_path(&source));
                if removed < 0 {
                    self.show_error(&format!(
                        "Copied {}, but could not remove source ({removed})",
                        entry.name
                    ));
                    break;
                }
            }
            copied.push(destination);
        }
        if cut {
            self.clipboard = None;
        }
        self.persist_overlay();
        let _ = self.reload();
        self.selected = copied.into_iter().collect();
        self.status = format!("Pasted {} item(s)", self.selected.len());
    }

    fn persist_overlay(&mut self) {
        if capabilities(&self.current).sync_backed {
            let result = runtime::sync();
            if result < 0 {
                self.status = format!("Changed in memory; sync failed ({result})");
            }
        }
    }

    fn show_properties(&mut self) {
        let selected = self.selected_entries();
        if selected.is_empty() {
            return;
        }
        let text = if selected.len() == 1 {
            let entry = &selected[0];
            format!(
                "Name: {}\nPath: {}\nType: {}\nSize: {}\nModified: {}\nMode: {:o}",
                entry.name,
                entry.path,
                entry.type_name(),
                format_size(entry.size),
                entry.modified,
                entry.mode
            )
        } else {
            let size: u64 = selected
                .iter()
                .filter(|entry| !entry.is_dir())
                .map(|entry| entry.size)
                .sum();
            format!(
                "{} items\nCombined file size: {}",
                selected.len(),
                format_size(size)
            )
        };
        self.show_info(&text);
    }

    fn show_error(&mut self, text: &str) {
        if self.modal.is_some() {
            self.status = text.to_string();
            return;
        }
        if let Ok(dialog) = MessageBox::error(text) {
            self.modal = Some(ActiveModal {
                modal: Modal::Message(dialog),
                purpose: ModalPurpose::Dismiss,
            });
        } else {
            self.status = text.to_string();
        }
    }

    fn show_info(&mut self, text: &str) {
        if let Ok(dialog) = MessageBox::info(text) {
            self.modal = Some(ActiveModal {
                modal: Modal::Message(dialog),
                purpose: ModalPurpose::Dismiss,
            });
        }
    }

    fn handle_modal(&mut self, event: &runtime::GuiEvent) {
        let status = match self.modal.as_mut() {
            Some(active) => active.modal.handle_event(event),
            None => return,
        };
        let DialogStatus::Done(outcome) = status else {
            return;
        };
        let active = self.modal.take().unwrap();
        match active.purpose {
            ModalPurpose::ConfirmDelete(paths) => {
                if matches!(outcome, Some(ModalOutcome::Choice(MessageChoice::Yes))) {
                    self.delete_paths(paths);
                }
            }
            ModalPurpose::Dismiss => {}
        }
        self.render();
    }

    fn run_context_action(&mut self, action: usize) {
        match action {
            0 => self.activate_selected(),
            1 => self.set_clipboard(false),
            2 => self.set_clipboard(true),
            3 => self.paste(),
            4 => self.start_rename(),
            5 => self.request_delete(),
            6 => self.show_properties(),
            _ => {}
        }
    }
}

fn draw_toolbar(
    canvas: &mut Canvas,
    width: i32,
    back: &[String],
    forward: &[String],
    current: &str,
    can_create_directories: bool,
) {
    canvas.fill_rect(0, 0, width as u32, TOOLBAR_H as u32, BG);
    canvas.horizontal_line(0, TOOLBAR_H - 1, width as u32, BORDER);
    draw_nav_button(
        canvas,
        UiRect::new(10, 10, 30, 28),
        NavIcon::Back,
        !back.is_empty(),
    );
    draw_nav_button(
        canvas,
        UiRect::new(46, 10, 30, 28),
        NavIcon::Forward,
        !forward.is_empty(),
    );
    draw_nav_button(
        canvas,
        UiRect::new(82, 10, 30, 28),
        NavIcon::Up,
        current != "/",
    );
    draw_nav_button(canvas, UiRect::new(118, 10, 30, 28), NavIcon::Home, true);
    draw_nav_button(
        canvas,
        UiRect::new(154, 10, 30, 28),
        NavIcon::NewFolder,
        can_create_directories,
    );
    draw_nav_button(canvas, UiRect::new(190, 10, 30, 28), NavIcon::Refresh, true);
}

fn draw_nav_button(canvas: &mut Canvas, rect: UiRect, icon: NavIcon, enabled: bool) {
    IconButton::new(rect, icon, enabled).draw(canvas, FILE_COLORS);
}

fn draw_entry_icon(canvas: &mut Canvas, x: i32, y: i32, kind: EntryKind, size: u32) {
    shared_draw_file_icon(canvas, x, y, kind, size, FILE_COLORS);
}

fn draw_status(
    canvas: &mut Canvas,
    width: i32,
    y: i32,
    status: &str,
    caps: Capabilities,
    view: ViewMode,
) {
    canvas.fill_rect(0, y, width as u32, STATUS_H as u32, BG);
    canvas.horizontal_line(0, y, width as u32, BORDER);
    draw_clipped(canvas, 10, y + 9, status, width - 300, TEXT);
    let capability = if caps.read_only {
        "Read-only"
    } else if caps.sync_backed {
        "Sync-backed"
    } else {
        "Persistent"
    };
    canvas.draw_text(
        width - 220,
        y + 9,
        capability,
        if caps.read_only { READ_ONLY } else { MUTED },
    );
    let list = UiRect::new(width - 84, y + 3, 32, 18);
    let grid = UiRect::new(width - 46, y + 3, 32, 18);
    canvas.fill_rect(
        list.x,
        list.y,
        list.w,
        list.h,
        if view == ViewMode::Details {
            SELECTION
        } else {
            SURFACE
        },
    );
    canvas.rect(
        list.x,
        list.y,
        list.w,
        list.h,
        if view == ViewMode::Details {
            ACCENT
        } else {
            BORDER
        },
    );
    for row in 0..3 {
        canvas.horizontal_line(list.x + 8, list.y + 5 + row * 4, 16, ACCENT);
    }
    canvas.fill_rect(
        grid.x,
        grid.y,
        grid.w,
        grid.h,
        if view == ViewMode::Grid {
            SELECTION
        } else {
            SURFACE
        },
    );
    canvas.rect(
        grid.x,
        grid.y,
        grid.w,
        grid.h,
        if view == ViewMode::Grid {
            ACCENT
        } else {
            BORDER
        },
    );
    for row in 0..2 {
        for col in 0..3 {
            canvas.rect(grid.x + 7 + col * 7, grid.y + 4 + row * 7, 5, 5, ACCENT);
        }
    }
}

fn draw_context_menu(canvas: &mut Canvas, x: i32, y: i32, caps: Capabilities) {
    const ITEMS: [&str; 7] = [
        "Open",
        "Copy",
        "Cut",
        "Paste",
        "Rename",
        "Delete",
        "Properties",
    ];
    let width = 144u32;
    let height = 7 * 24;
    canvas.fill_rect(x, y, width, height, SURFACE);
    canvas.rect(x, y, width, height, BORDER);
    for (index, item) in ITEMS.iter().enumerate() {
        let enabled = match index {
            2 | 5 => caps.delete_files,
            3 => caps.create_files,
            4 => caps.rename,
            _ => true,
        };
        let color = if !enabled {
            BORDER
        } else if index == 5 {
            DANGER
        } else {
            TEXT
        };
        canvas.draw_text(x + 10, y + index as i32 * 24 + 9, item, color);
    }
}

fn context_hit(menu_x: i32, menu_y: i32, x: i32, y: i32) -> Option<usize> {
    if x < menu_x || x >= menu_x + 144 || y < menu_y || y >= menu_y + 7 * 24 {
        return None;
    }
    Some(((y - menu_y) / 24) as usize)
}

fn sort_label(label: &str, active: bool, descending: bool) -> String {
    if !active {
        return label.to_string();
    }
    format!("{} {}", label, if descending { "v" } else { "^" })
}

fn classify(name: &str, is_dir: bool, mode: u32) -> EntryKind {
    if is_dir {
        return EntryKind::Folder;
    }
    let extension = extension(name);
    if extension == "elf" || mode & 0o111 != 0 {
        return EntryKind::Executable;
    }
    if matches!(
        extension.as_str(),
        "txt" | "md" | "rs" | "toml" | "json" | "log" | "conf" | "sh" | "c" | "h" | "cpp"
    ) {
        EntryKind::Text
    } else if matches!(extension.as_str(), "bmp" | "png" | "jpg" | "jpeg" | "gif") {
        EntryKind::Image
    } else if matches!(extension.as_str(), "zip" | "tar" | "gz" | "bz2") {
        EntryKind::Archive
    } else {
        EntryKind::File
    }
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn compare_name(a: &str, b: &str) -> Ordering {
    a.to_ascii_lowercase()
        .cmp(&b.to_ascii_lowercase())
        .then_with(|| a.cmp(b))
}

fn same_mount(a: &str, b: &str) -> bool {
    mount_key(a) == mount_key(b)
}

fn mount_key(path: &str) -> u8 {
    if component_prefix(path, "/host") {
        1
    } else if component_prefix(path, "/data") {
        2
    } else if component_prefix(path, "/bin") {
        3
    } else {
        0
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

fn normalize_path(input: &str) -> String {
    let absolute = input.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                let _ = parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut result = if absolute {
        String::from("/")
    } else {
        String::from("/")
    };
    result.push_str(&parts.join("/"));
    if result.len() > 1 && result.ends_with('/') {
        result.pop();
    }
    result
}

fn resolve_location(current: &str, input: &str) -> String {
    if input.starts_with('/') {
        normalize_path(input)
    } else {
        normalize_path(&join_path(current, input))
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

fn parent_path(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => Some(String::from("/")),
        Some(index) => Some(trimmed[..index].to_string()),
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("/")
}

fn path_exists(path: &str) -> bool {
    runtime::access(&gui::c_path(path), runtime::F_OK) == 0
}

fn stat_entry(path: &str) -> Option<Entry> {
    let mut stat = runtime::LinuxStat::default();
    if runtime::newfstatat(runtime::AT_FDCWD, &gui::c_path(path), &mut stat, 0) < 0 {
        return None;
    }
    let name = basename(path).to_string();
    let is_dir = stat.st_mode & 0o170000 == 0o040000;
    Some(Entry {
        kind: classify(&name, is_dir, stat.st_mode),
        name,
        path: path.to_string(),
        size: stat.st_size.max(0) as u64,
        modified: stat.st_mtime,
        mode: stat.st_mode,
    })
}

fn unique_destination(directory: &str, name: &str) -> String {
    let direct = join_path(directory, name);
    if !path_exists(&direct) {
        return direct;
    }
    let (stem, ext) = name.rsplit_once('.').map_or((name, ""), |parts| parts);
    for index in 1..100 {
        let candidate = if component_prefix(directory, "/data") {
            let short_stem: String = stem.chars().take(5).collect();
            let short_ext: String = ext.chars().take(3).collect();
            if short_ext.is_empty() {
                format!("{}~{}", short_stem, index)
            } else {
                format!("{}~{}.{}", short_stem, index, short_ext)
            }
        } else if ext.is_empty() {
            format!("{} copy {}", stem, index)
        } else {
            format!("{} copy {}.{}", stem, index, ext)
        };
        let target = join_path(directory, &candidate);
        if !path_exists(&target) {
            return target;
        }
    }
    join_path(directory, "COPY.TMP")
}

fn copy_file(source: &str, destination: &str) -> i64 {
    let source_fd = runtime::openat(
        runtime::AT_FDCWD,
        &gui::c_path(source),
        runtime::O_RDONLY,
        0,
    );
    if source_fd < 0 {
        return source_fd;
    }
    let destination_fd = runtime::openat(
        runtime::AT_FDCWD,
        &gui::c_path(destination),
        runtime::O_WRONLY | runtime::O_CREAT | runtime::O_TRUNC,
        0o644,
    );
    if destination_fd < 0 {
        let _ = runtime::close(source_fd as i32);
        return destination_fd;
    }
    let mut buffer = vec![0u8; 32 * 1024];
    let mut result = 0i64;
    loop {
        let count = runtime::read(source_fd as i32, &mut buffer);
        if count < 0 {
            result = count;
            break;
        }
        if count == 0 {
            break;
        }
        let mut written = 0usize;
        while written < count as usize {
            let next = runtime::write(destination_fd as i32, &buffer[written..count as usize]);
            if next <= 0 {
                result = if next < 0 { next } else { -5 };
                break;
            }
            written += next as usize;
        }
        if result < 0 {
            break;
        }
    }
    let _ = runtime::close(source_fd as i32);
    let _ = runtime::close(destination_fd as i32);
    if result < 0 {
        let _ = runtime::unlink(&gui::c_path(destination));
    }
    result
}

fn item_count(count: usize) -> String {
    if count == 1 {
        String::from("1 item")
    } else {
        format!("{} items", count)
    }
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

unsafe fn env_value(envp: &[*const u8], name: &str) -> Option<String> {
    for pointer in envp {
        let value = c_string(*pointer)?;
        if let Some(rest) = value
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(rest.to_string());
        }
    }
    None
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {}",
        "ud2",
        sym fileman_main,
    );
}

unsafe extern "C" fn fileman_main(stack: *const u64) -> ! {
    let startup = runtime::startup_from_stack(stack);
    let mut home = env_value(startup.envp, "HOME").unwrap_or_else(|| String::from("/"));
    if !path_exists(&home) {
        home = String::from("/");
    }
    let initial = startup.argv.get(1).and_then(|pointer| c_string(*pointer));
    let mut envp = startup.envp.to_vec();
    envp.push(core::ptr::null());
    let code = match FileManager::new(initial, home, envp) {
        Ok(mut app) => app.run(),
        Err(error) => error,
    };
    runtime::exit(if code == 0 { 0 } else { 1 })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}
