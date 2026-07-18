//! Modern modal file Open / Save dialog.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

use gui::file_ui::{
    capabilities, classify_file, draw_clipped, draw_file_icon, file_extension, format_modified,
    format_size, BreadcrumbBar, BrowserScrollbar, FileIconKind, FilePlace, FileUiColors,
    IconButton, NavIcon, PlaceIcon, PlacesSidebar, UiRect,
};
use gui::{
    decode_control_input, theme, Button, ButtonAction, Canvas, ControlInput, PointerKind,
    TextField, Window,
};

use crate::DialogStatus;

/// Open vs Save behavior for a [`FileDialog`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Open,
    Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileView {
    Details,
    Grid,
}

/// A labeled group of allowed filename extensions. An empty extension list
/// means all files.
#[derive(Clone)]
pub struct FileFilter {
    pub label: String,
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(label: &str, extensions: &[&str]) -> Self {
        Self {
            label: label.to_string(),
            extensions: extensions
                .iter()
                .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
                .collect(),
        }
    }

    pub fn all_files() -> Self {
        Self::new("All files", &[])
    }

    fn matches(&self, name: &str) -> bool {
        self.extensions.is_empty()
            || self
                .extensions
                .iter()
                .any(|extension| file_extension(name) == *extension)
    }
}

/// Owned configuration for a common file chooser.
pub struct FileDialogOptions {
    pub initial_path: String,
    pub title: Option<String>,
    pub commit_label: Option<String>,
    pub filters: Vec<FileFilter>,
    pub default_filter: usize,
    pub default_extension: Option<String>,
    pub places: Vec<FilePlace>,
    pub allow_all_files: bool,
    pub initial_view: FileView,
}

impl FileDialogOptions {
    pub fn new(initial_path: &str) -> Self {
        Self {
            initial_path: initial_path.to_string(),
            title: None,
            commit_label: None,
            filters: Vec::new(),
            default_filter: 0,
            default_extension: None,
            places: Vec::new(),
            allow_all_files: true,
            initial_view: FileView::Details,
        }
    }

    pub fn with_filter(mut self, filter: FileFilter) -> Self {
        self.filters.push(filter);
        self
    }

    pub fn with_default_extension(mut self, extension: &str) -> Self {
        self.default_extension = Some(extension.trim_start_matches('.').to_ascii_lowercase());
        self
    }

    pub fn with_place(mut self, place: FilePlace) -> Self {
        self.places.push(place);
        self
    }
}

const INITIAL_W: u32 = 760;
const INITIAL_H: u32 = 500;
const MIN_W: i32 = 620;
const MIN_H: i32 = 390;
const TOOLBAR_H: i32 = 48;
const SIDEBAR_W: i32 = 152;
const FOOTER_H: i32 = 96;
const HEADER_H: i32 = 26;
const ROW_H: i32 = 28;
const TILE_W: i32 = 112;
const TILE_H: i32 = 84;

const MODERN: FileUiColors = FileUiColors {
    background: 0xF6F8FB,
    surface: 0xFFFFFF,
    text: 0x20242C,
    muted: 0x687386,
    border: 0xD9E0EA,
    accent: 0x2F73DA,
    selection: 0xDCEBFF,
    folder: 0xE9B949,
    read_only: 0xA15B20,
};

#[derive(Clone)]
struct Entry {
    name: String,
    path: String,
    kind: FileIconKind,
    size: u64,
    modified: i64,
}

impl Entry {
    fn is_dir(&self) -> bool {
        self.kind == FileIconKind::Folder
    }
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
    Places,
    Content,
    Filter,
    Location,
    Name,
    FileType,
    Confirm,
    Cancel,
    FolderName,
    Overwrite,
}

struct LastClick {
    path: String,
    tick: u64,
    x: i32,
    y: i32,
}

/// A retained-mode, single-file common chooser.
pub struct FileDialog {
    window: Window,
    mode: FileMode,
    current_dir: String,
    entries: Vec<Entry>,
    visible: Vec<usize>,
    selected: Option<String>,
    back: Vec<String>,
    forward: Vec<String>,
    sort: SortKey,
    descending: bool,
    view: FileView,
    scroll: usize,
    browser_scroll: BrowserScrollbar,
    focus: FocusTarget,
    location: TextField,
    filter: TextField,
    name: TextField,
    confirm: Button,
    cancel: Button,
    breadcrumbs: BreadcrumbBar,
    sidebar: PlacesSidebar,
    place_focus: usize,
    filters: Vec<FileFilter>,
    active_filter: usize,
    default_extension: Option<String>,
    type_open: bool,
    last_click: Option<LastClick>,
    status: String,
    folder_editor: Option<TextField>,
    overwrite: Option<String>,
}

impl FileDialog {
    pub fn open(start_dir: &str) -> Result<Self, i64> {
        Self::open_with(FileDialogOptions::new(start_dir))
    }

    pub fn save(suggested_path: &str) -> Result<Self, i64> {
        Self::save_with(FileDialogOptions::new(suggested_path))
    }

    pub fn open_with(options: FileDialogOptions) -> Result<Self, i64> {
        Self::new(FileMode::Open, options)
    }

    pub fn save_with(options: FileDialogOptions) -> Result<Self, i64> {
        Self::new(FileMode::Save, options)
    }

    fn new(mode: FileMode, mut options: FileDialogOptions) -> Result<Self, i64> {
        let title = options.title.take().unwrap_or_else(|| match mode {
            FileMode::Open => "Open File".to_string(),
            FileMode::Save => "Save File As".to_string(),
        });
        let commit_label = options.commit_label.take().unwrap_or_else(|| match mode {
            FileMode::Open => "Open".to_string(),
            FileMode::Save => "Save".to_string(),
        });
        let initial = normalize_path(&options.initial_path);
        let (current_dir, suggested_name) = initial_directory_and_name(mode, &options.initial_path);

        if options.filters.is_empty()
            || (options.allow_all_files
                && !options
                    .filters
                    .iter()
                    .any(|filter| filter.extensions.is_empty()))
        {
            options.filters.push(FileFilter::all_files());
        }
        let active_filter = options
            .default_filter
            .min(options.filters.len().saturating_sub(1));
        let places = build_places(&current_dir, options.places);
        let window = Window::new(INITIAL_W, INITIAL_H, &title)?;
        let mut dialog = Self {
            window,
            mode,
            current_dir: current_dir.clone(),
            entries: Vec::new(),
            visible: Vec::new(),
            selected: None,
            back: Vec::new(),
            forward: Vec::new(),
            sort: SortKey::Name,
            descending: false,
            view: options.initial_view,
            scroll: 0,
            browser_scroll: BrowserScrollbar::new(),
            focus: FocusTarget::Content,
            location: TextField::new(0, 0, 0, 0, &current_dir),
            filter: TextField::new(0, 0, 0, 0, ""),
            name: TextField::new(0, 0, 0, 0, &suggested_name),
            confirm: Button::new(&commit_label, 0, 0, 88, 26),
            cancel: Button::new("Cancel", 0, 0, 88, 26),
            breadcrumbs: BreadcrumbBar::new(),
            sidebar: PlacesSidebar::new(UiRect::new(0, 0, 0, 0), places),
            place_focus: 0,
            filters: options.filters,
            active_filter,
            default_extension: options.default_extension,
            type_open: false,
            last_click: None,
            status: String::new(),
            folder_editor: None,
            overwrite: None,
        };
        // `normalize_path` above intentionally validates the escape-hatch path
        // even when the constructor eventually uses its parent.
        let _ = initial;
        if let Err(error) = dialog.reload() {
            if dialog.current_dir == "/" {
                return Err(error);
            }
            dialog.current_dir = "/".to_string();
            dialog.location.set_text("/");
            dialog.reload()?;
            dialog.status = format!("Start location unavailable ({error}); showing Root");
        }
        dialog.relayout();
        dialog.render();
        Ok(dialog)
    }

