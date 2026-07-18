//! Shared presentation primitives for local-file browsers.
//!
//! This module deliberately contains drawing, hit geometry, and the current
//! mount capability description. Directory navigation and selection policy
//! remain with the caller: File Manager is multi-select and operational, while
//! the common dialog is a single-select commit surface.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{Canvas, FONT_CELL_WIDTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl UiRect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn hit(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w as i32 && y >= self.y && y < self.y + self.h as i32
    }
}

#[derive(Clone, Copy)]
pub struct FileUiColors {
    pub background: u32,
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
    pub selection: u32,
    pub folder: u32,
    pub read_only: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavIcon {
    Back,
    Forward,
    Up,
    Home,
    NewFolder,
    Refresh,
}

pub struct IconButton {
    pub rect: UiRect,
    pub icon: NavIcon,
    pub enabled: bool,
}

impl IconButton {
    pub const fn new(rect: UiRect, icon: NavIcon, enabled: bool) -> Self {
        Self {
            rect,
            icon,
            enabled,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        self.enabled && self.rect.hit(x, y)
    }

    pub fn draw(&self, canvas: &mut Canvas, colors: FileUiColors) {
        let rect = self.rect;
        canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, colors.surface);
        canvas.rect(rect.x, rect.y, rect.w, rect.h, colors.border);
        let color = if self.enabled {
            colors.text
        } else {
            colors.border
        };
        let cx = rect.x + rect.w as i32 / 2;
        let cy = rect.y + rect.h as i32 / 2;
        match self.icon {
            NavIcon::Back => {
                for i in 0..6 {
                    canvas.pixel(cx - 4 + i, cy - i, color);
                    canvas.pixel(cx - 4 + i, cy + i, color);
                }
                canvas.horizontal_line(cx - 3, cy, 11, color);
            }
            NavIcon::Forward => {
                for i in 0..6 {
                    canvas.pixel(cx + 4 - i, cy - i, color);
                    canvas.pixel(cx + 4 - i, cy + i, color);
                }
                canvas.horizontal_line(cx - 7, cy, 11, color);
            }
            NavIcon::Up => {
                for i in 0..6 {
                    canvas.pixel(cx - i, cy - 3 + i, color);
                    canvas.pixel(cx + i, cy - 3 + i, color);
                }
                canvas.vertical_line(cx, cy - 2, 11, color);
            }
            NavIcon::Home => {
                for i in 0..7 {
                    canvas.pixel(cx - 7 + i, cy - 1 - i, color);
                    canvas.pixel(cx + i, cy - 7 + i, color);
                }
                canvas.rect(cx - 6, cy - 1, 13, 9, color);
            }
            NavIcon::NewFolder => {
                canvas.rect(cx - 8, cy - 5, 16, 12, color);
                canvas.horizontal_line(cx - 7, cy - 7, 7, color);
                canvas.horizontal_line(cx - 2, cy, 9, color);
                canvas.vertical_line(cx + 2, cy - 4, 9, color);
            }
            NavIcon::Refresh => {
                for i in 0..7 {
                    canvas.pixel(cx - 7 + i, cy - 5 - i / 2, color);
                    canvas.pixel(cx + 7 - i, cy + 5 + i / 2, color);
                }
                canvas.vertical_line(cx - 7, cy - 5, 8, color);
                canvas.vertical_line(cx + 7, cy - 2, 8, color);
                canvas.horizontal_line(cx - 9, cy - 5, 5, color);
                canvas.horizontal_line(cx + 5, cy + 5, 5, color);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceIcon {
    Home,
    Root,
    Data,
    Host,
    Folder,
}

#[derive(Clone)]
pub struct FilePlace {
    pub label: String,
    pub path: String,
    pub icon: PlaceIcon,
}

impl FilePlace {
    pub fn new(label: &str, path: &str, icon: PlaceIcon) -> Self {
        Self {
            label: label.to_string(),
            path: path.to_string(),
            icon,
        }
    }
}

pub struct PlacesSidebar {
    pub bounds: UiRect,
    pub places: Vec<FilePlace>,
}

impl PlacesSidebar {
    pub const ROW_HEIGHT: i32 = 36;

    pub fn new(bounds: UiRect, places: Vec<FilePlace>) -> Self {
        Self { bounds, places }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<&str> {
        if !self.bounds.hit(x, y) || y < self.bounds.y + 34 {
            return None;
        }
        let index = ((y - self.bounds.y - 34) / Self::ROW_HEIGHT) as usize;
        self.places.get(index).map(|place| place.path.as_str())
    }

    pub fn draw(&self, canvas: &mut Canvas, current: &str, colors: FileUiColors) {
        canvas.fill_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            colors.background,
        );
        canvas.vertical_line(
            self.bounds.x + self.bounds.w as i32 - 1,
            self.bounds.y,
            self.bounds.h,
            colors.border,
        );
        canvas.draw_text(
            self.bounds.x + 14,
            self.bounds.y + 14,
            "PLACES",
            colors.muted,
        );
        for (index, place) in self.places.iter().enumerate() {
            let y = self.bounds.y + 34 + index as i32 * Self::ROW_HEIGHT;
            if y + 30 > self.bounds.y + self.bounds.h as i32 {
                break;
            }
            let active = current == place.path
                || (place.path != "/"
                    && current
                        .strip_prefix(&place.path)
                        .is_some_and(|rest| rest.starts_with('/')));
            if active {
                canvas.fill_rect(
                    self.bounds.x + 8,
                    y,
                    self.bounds.w.saturating_sub(17),
                    30,
                    colors.selection,
                );
                canvas.rect(
                    self.bounds.x + 8,
                    y,
                    self.bounds.w.saturating_sub(17),
                    30,
                    colors.accent,
                );
            }
            draw_place_icon(canvas, self.bounds.x + 18, y + 8, place.icon, colors);
            draw_clipped(
                canvas,
                self.bounds.x + 42,
                y + 11,
                &place.label,
                self.bounds.w as i32 - 50,
                if active { colors.accent } else { colors.text },
            );
        }
    }
}

fn draw_place_icon(canvas: &mut Canvas, x: i32, y: i32, icon: PlaceIcon, colors: FileUiColors) {
    match icon {
        PlaceIcon::Home => {
            canvas.rect(x, y + 5, 14, 10, colors.accent);
            for i in 0..8 {
                canvas.pixel(x + i, y + 7 - i, colors.accent);
                canvas.pixel(x + 7 + i, y + i, colors.accent);
            }
        }
        PlaceIcon::Root => canvas.rect(x, y + 1, 15, 15, colors.muted),
        PlaceIcon::Data => {
            canvas.rect(x, y + 2, 16, 13, colors.accent);
            canvas.horizontal_line(x + 3, y + 11, 10, colors.accent);
        }
        PlaceIcon::Host => {
            canvas.rect(x, y + 2, 16, 13, colors.read_only);
            canvas.vertical_line(x + 8, y + 5, 7, colors.read_only);
        }
        PlaceIcon::Folder => draw_file_icon(canvas, x, y, FileIconKind::Folder, 16, colors),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileIconKind {
    Folder,
    Text,
    Executable,
    Image,
    Archive,
    File,
}

impl FileIconKind {
    pub const fn type_name(self) -> &'static str {
        match self {
            FileIconKind::Folder => "Folder",
            FileIconKind::Text => "Text document",
            FileIconKind::Executable => "Application",
            FileIconKind::Image => "Image",
            FileIconKind::Archive => "Archive",
            FileIconKind::File => "File",
        }
    }
}

pub fn classify_file(name: &str, is_dir: bool, mode: u32) -> FileIconKind {
    if is_dir {
        return FileIconKind::Folder;
    }
    let extension = file_extension(name);
    if extension == "elf" || mode & 0o111 != 0 {
        FileIconKind::Executable
    } else if matches!(
        extension.as_str(),
        "txt" | "md" | "rs" | "toml" | "json" | "log" | "conf" | "sh" | "c" | "h" | "cpp"
    ) {
        FileIconKind::Text
    } else if matches!(extension.as_str(), "bmp" | "png" | "jpg" | "jpeg" | "gif") {
        FileIconKind::Image
    } else if matches!(extension.as_str(), "zip" | "tar" | "gz" | "bz2") {
        FileIconKind::Archive
    } else {
        FileIconKind::File
    }
}

pub fn file_extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn draw_file_icon(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    kind: FileIconKind,
    size: u32,
    colors: FileUiColors,
) {
    let s = size as i32;
    match kind {
        FileIconKind::Folder => {
            canvas.fill_rect(x, y + s / 4, size, (s * 3 / 4) as u32, colors.folder);
            canvas.fill_rect(x + 2, y, (s / 2) as u32, (s / 3) as u32, 0xF4CE72);
            canvas.rect(x, y + s / 4, size, (s * 3 / 4) as u32, 0xB98520);
        }
        _ => {
            let color = match kind {
                FileIconKind::Text => 0x4F8DD6,
                FileIconKind::Executable => 0x5C6BC0,
                FileIconKind::Image => 0x4E9F66,
                FileIconKind::Archive => 0xA66B3D,
                FileIconKind::File => 0x8A94A6,
                FileIconKind::Folder => colors.folder,
            };
            canvas.fill_rect(x, y, size, size, 0xFDFEFF);
            canvas.rect(x, y, size, size, color);
            canvas.fill_rect(x + 3, y + 4, size.saturating_sub(6), 3, color);
            canvas.fill_rect(x + 3, y + 10, size.saturating_sub(8), 2, color);
        }
    }
}

#[derive(Clone)]
pub struct BreadcrumbSegment {
    pub rect: UiRect,
    pub label: String,
    pub path: String,
}

pub struct BreadcrumbBar {
    pub segments: Vec<BreadcrumbSegment>,
}

impl BreadcrumbBar {
    pub const fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    pub fn rebuild(&mut self, current: &str, bounds: UiRect) {
        let mut all: Vec<(String, String)> = Vec::new();
        all.push(("Root".to_string(), "/".to_string()));
        let mut path = String::from("/");
        for component in current.trim_matches('/').split('/') {
            if component.is_empty() {
                continue;
            }
            if path != "/" {
                path.push('/');
            }
            path.push_str(component);
            all.push((component.to_string(), path.clone()));
        }

        let available = bounds.w as i32;
        let widths: Vec<i32> = all
            .iter()
            .map(|(label, _)| (label.chars().count() as i32 * FONT_CELL_WIDTH + 22).clamp(54, 150))
            .collect();
        let mut chosen: Vec<usize> = Vec::new();
        let mut used = widths[0];
        if all.len() > 1 {
            for index in (1..all.len()).rev() {
                let extra = widths[index] + 4;
                let ellipsis = if index > 1 { 38 } else { 0 };
                if used + extra + ellipsis > available {
                    break;
                }
                chosen.push(index);
                used += extra;
            }
            chosen.reverse();
        }

        self.segments.clear();
        let mut x = bounds.x;
        self.segments.push(BreadcrumbSegment {
            rect: UiRect::new(x, bounds.y, widths[0] as u32, bounds.h),
            label: all[0].0.clone(),
            path: all[0].1.clone(),
        });
        x += widths[0] + 4;
        if chosen.first().copied().unwrap_or(1) > 1 {
            self.segments.push(BreadcrumbSegment {
                rect: UiRect::new(x, bounds.y, 34, bounds.h),
                label: "...".to_string(),
                path: all[chosen[0] - 1].1.clone(),
            });
            x += 38;
        }
        for index in chosen {
            self.segments.push(BreadcrumbSegment {
                rect: UiRect::new(x, bounds.y, widths[index] as u32, bounds.h),
                label: all[index].0.clone(),
                path: all[index].1.clone(),
            });
            x += widths[index] + 4;
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<&str> {
        self.segments
            .iter()
            .find(|segment| segment.rect.hit(x, y))
            .map(|segment| segment.path.as_str())
    }

    pub fn draw(&self, canvas: &mut Canvas, colors: FileUiColors) {
        for segment in &self.segments {
            canvas.fill_rect(
                segment.rect.x,
                segment.rect.y,
                segment.rect.w,
                segment.rect.h,
                colors.surface,
            );
            canvas.rect(
                segment.rect.x,
                segment.rect.y,
                segment.rect.w,
                segment.rect.h,
                colors.border,
            );
            draw_clipped(
                canvas,
                segment.rect.x + 7,
                segment.rect.y + (segment.rect.h as i32 - crate::FONT_LINE_HEIGHT) / 2,
                &segment.label,
                segment.rect.w as i32 - 14,
                colors.text,
            );
        }
    }
}

impl Default for BreadcrumbBar {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct MountCapabilities {
    pub create_files: bool,
    pub delete_files: bool,
    pub directories: bool,
    pub rename: bool,
    pub read_only: bool,
    pub sync_backed: bool,
}

impl MountCapabilities {
    pub const fn label(self) -> &'static str {
        if self.read_only {
            "Read-only"
        } else if self.sync_backed {
            "Sync-backed"
        } else {
            "Persistent"
        }
    }
}

pub fn capabilities(path: &str) -> MountCapabilities {
    if component_prefix(path, "/host") || component_prefix(path, "/bin") {
        MountCapabilities {
            create_files: false,
            delete_files: false,
            directories: false,
            rename: false,
            read_only: true,
            sync_backed: false,
        }
    } else if component_prefix(path, "/data") {
        MountCapabilities {
            create_files: true,
            delete_files: true,
            directories: true,
            rename: true,
            read_only: false,
            sync_backed: false,
        }
    } else {
        MountCapabilities {
            create_files: true,
            delete_files: true,
            directories: true,
            rename: true,
            read_only: false,
            sync_backed: true,
        }
    }
}

pub fn component_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn draw_clipped(canvas: &mut Canvas, x: i32, y: i32, text: &str, width: i32, color: u32) {
    let chars = (width / FONT_CELL_WIDTH).max(1) as usize;
    if text.chars().count() <= chars {
        canvas.draw_text(x, y, text, color);
    } else if chars > 3 {
        let mut clipped: String = text.chars().take(chars - 3).collect();
        clipped.push_str("...");
        canvas.draw_text(x, y, &clipped, color);
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes + 512) / 1024)
    } else {
        format!("{} MB", (bytes + 512 * 1024) / (1024 * 1024))
    }
}

/// Format a Unix timestamp as a compact UTC date/time without libc.
pub fn format_modified(seconds: i64) -> String {
    if seconds <= 0 {
        return "--".to_string();
    }
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days transform, with day zero at the Unix
    // epoch. It is small, allocation-free until the final formatted String,
    // and works across Gregorian era boundaries.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524
        - day_of_era / 146_096)
        .div_euclid(365);
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2).div_euclid(153);
    let day = day_of_year - (153 * month_prime + 2).div_euclid(5) + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}