    pub fn window_handle(&self) -> u32 {
        self.window.handle()
    }

    /// Current directory, useful to hosts that remember chooser location.
    pub fn current_directory(&self) -> &str {
        &self.current_dir
    }

    pub fn refresh_theme(&mut self) {
        self.render();
    }

    fn colors(&self) -> FileUiColors {
        if theme::current().is_modern() {
            MODERN
        } else {
            let palette = theme::palette();
            FileUiColors {
                background: palette.content_bg,
                surface: palette.field_bg,
                text: palette.text,
                muted: palette.disabled_text,
                border: palette.border,
                accent: palette.selection_bg,
                selection: palette.selection_bg,
                folder: 0xE9B949,
                read_only: 0x800000,
            }
        }
    }

    fn relayout(&mut self) {
        let width = (self.window.canvas().width() as i32).max(MIN_W);
        let height = (self.window.canvas().height() as i32).max(MIN_H);
        let content_bottom = height - FOOTER_H;
        let filter_w = if width >= 700 { 184 } else { 146 };
        self.filter.x = width - filter_w - 10;
        self.filter.y = 10;
        self.filter.w = filter_w as u32;
        self.filter.h = 28;
        self.location.x = 198;
        self.location.y = 10;
        self.location.w = (self.filter.x - 208).max(116) as u32;
        self.location.h = 28;
        self.breadcrumbs
            .rebuild(&self.current_dir, UiRect::new(198, 10, self.location.w, 28));
        self.sidebar.bounds = UiRect::new(
            0,
            TOOLBAR_H,
            SIDEBAR_W as u32,
            (content_bottom - TOOLBAR_H).max(1) as u32,
        );

        let type_w = 178;
        self.name.x = 86;
        self.name.y = content_bottom + 28;
        self.name.w = (width - 86 - type_w - 28).max(180) as u32;
        self.name.h = 24;
        let button_y = content_bottom + 60;
        self.cancel.x = width - 12 - self.cancel.w as i32;
        self.cancel.y = button_y;
        self.confirm.x = self.cancel.x - 10 - self.confirm.w as i32;
        self.confirm.y = button_y;
        if let Some(editor) = self.folder_editor.as_mut() {
            editor.x = SIDEBAR_W + 24;
            editor.y = TOOLBAR_H + 42;
            editor.w = 320;
            editor.h = 26;
        }
    }

    fn read_entries(&self, directory: &str) -> Result<Vec<Entry>, i64> {
        let listed = gui::list_dir(directory)?;
        Ok(listed
            .into_iter()
            .map(|item| Entry {
                path: join_path(directory, &item.name),
                kind: classify_file(&item.name, item.is_dir, item.mode),
                name: item.name,
                size: item.size,
                modified: item.modified,
            })
            .collect())
    }

    fn reload(&mut self) -> Result<(), i64> {
        let entries = self.read_entries(&self.current_dir)?;
        self.entries = entries;
        self.sort_entries();
        self.rebuild_visible();
        if self
            .selected
            .as_ref()
            .is_some_and(|path| !self.entries.iter().any(|entry| &entry.path == path))
        {
            self.selected = None;
        }
        self.location.set_text(&self.current_dir);
        self.status = item_count(self.visible.len());
        Ok(())
    }

    fn sort_entries(&mut self) {
        let key = self.sort;
        let descending = self.descending;
        self.entries.sort_by(|a, b| {
            let folders = match (a.is_dir(), b.is_dir()) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            };
            if folders != Ordering::Equal {
                return folders;
            }
            let order = match key {
                SortKey::Name => compare_name(&a.name, &b.name),
                SortKey::Size => a
                    .size
                    .cmp(&b.size)
                    .then_with(|| compare_name(&a.name, &b.name)),
                SortKey::Type => a
                    .kind
                    .type_name()
                    .cmp(b.kind.type_name())
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

    fn active_filter(&self) -> &FileFilter {
        &self.filters[self.active_filter.min(self.filters.len() - 1)]
    }

    fn rebuild_visible(&mut self) {
        let needle = self.filter.text.to_ascii_lowercase();
        let filter = self.active_filter().clone();
        self.visible.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            let text_matches =
                needle.is_empty() || entry.name.to_ascii_lowercase().contains(&needle);
            let type_matches = entry.is_dir() || filter.matches(&entry.name);
            if text_matches && type_matches {
                self.visible.push(index);
            }
        }
        self.scroll = self.scroll.min(self.visible.len().saturating_sub(1));
        if self.selected.as_ref().is_some_and(|path| {
            !self
                .visible
                .iter()
                .any(|index| self.entries[*index].path == *path)
        }) {
            self.selected = None;
        }
        self.status = item_count(self.visible.len());
    }

    fn navigate(&mut self, path: &str, record: bool) {
        let target = normalize_path(path);
        if target == self.current_dir {
            return;
        }
        match self.read_entries(&target) {
            Ok(entries) => {
                let previous = core::mem::replace(&mut self.current_dir, target);
                if record {
                    self.back.push(previous);
                    if self.back.len() > 64 {
                        self.back.remove(0);
                    }
                    self.forward.clear();
                }
                self.entries = entries;
                self.selected = None;
                self.scroll = 0;
                self.filter.set_text("");
                self.sort_entries();
                self.rebuild_visible();
                self.location.set_text(&self.current_dir);
                self.breadcrumbs
                    .rebuild(&self.current_dir, UiRect::new(198, 10, self.location.w, 28));
            }
            Err(error) => self.status = format!("Cannot open {target} ({error})"),
        }
    }

    fn go_back(&mut self) {
        let Some(target) = self.back.pop() else {
            return;
        };
        self.forward.push(self.current_dir.clone());
        self.navigate(&target, false);
    }

    fn go_forward(&mut self) {
        let Some(target) = self.forward.pop() else {
            return;
        };
        self.back.push(self.current_dir.clone());
        self.navigate(&target, false);
    }

    fn go_up(&mut self) {
        if let Some(parent) = parent_path(&self.current_dir) {
            self.navigate(&parent, true);
        }
    }

    fn render(&mut self) {
        self.relayout();
        let width = self.window.canvas().width() as i32;
        let height = self.window.canvas().height() as i32;
        let safe_width = width.max(MIN_W);
        let safe_height = height.max(MIN_H);
        let content_bottom = safe_height - FOOTER_H;
        self.sync_browser_scroll();
        let colors = self.colors();
        let mut canvas = core::mem::replace(self.window.canvas_mut(), Canvas::new(1, 1));
        canvas.clear(colors.background);

        canvas.fill_rect(0, 0, safe_width as u32, TOOLBAR_H as u32, colors.background);
        canvas.horizontal_line(0, TOOLBAR_H - 1, safe_width as u32, colors.border);
        self.draw_toolbar(&mut canvas, colors);
        if self.focus == FocusTarget::Location {
            self.location.draw(&mut canvas, true);
        } else {
            self.breadcrumbs.draw(&mut canvas, colors);
        }
        if self.filter.text.is_empty() && self.focus != FocusTarget::Filter {
            canvas.fill_rect(
                self.filter.x,
                self.filter.y,
                self.filter.w,
                self.filter.h,
                colors.surface,
            );
            canvas.rect(
                self.filter.x,
                self.filter.y,
                self.filter.w,
                self.filter.h,
                colors.border,
            );
            draw_clipped(
                &mut canvas,
                self.filter.x + 7,
                self.filter.y + 9,
                "Filter this folder",
                self.filter.w as i32 - 14,
                colors.muted,
            );
        } else {
            self.filter
                .draw(&mut canvas, self.focus == FocusTarget::Filter);
        }

        self.sidebar.draw(&mut canvas, &self.current_dir, colors);
        if self.focus == FocusTarget::Places && self.place_focus < self.sidebar.places.len() {
            let y =
                self.sidebar.bounds.y + 34 + self.place_focus as i32 * PlacesSidebar::ROW_HEIGHT;
            canvas.rect(
                self.sidebar.bounds.x + 8,
                y,
                self.sidebar.bounds.w.saturating_sub(17),
                30,
                colors.accent,
            );
        }
        match self.view {
            FileView::Details => self.draw_details(&mut canvas, safe_width, content_bottom, colors),
            FileView::Grid => self.draw_grid(&mut canvas, safe_width, content_bottom, colors),
        }
        self.browser_scroll.draw(&mut canvas);
        self.draw_footer(&mut canvas, safe_width, content_bottom, colors);
        if let Some(editor) = self.folder_editor.as_mut() {
            canvas.fill_rect(SIDEBAR_W + 12, TOOLBAR_H + 30, 350, 52, colors.surface);
            canvas.rect(SIDEBAR_W + 12, TOOLBAR_H + 30, 350, 52, colors.accent);
            canvas.draw_text(SIDEBAR_W + 22, TOOLBAR_H + 36, "New folder:", colors.text);
            editor.draw(&mut canvas, true);
        }
        if self.overwrite.is_some() {
            self.draw_overwrite(&mut canvas, safe_width, safe_height, colors);
        }

        *self.window.canvas_mut() = canvas;
        let _ = self.window.present();
    }

    fn draw_toolbar(&self, canvas: &mut Canvas, colors: FileUiColors) {
        let buttons = [
            IconButton::new(
                UiRect::new(10, 10, 30, 28),
                NavIcon::Back,
                !self.back.is_empty(),
            ),
            IconButton::new(
                UiRect::new(46, 10, 30, 28),
                NavIcon::Forward,
                !self.forward.is_empty(),
            ),
            IconButton::new(
                UiRect::new(82, 10, 30, 28),
                NavIcon::Up,
                self.current_dir != "/",
            ),
            IconButton::new(
                UiRect::new(118, 10, 30, 28),
                NavIcon::NewFolder,
                capabilities(&self.current_dir).directories,
            ),
            IconButton::new(UiRect::new(154, 10, 30, 28), NavIcon::Refresh, true),
        ];
        for button in buttons {
            button.draw(canvas, colors);
        }
    }

    fn draw_details(&self, canvas: &mut Canvas, width: i32, bottom: i32, colors: FileUiColors) {
        let left = SIDEBAR_W;
        let content_w = width - left;
        canvas.fill_rect(
            left,
            TOOLBAR_H,
            content_w as u32,
            (bottom - TOOLBAR_H).max(1) as u32,
            colors.surface,
        );
        canvas.fill_rect(
            left,
            TOOLBAR_H,
            content_w as u32,
            HEADER_H as u32,
            colors.background,
        );
        canvas.horizontal_line(
            left,
            TOOLBAR_H + HEADER_H - 1,
            content_w as u32,
            colors.border,
        );
        let name_w = (content_w - 294).max(190);
        canvas.draw_text(
            left + 14,
            TOOLBAR_H + 9,
            &sort_label("Name", self.sort == SortKey::Name, self.descending),
            colors.text,
        );
        canvas.draw_text(
            left + name_w,
            TOOLBAR_H + 9,
            &sort_label("Size", self.sort == SortKey::Size, self.descending),
            colors.text,
        );
        canvas.draw_text(
            left + name_w + 78,
            TOOLBAR_H + 9,
            &sort_label("Type", self.sort == SortKey::Type, self.descending),
            colors.text,
        );
        if content_w > 560 {
            canvas.draw_text(
                left + content_w - 116,
                TOOLBAR_H + 9,
                &sort_label("Modified", self.sort == SortKey::Modified, self.descending),
                colors.text,
            );
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
            if self.selected.as_deref() == Some(entry.path.as_str()) {
                canvas.fill_rect(
                    left + 1,
                    y,
                    (content_w - 2) as u32,
                    ROW_H as u32,
                    colors.selection,
                );
                canvas.rect(
                    left + 1,
                    y,
                    (content_w - 2) as u32,
                    ROW_H as u32,
                    colors.accent,
                );
            } else if slot % 2 == 1 && theme::current().is_modern() {
                canvas.fill_rect(left + 1, y, (content_w - 2) as u32, ROW_H as u32, 0xFAFBFD);
            }
            draw_file_icon(canvas, left + 10, y + 6, entry.kind, 16, colors);
            draw_clipped(
                canvas,
                left + 34,
                y + 9,
                &entry.name,
                (name_w - 46).max(24),
                colors.text,
            );
            if !entry.is_dir() {
                canvas.draw_text(left + name_w, y + 9, &format_size(entry.size), colors.muted);
            }
            draw_clipped(
                canvas,
                left + name_w + 78,
                y + 9,
                entry.kind.type_name(),
                104,
                colors.muted,
            );
            if content_w > 560 {
                let modified = format_modified(entry.modified);
                draw_clipped(
                    canvas,
                    left + content_w - 116,
                    y + 9,
                    &modified,
                    108,
                    colors.muted,
                );
            }
        }
        if self.focus == FocusTarget::Content {
            canvas.rect(
                left + 1,
                TOOLBAR_H + HEADER_H,
                (content_w - 2) as u32,
                (bottom - TOOLBAR_H - HEADER_H).max(1) as u32,
                colors.accent,
            );
        }
    }

    fn draw_grid(&self, canvas: &mut Canvas, width: i32, bottom: i32, colors: FileUiColors) {
        let left = SIDEBAR_W;
        let content_w = width - left;
        let content_h = bottom - TOOLBAR_H;
        canvas.fill_rect(
            left,
            TOOLBAR_H,
            content_w as u32,
            content_h.max(1) as u32,
            colors.surface,
        );
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
            if self.selected.as_deref() == Some(entry.path.as_str()) {
                canvas.fill_rect(
                    x,
                    y,
                    (TILE_W - 12) as u32,
                    (TILE_H - 10) as u32,
                    colors.selection,
                );
                canvas.rect(
                    x,
                    y,
                    (TILE_W - 12) as u32,
                    (TILE_H - 10) as u32,
                    colors.accent,
                );
            }
            draw_file_icon(canvas, x + 35, y + 8, entry.kind, 30, colors);
            draw_clipped(canvas, x + 5, y + 50, &entry.name, TILE_W - 22, colors.text);
            if !entry.is_dir() {
                draw_clipped(
                    canvas,
                    x + 5,
                    y + 64,
                    &format_size(entry.size),
                    TILE_W - 22,
                    colors.muted,
                );
            }
        }
        if self.focus == FocusTarget::Content {
            canvas.rect(
                left + 1,
                TOOLBAR_H + 1,
                (content_w - 2) as u32,
                (content_h - 2).max(1) as u32,
                colors.accent,
            );
        }
    }

    fn draw_footer(
        &mut self,
        canvas: &mut Canvas,
        width: i32,
        content_bottom: i32,
        colors: FileUiColors,
    ) {
        canvas.fill_rect(
            0,
            content_bottom,
            width as u32,
            FOOTER_H as u32,
            colors.background,
        );
        canvas.horizontal_line(0, content_bottom, width as u32, colors.border);
        draw_clipped(
            canvas,
            10,
            content_bottom + 8,
            &self.status_line(),
            width - 250,
            colors.text,
        );
        let caps = capabilities(&self.current_dir);
        canvas.draw_text(
            width - 218,
            content_bottom + 8,
            caps.label(),
            if caps.read_only {
                colors.read_only
            } else {
                colors.muted
            },
        );
        draw_view_toggle(canvas, width, content_bottom + 2, self.view, colors);

        canvas.draw_text(10, self.name.y + 7, "File name:", colors.text);
        self.name.draw(canvas, self.focus == FocusTarget::Name);
        let type_rect = self.type_rect();
        canvas.fill_rect(
            type_rect.x,
            type_rect.y,
            type_rect.w,
            type_rect.h,
            colors.surface,
        );
        canvas.rect(
            type_rect.x,
            type_rect.y,
            type_rect.w,
            type_rect.h,
            if self.focus == FocusTarget::FileType {
                colors.accent
            } else {
                colors.border
            },
        );
        draw_clipped(
            canvas,
            type_rect.x + 7,
            type_rect.y + 7,
            &self.active_filter().label,
            type_rect.w as i32 - 24,
            colors.text,
        );
        canvas.draw_text(
            type_rect.x + type_rect.w as i32 - 16,
            type_rect.y + 7,
            "v",
            colors.muted,
        );

        let can_confirm = self.can_confirm();
        self.confirm.set_enabled(can_confirm);
        self.confirm.draw_control(canvas, can_confirm);
        self.cancel
            .draw_control(canvas, self.focus == FocusTarget::Cancel);
        if self.type_open {
            self.draw_type_dropdown(canvas, colors);
        }
    }

    fn draw_type_dropdown(&self, canvas: &mut Canvas, colors: FileUiColors) {
        let rect = self.type_rect();
        let height = self.filters.len() as i32 * 24 + 4;
        let y = rect.y - height;
        canvas.fill_rect(rect.x, y, rect.w, height as u32, colors.surface);
        canvas.rect(rect.x, y, rect.w, height as u32, colors.border);
        for (index, filter) in self.filters.iter().enumerate() {
            let row_y = y + 2 + index as i32 * 24;
            if index == self.active_filter {
                canvas.fill_rect(rect.x + 2, row_y, rect.w - 4, 24, colors.selection);
            }
            draw_clipped(
                canvas,
                rect.x + 7,
                row_y + 8,
                &filter.label,
                rect.w as i32 - 14,
                colors.text,
            );
        }
    }

    fn draw_overwrite(&self, canvas: &mut Canvas, width: i32, height: i32, colors: FileUiColors) {
        let panel = UiRect::new((width - 420) / 2, (height - 130) / 2, 420, 130);
        canvas.fill_rect(panel.x, panel.y, panel.w, panel.h, colors.surface);
        canvas.rect(panel.x, panel.y, panel.w, panel.h, colors.accent);
        canvas.draw_text(
            panel.x + 16,
            panel.y + 18,
            "Replace existing file?",
            colors.text,
        );
        if let Some(path) = self.overwrite.as_ref() {
            draw_clipped(
                canvas,
                panel.x + 16,
                panel.y + 44,
                path,
                panel.w as i32 - 32,
                colors.muted,
            );
        }
        let replace = Button::new("Replace", panel.x + 216, panel.y + 86, 88, 26);
        let cancel = Button::new("Cancel", panel.x + 316, panel.y + 86, 88, 26);
        replace.draw(canvas, true);
        cancel.draw(canvas, false);
    }

    fn status_line(&self) -> String {
        if self.status != item_count(self.visible.len()) {
            return self.status.clone();
        }
        let Some(path) = self.selected.as_ref() else {
            return item_count(self.visible.len());
        };
        if let Some(entry) = self.entries.iter().find(|entry| &entry.path == path) {
            if entry.is_dir() {
                format!("{} | Folder", entry.name)
            } else {
                format!("{} | {}", entry.name, format_size(entry.size))
            }
        } else {
            item_count(self.visible.len())
        }
    }

    fn type_rect(&self) -> UiRect {
        UiRect::new(self.name.x + self.name.w as i32 + 8, self.name.y, 178, 24)
    }

    fn can_confirm(&self) -> bool {
        if self.overwrite.is_some() || self.folder_editor.is_some() {
            return false;
        }
        let input = self.name.text.trim();
        if input.is_empty() {
            return false;
        }
        let path = self.resolved_name(false);
        match self.mode {
            FileMode::Open => stat_path(&path)
                .is_some_and(|stat| !stat.is_dir && self.active_filter().matches(basename(&path))),
            FileMode::Save => {
                let parent = parent_path(&path).unwrap_or_else(|| "/".to_string());
                !capabilities(&parent).read_only && valid_save_input(input)
            }
        }
    }

    fn resolved_name(&self, apply_extension: bool) -> String {
        let input = self.name.text.trim();
        let mut path = if input.starts_with('/') {
            normalize_path(input)
        } else {
            join_path(&self.current_dir, input)
        };
        if apply_extension
            && self.mode == FileMode::Save
            && file_extension(basename(&path)).is_empty()
        {
            if let Some(extension) = self.default_extension.as_ref().or_else(|| {
                let filter = self.active_filter();
                (filter.extensions.len() == 1).then(|| &filter.extensions[0])
            }) {
                path.push('.');
                path.push_str(extension);
            }
        }
        path
    }

    fn commit(&mut self) -> DialogStatus<String> {
        let input = self.name.text.trim();
        if input.is_empty() {
            self.status = "Choose a file name first".to_string();
            return DialogStatus::Pending;
        }
        let path = self.resolved_name(true);
        let stat = stat_path(&path);
        match self.mode {
            FileMode::Open => match stat {
                Some(info) if info.is_dir => {
                    self.navigate(&path, true);
                    DialogStatus::Pending
                }
                Some(_) if self.active_filter().matches(basename(&path)) => {
                    DialogStatus::Done(Some(path))
                }
                Some(_) => {
                    self.status = "The file does not match the selected file type".to_string();
                    DialogStatus::Pending
                }
                None => {
                    self.status = "The selected file no longer exists".to_string();
                    DialogStatus::Pending
                }
            },
            FileMode::Save => {
                if !valid_save_input(input) {
                    self.status = "Enter a valid file name".to_string();
                    return DialogStatus::Pending;
                }
                if stat.as_ref().is_some_and(|info| info.is_dir) {
                    self.status = "A folder cannot be used as a file name".to_string();
                    return DialogStatus::Pending;
                }
                let parent = parent_path(&path).unwrap_or_else(|| "/".to_string());
                if capabilities(&parent).read_only {
                    self.status = "This location is read-only".to_string();
                    return DialogStatus::Pending;
                }
                if stat.is_some() {
                    self.overwrite = Some(path);
                    self.focus = FocusTarget::Overwrite;
                    DialogStatus::Pending
                } else if stat_path(&parent).is_some_and(|info| info.is_dir) {
                    DialogStatus::Done(Some(path))
                } else {
                    self.status = "The destination folder does not exist".to_string();
                    DialogStatus::Pending
                }
            }
        }
    }

    fn activate_position(&mut self, position: usize) -> DialogStatus<String> {
        let Some(entry_index) = self.visible.get(position).copied() else {
            return DialogStatus::Pending;
        };
        let entry = self.entries[entry_index].clone();
        self.selected = Some(entry.path.clone());
        if entry.is_dir() {
            self.navigate(&entry.path, true);
            DialogStatus::Pending
        } else {
            self.name.set_text(&entry.name);
            self.commit()
        }
    }

    fn selected_position(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.visible
            .iter()
            .position(|index| self.entries[*index].path == *selected)
    }

    fn select_position(&mut self, position: usize) {
        let Some(entry_index) = self.visible.get(position).copied() else {
            return;
        };
        let entry = &self.entries[entry_index];
        self.selected = Some(entry.path.clone());
        if entry.is_dir() {
            self.name.set_text("");
        } else {
            self.name.set_text(&entry.name);
        }
        self.ensure_visible(position);
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let current = self.selected_position().unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(self.visible.len() - 1)
        };
        self.select_position(next);
    }

    fn ensure_visible(&mut self, position: usize) {
        let height = self.window.canvas().height() as i32;
        let content_bottom = height.max(MIN_H) - FOOTER_H;
        let page = match self.view {
            FileView::Details => ((content_bottom - TOOLBAR_H - HEADER_H) / ROW_H).max(1) as usize,
            FileView::Grid => {
                let rows = ((content_bottom - TOOLBAR_H) / TILE_H).max(1) as usize;
                rows * self.grid_columns()
            }
        };
        if position < self.scroll {
            self.scroll = position;
        } else if position >= self.scroll + page {
            self.scroll = position + 1 - page;
        }
    }

    fn grid_columns(&self) -> usize {
        ((self.window.canvas().width() as i32).max(MIN_W) - SIDEBAR_W)
            .div_euclid(TILE_W)
            .max(1) as usize
    }

    fn sync_browser_scroll(&mut self) {
        let width = (self.window.canvas().width() as i32).max(MIN_W);
        let height = (self.window.canvas().height() as i32).max(MIN_H);
        let bottom = height - FOOTER_H;
        let (top, page, step) = match self.view {
            FileView::Details => {
                let top = TOOLBAR_H + HEADER_H;
                (top, ((bottom - top) / ROW_H).max(1) as usize, 1)
            }
            FileView::Grid => {
                let columns = self.grid_columns();
                let rows = ((bottom - TOOLBAR_H) / TILE_H).max(1) as usize;
                (TOOLBAR_H, rows * columns, columns)
            }
        };
        self.browser_scroll.configure(
            width - 2,
            top + 2,
            (bottom - top - 4).max(1) as u32,
            self.visible.len(),
            page,
            self.scroll,
            step,
        );
        self.scroll = self.browser_scroll.first();
    }

    fn entry_position_at(&self, x: i32, y: i32) -> Option<usize> {
        let width = (self.window.canvas().width() as i32).max(MIN_W);
        let bottom = (self.window.canvas().height() as i32).max(MIN_H) - FOOTER_H;
        if x < SIDEBAR_W || x >= width || y < TOOLBAR_H || y >= bottom {
            return None;
        }
        let position = match self.view {
            FileView::Details => {
                if y < TOOLBAR_H + HEADER_H {
                    return None;
                }
                self.scroll + ((y - TOOLBAR_H - HEADER_H) / ROW_H) as usize
            }
            FileView::Grid => {
                let column = ((x - SIDEBAR_W) / TILE_W) as usize;
                let row = ((y - TOOLBAR_H) / TILE_H) as usize;
                self.scroll + row * self.grid_columns() + column
            }
        };
        (position < self.visible.len()).then_some(position)
    }

    fn header_click(&mut self, x: i32) {
        let content_w = (self.window.canvas().width() as i32).max(MIN_W) - SIDEBAR_W;
        let name_w = (content_w - 294).max(190);
        let relative = x - SIDEBAR_W;
        let key = if relative < name_w {
            SortKey::Name
        } else if relative < name_w + 78 {
            SortKey::Size
        } else if relative < content_w - 116 {
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

    fn cycle_focus(&mut self, backwards: bool) {
        const ORDER: [FocusTarget; 7] = [
            FocusTarget::Places,
            FocusTarget::Content,
            FocusTarget::Filter,
            FocusTarget::Name,
            FocusTarget::FileType,
            FocusTarget::Confirm,
            FocusTarget::Cancel,
        ];
        let current = ORDER
            .iter()
            .position(|target| *target == self.focus)
            .unwrap_or(0);
        let next = if backwards {
            current.checked_sub(1).unwrap_or(ORDER.len() - 1)
        } else {
            (current + 1) % ORDER.len()
        };
        self.focus = ORDER[next];
        self.type_open = false;
    }

    fn begin_new_folder(&mut self) {
        if !capabilities(&self.current_dir).directories {
            self.status = "Folders cannot be created in this location".to_string();
            return;
        }
        self.folder_editor = Some(TextField::new(0, 0, 0, 0, "New Folder"));
        self.focus = FocusTarget::FolderName;
        self.relayout();
    }

    fn commit_new_folder(&mut self) {
        let Some(editor) = self.folder_editor.as_ref() else {
            return;
        };
        let name = editor.text.trim().to_string();
        if !valid_leaf(&name) {
            self.status = "Enter a valid folder name".to_string();
            return;
        }
        let target = join_path(&self.current_dir, &name);
        if stat_path(&target).is_some() {
            self.status = "An item with that name already exists".to_string();
            return;
        }
        let result = runtime::mkdir(&gui::c_path(&target), 0o755);
        if result < 0 {
            self.status = format!("Could not create folder ({result})");
            return;
        }
        self.folder_editor = None;
        self.focus = FocusTarget::Content;
        let mut sync_failed = false;
        if capabilities(&self.current_dir).sync_backed {
            let sync_result = runtime::sync();
            if sync_result < 0 {
                sync_failed = true;
            }
        }
        if self.reload().is_ok() {
            self.selected = Some(target);
            if sync_failed {
                self.status =
                    "Folder created, but sync failed; it may not survive reboot".to_string();
            }
        }
    }

    fn handle_overwrite_key(&mut self, key: u32) -> DialogStatus<String> {
        match key {
            runtime::KEY_ENTER => DialogStatus::Done(self.overwrite.take()),
            runtime::KEY_ESCAPE => {
                self.overwrite = None;
                self.focus = FocusTarget::Name;
                DialogStatus::Pending
            }
            _ => DialogStatus::Pending,
        }
    }

    fn handle_key(&mut self, payload: [u32; 6]) -> DialogStatus<String> {
        let key = payload[0];
        let character = char::from_u32(payload[1]).unwrap_or('\0');
        let shift = payload[2] & 1 != 0;
        let ctrl = payload[2] & 2 != 0;
        let alt = payload[2] & 4 != 0;

        if self.overwrite.is_some() {
            return self.handle_overwrite_key(key);
        }
        if self.folder_editor.is_some() {
            match key {
                runtime::KEY_ESCAPE => {
                    self.folder_editor = None;
                    self.focus = FocusTarget::Content;
                }
                runtime::KEY_ENTER => self.commit_new_folder(),
                _ => {
                    if let Some(editor) = self.folder_editor.as_mut() {
                        editor.key(key, character);
                    }
                }
            }
            return DialogStatus::Pending;
        }
        if key == runtime::KEY_TAB {
            self.cycle_focus(shift);
            return DialogStatus::Pending;
        }
        if ctrl {
            match character.to_ascii_lowercase() {
                'l' => {
                    self.location.set_text(&self.current_dir);
                    self.focus = FocusTarget::Location;
                }
                'f' => self.focus = FocusTarget::Filter,
                'n' if shift => self.begin_new_folder(),
                _ => {}
            }
            return DialogStatus::Pending;
        }
        if alt {
            match key {
                runtime::KEY_LEFT => self.go_back(),
                runtime::KEY_RIGHT => self.go_forward(),
                _ => {}
            }
            return DialogStatus::Pending;
        }

        match self.focus {
            FocusTarget::Filter => {
                if key == runtime::KEY_ESCAPE {
                    self.filter.set_text("");
                    self.rebuild_visible();
                    self.focus = FocusTarget::Content;
                } else if key == runtime::KEY_ENTER {
                    self.focus = FocusTarget::Content;
                } else if self.filter.key(key, character) {
                    self.rebuild_visible();
                }
                return DialogStatus::Pending;
            }
            FocusTarget::Location => {
                if key == runtime::KEY_ESCAPE {
                    self.location.set_text(&self.current_dir);
                    self.focus = FocusTarget::Content;
                } else if key == runtime::KEY_ENTER {
                    let target = resolve_location(&self.current_dir, &self.location.text);
                    self.focus = FocusTarget::Content;
                    self.navigate(&target, true);
                } else {
                    self.location.key(key, character);
                }
                return DialogStatus::Pending;
            }
            FocusTarget::Name => {
                if key == runtime::KEY_ESCAPE {
                    return DialogStatus::Done(None);
                }
                if key == runtime::KEY_ENTER {
                    return self.commit();
                }
                self.name.key(key, character);
                return DialogStatus::Pending;
            }
            FocusTarget::FileType => {
                match key {
                    runtime::KEY_ESCAPE => self.type_open = false,
                    runtime::KEY_ENTER | runtime::KEY_SPACE => self.type_open = !self.type_open,
                    runtime::KEY_UP => {
                        self.active_filter = self.active_filter.saturating_sub(1);
                        self.rebuild_visible();
                    }
                    runtime::KEY_DOWN => {
                        self.active_filter =
                            (self.active_filter + 1).min(self.filters.len().saturating_sub(1));
                        self.rebuild_visible();
                    }
                    runtime::KEY_HOME => {
                        self.active_filter = 0;
                        self.rebuild_visible();
                    }
                    runtime::KEY_END => {
                        self.active_filter = self.filters.len().saturating_sub(1);
                        self.rebuild_visible();
                    }
                    _ => {}
                }
                return DialogStatus::Pending;
            }
            FocusTarget::Confirm => {
                return if key == runtime::KEY_ENTER || key == runtime::KEY_SPACE {
                    self.commit()
                } else if key == runtime::KEY_ESCAPE {
                    DialogStatus::Done(None)
                } else {
                    DialogStatus::Pending
                };
            }
            FocusTarget::Cancel => {
                return if matches!(
                    key,
                    runtime::KEY_ENTER | runtime::KEY_SPACE | runtime::KEY_ESCAPE
                ) {
                    DialogStatus::Done(None)
                } else {
                    DialogStatus::Pending
                };
            }
            FocusTarget::Places => {
                match key {
                    runtime::KEY_ESCAPE => return DialogStatus::Done(None),
                    runtime::KEY_UP => {
                        self.place_focus = self.place_focus.saturating_sub(1);
                    }
                    runtime::KEY_DOWN => {
                        self.place_focus =
                            (self.place_focus + 1).min(self.sidebar.places.len().saturating_sub(1));
                    }
                    runtime::KEY_HOME => self.place_focus = 0,
                    runtime::KEY_END => {
                        self.place_focus = self.sidebar.places.len().saturating_sub(1);
                    }
                    runtime::KEY_ENTER | runtime::KEY_SPACE => {
                        if let Some(target) = self
                            .sidebar
                            .places
                            .get(self.place_focus)
                            .map(|place| place.path.clone())
                        {
                            self.navigate(&target, true);
                        }
                    }
                    _ => {}
                }
                return DialogStatus::Pending;
            }
            _ => {}
        }

        match key {
            runtime::KEY_ESCAPE => return DialogStatus::Done(None),
            runtime::KEY_BACKSPACE => self.go_up(),
            runtime::KEY_F5 => {
                if let Err(error) = self.reload() {
                    self.status = format!("Refresh failed ({error})");
                }
            }
            runtime::KEY_ENTER => {
                if let Some(position) = self.selected_position() {
                    return self.activate_position(position);
                }
                return self.commit();
            }
            runtime::KEY_UP => self.move_selection(-1),
            runtime::KEY_DOWN => self.move_selection(1),
            runtime::KEY_PAGE_UP => self.move_selection(-10),
            runtime::KEY_PAGE_DOWN => self.move_selection(10),
            runtime::KEY_HOME if !self.visible.is_empty() => self.select_position(0),
            runtime::KEY_END if !self.visible.is_empty() => {
                self.select_position(self.visible.len() - 1)
            }
            _ => {}
        }
        DialogStatus::Pending
    }

    fn handle_mouse(&mut self, event: &runtime::GuiEvent) -> DialogStatus<String> {
        let x = event.payload[0] as i32;
        let y = event.payload[1] as i32;
        let action = event.payload[3];

        if self.overwrite.is_some() {
            if action == runtime::GUI_MOUSE_DOWN && event.payload[2] & 1 != 0 {
                let width = (self.window.canvas().width() as i32).max(MIN_W);
                let height = (self.window.canvas().height() as i32).max(MIN_H);
                let panel = UiRect::new((width - 420) / 2, (height - 130) / 2, 420, 130);
                if UiRect::new(panel.x + 216, panel.y + 86, 88, 26).hit(x, y) {
                    return DialogStatus::Done(self.overwrite.take());
                }
                if UiRect::new(panel.x + 316, panel.y + 86, 88, 26).hit(x, y) {
                    self.overwrite = None;
                    self.focus = FocusTarget::Name;
                }
            }
            return DialogStatus::Pending;
        }

        if let Some(editor) = self.folder_editor.as_mut() {
            if action == runtime::GUI_MOUSE_DOWN && event.payload[2] & 1 != 0 && editor.hit(x, y) {
                editor.click(x);
            }
            return DialogStatus::Pending;
        }

        self.sync_browser_scroll();
        if let Some(ControlInput::Pointer(input)) = decode_control_input(event) {
            if matches!(input.kind, PointerKind::Down) {
                if self.confirm.hit(input.x, input.y) {
                    self.focus = FocusTarget::Confirm;
                } else if self.cancel.hit(input.x, input.y) {
                    self.focus = FocusTarget::Cancel;
                }
            }
            let confirm = self.confirm.handle_pointer(input);
            if confirm.action == Some(ButtonAction::Activated) {
                return if self.can_confirm() {
                    self.commit()
                } else {
                    self.status = "Choose a valid file first".to_string();
                    DialogStatus::Pending
                };
            }
            let cancel = self.cancel.handle_pointer(input);
            if cancel.action == Some(ButtonAction::Activated) {
                return DialogStatus::Done(None);
            }
            let response = self.browser_scroll.handle_pointer(input);
            if response.consumed {
                self.scroll = self.browser_scroll.first();
                return DialogStatus::Pending;
            }
        }

        if action == runtime::GUI_MOUSE_SCROLL {
            let bottom = (self.window.canvas().height() as i32).max(MIN_H) - FOOTER_H;
            if x < SIDEBAR_W || y < TOOLBAR_H || y >= bottom {
                return DialogStatus::Pending;
            }
            let step = match self.view {
                FileView::Details => 3,
                FileView::Grid => self.grid_columns(),
            };
            if (event.payload[5] as i32) < 0 {
                self.scroll = self.scroll.saturating_sub(step);
            } else {
                self.scroll = (self.scroll + step).min(self.visible.len().saturating_sub(1));
            }
            return DialogStatus::Pending;
        }
        if action != runtime::GUI_MOUSE_DOWN || event.payload[2] & 1 == 0 {
            return DialogStatus::Pending;
        }

        if self.type_open {
            let rect = self.type_rect();
            let height = self.filters.len() as i32 * 24 + 4;
            let top = rect.y - height;
            if x >= rect.x && x < rect.x + rect.w as i32 && y >= top && y < rect.y {
                let index = ((y - top - 2).max(0) / 24) as usize;
                if index < self.filters.len() {
                    self.active_filter = index;
                    self.rebuild_visible();
                }
                self.type_open = false;
                return DialogStatus::Pending;
            }
            self.type_open = false;
        }

        if UiRect::new(10, 10, 30, 28).hit(x, y) {
            self.go_back();
            return DialogStatus::Pending;
        }
        if UiRect::new(46, 10, 30, 28).hit(x, y) {
            self.go_forward();
            return DialogStatus::Pending;
        }
        if UiRect::new(82, 10, 30, 28).hit(x, y) {
            self.go_up();
            return DialogStatus::Pending;
        }
        if UiRect::new(118, 10, 30, 28).hit(x, y) {
            self.begin_new_folder();
            return DialogStatus::Pending;
        }
        if UiRect::new(154, 10, 30, 28).hit(x, y) {
            if let Err(error) = self.reload() {
                self.status = format!("Refresh failed ({error})");
            }
            return DialogStatus::Pending;
        }
        if self.filter.hit(x, y) {
            self.filter.click(x);
            self.focus = FocusTarget::Filter;
            return DialogStatus::Pending;
        }
        if let Some(target) = self.breadcrumbs.hit(x, y).map(ToString::to_string) {
            self.navigate(&target, true);
            return DialogStatus::Pending;
        }
        if (10..38).contains(&y) && x >= 198 && x < self.filter.x - 10 {
            self.location.set_text(&self.current_dir);
            self.location.click(x);
            self.focus = FocusTarget::Location;
            return DialogStatus::Pending;
        }
        if let Some(target) = self.sidebar.hit(x, y).map(ToString::to_string) {
            self.focus = FocusTarget::Places;
            if let Some(index) = self
                .sidebar
                .places
                .iter()
                .position(|place| place.path == target)
            {
                self.place_focus = index;
            }
            self.navigate(&target, true);
            return DialogStatus::Pending;
        }

        let width = (self.window.canvas().width() as i32).max(MIN_W);
        let bottom = (self.window.canvas().height() as i32).max(MIN_H) - FOOTER_H;
        if UiRect::new(width - 84, bottom + 2, 32, 18).hit(x, y) {
            self.view = FileView::Details;
            self.scroll = 0;
            return DialogStatus::Pending;
        }
        if UiRect::new(width - 46, bottom + 2, 32, 18).hit(x, y) {
            self.view = FileView::Grid;
            self.scroll = 0;
            return DialogStatus::Pending;
        }
        if self.name.hit(x, y) {
            self.name.click(x);
            self.focus = FocusTarget::Name;
            return DialogStatus::Pending;
        }
        if self.type_rect().hit(x, y) {
            self.focus = FocusTarget::FileType;
            self.type_open = !self.type_open;
            return DialogStatus::Pending;
        }
        if x >= SIDEBAR_W
            && self.view == FileView::Details
            && (TOOLBAR_H..TOOLBAR_H + HEADER_H).contains(&y)
        {
            self.header_click(x);
            return DialogStatus::Pending;
        }
        let Some(position) = self.entry_position_at(x, y) else {
            if x >= SIDEBAR_W && y >= TOOLBAR_H && y < bottom {
                self.selected = None;
                self.focus = FocusTarget::Content;
            }
            return DialogStatus::Pending;
        };
        self.select_position(position);
        self.focus = FocusTarget::Content;
        let path = self.entries[self.visible[position]].path.clone();
        let tick = event.payload[4] as u64 | ((event.payload[5] as u64) << 32);
        let double = self.last_click.as_ref().is_some_and(|last| {
            last.path == path
                && tick.saturating_sub(last.tick) <= 50
                && (last.x - x).abs() <= 4
                && (last.y - y).abs() <= 4
        });
        self.last_click = Some(LastClick { path, tick, x, y });
        if double {
            self.last_click = None;
            return self.activate_position(position);
        }
        DialogStatus::Pending
    }

    pub fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<String> {
        let status = match event.kind {
            runtime::GUI_EVENT_CLOSE => return DialogStatus::Done(None),
            runtime::GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                DialogStatus::Pending
            }
            runtime::GUI_EVENT_KEY if event.payload[3] != 0 => self.handle_key(event.payload),
            runtime::GUI_EVENT_MOUSE => self.handle_mouse(event),
            _ => return DialogStatus::Pending,
        };
        if matches!(&status, DialogStatus::Pending) {
            self.render();
        }
        status
    }
}

#[derive(Clone, Copy)]
struct PathStat {
    is_dir: bool,
}

fn stat_path(path: &str) -> Option<PathStat> {
    let mut stat = runtime::LinuxStat::default();
    if runtime::newfstatat(runtime::AT_FDCWD, &gui::c_path(path), &mut stat, 0) < 0 {
        return None;
    }
    Some(PathStat {
        is_dir: stat.st_mode & 0o170000 == 0o040000,
    })
}

fn build_places(current: &str, extras: Vec<FilePlace>) -> Vec<FilePlace> {
    let mut places = Vec::new();
    if current != "/" && current != "/data" && current != "/host" {
        places.push(FilePlace::new("Start", current, PlaceIcon::Folder));
    }
    places.push(FilePlace::new("Root", "/", PlaceIcon::Root));
    if stat_path("/data").is_some_and(|stat| stat.is_dir) {
        places.push(FilePlace::new("Data", "/data", PlaceIcon::Data));
    }
    if stat_path("/host").is_some_and(|stat| stat.is_dir) {
        places.push(FilePlace::new("Host", "/host", PlaceIcon::Host));
    }
    for place in extras {
        if !places
            .iter()
            .any(|existing: &FilePlace| existing.path == place.path)
        {
            places.push(place);
        }
    }
    places
}

fn initial_directory_and_name(mode: FileMode, input: &str) -> (String, String) {
    let normalized = normalize_path(input);
    if input.ends_with('/') || stat_path(&normalized).is_some_and(|stat| stat.is_dir) {
        return (normalized, String::new());
    }
    match mode {
        FileMode::Open => {
            if stat_path(&normalized).is_some() {
                (
                    parent_path(&normalized).unwrap_or_else(|| "/".to_string()),
                    basename(&normalized).to_string(),
                )
            } else {
                (normalized, String::new())
            }
        }
        FileMode::Save => (
            parent_path(&normalized).unwrap_or_else(|| "/".to_string()),
            basename(&normalized).to_string(),
        ),
    }
}

fn normalize_path(input: &str) -> String {
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
    let mut result = String::from("/");
    result.push_str(&parts.join("/"));
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
        Some(0) | None => Some("/".to_string()),
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

fn valid_leaf(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

fn valid_save_input(input: &str) -> bool {
    if input.ends_with('/') || input.contains('\0') {
        return false;
    }
    valid_leaf(basename(input))
}

fn compare_name(a: &str, b: &str) -> Ordering {
    a.to_ascii_lowercase()
        .cmp(&b.to_ascii_lowercase())
        .then_with(|| a.cmp(b))
}

fn sort_label(label: &str, active: bool, descending: bool) -> String {
    if active {
        format!("{} {}", label, if descending { "v" } else { "^" })
    } else {
        label.to_string()
    }
}

fn item_count(count: usize) -> String {
    if count == 1 {
        "1 item".to_string()
    } else {
        format!("{count} items")
    }
}

fn draw_view_toggle(canvas: &mut Canvas, width: i32, y: i32, view: FileView, colors: FileUiColors) {
    let list = UiRect::new(width - 84, y, 32, 18);
    let grid = UiRect::new(width - 46, y, 32, 18);
    for (rect, active) in [
        (list, view == FileView::Details),
        (grid, view == FileView::Grid),
    ] {
        canvas.fill_rect(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            if active {
                colors.selection
            } else {
                colors.surface
            },
        );
        canvas.rect(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            if active { colors.accent } else { colors.border },
        );
    }
    for row in 0..3 {
        canvas.horizontal_line(list.x + 8, list.y + 5 + row * 4, 16, colors.accent);
    }
    for row in 0..2 {
        for column in 0..3 {
            canvas.rect(
                grid.x + 7 + column * 7,
                grid.y + 4 + row * 7,
                5,
                5,
                colors.accent,
            );
        }
    }
}
