#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use dialogs::{DialogStatus, FileDialog, MessageBox, Modal, ModalOutcome};
use gui::{Canvas, TextField, Window};
use runtime::{ApplyResult, GuiEvent, SystemControlSnapshotV1};

const BG: u32 = 0xF5F6F8;
const SIDEBAR: u32 = 0xEEF1F5;
const CARD: u32 = 0xFFFFFF;
const TEXT: u32 = 0x1F2329;
const MUTED: u32 = 0x667085;
const DIVIDER: u32 = 0xD9DEE7;
const ACCENT: u32 = 0x3478E5;
const ACCENT_SOFT: u32 = 0xE6F0FF;
const SUCCESS: u32 = 0x218739;
const WARNING: u32 = 0xA76500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Appearance,
    Desktop,
    System,
    Network,
    About,
}

impl Page {
    const ALL: [Page; 6] = [
        Page::Home,
        Page::Appearance,
        Page::Desktop,
        Page::System,
        Page::Network,
        Page::About,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Appearance => "Appearance",
            Self::Desktop => "Desktop",
            Self::System => "System",
            Self::Network => "Network",
            Self::About => "About",
        }
    }

    const fn subtitle(self) -> &'static str {
        match self {
            Self::Home => "Your AgenticOS at a glance",
            Self::Appearance => "Choose how AgenticOS looks",
            Self::Desktop => "Personalize the desktop background",
            Self::System => "Display, renderer, and memory information",
            Self::Network => "Interface activity and resolver configuration",
            Self::About => "About this AgenticOS development build",
        }
    }
}

enum ModalPurpose {
    Wallpaper,
    Dismiss,
}

struct ActiveModal {
    modal: Modal,
    purpose: ModalPurpose,
}

struct ControlCenter {
    window: Window,
    page: Page,
    search: TextField,
    search_focused: bool,
    snapshot: SystemControlSnapshotV1,
    wallpaper_path: String,
    uptime: String,
    memory: String,
    network: String,
    resolver: String,
    banner: String,
    banner_warning: bool,
    modal: Option<ActiveModal>,
}

impl ControlCenter {
    fn new() -> Result<Self, i64> {
        let window = Window::new(900, 620, "Settings")?;
        let mut app = Self {
            window,
            page: Page::Home,
            search: TextField::new(16, 20, 188, 30, ""),
            search_focused: false,
            snapshot: SystemControlSnapshotV1::default(),
            wallpaper_path: String::from("/WALLPAPR.BMP"),
            uptime: String::from("Unavailable"),
            memory: String::from("Unavailable"),
            network: String::from("Unavailable"),
            resolver: String::from("Unavailable"),
            banner: String::new(),
            banner_warning: false,
            modal: None,
        };
        app.refresh();
        Ok(app)
    }

    fn sidebar_width(&self) -> i32 {
        if self.window.canvas().width() < 800 {
            72
        } else {
            220
        }
    }

    fn refresh(&mut self) {
        if let Ok(snapshot) = runtime::system_control_snapshot() {
            self.snapshot = snapshot;
        }
        let mut path = [0u8; 1024];
        if let Ok(count) = runtime::system_control_wallpaper_path(&mut path) {
            if let Ok(value) = core::str::from_utf8(&path[..count]) {
                self.wallpaper_path = value.to_string();
            }
        }
        self.uptime = uptime_summary();
        self.memory = memory_summary();
        self.network = network_summary();
        self.resolver = resolver_summary();
    }

    fn filtered_pages(&self) -> Vec<Page> {
        let query = self.search.text.to_ascii_lowercase();
        Page::ALL
            .iter()
            .copied()
            .filter(|page| query.is_empty() || page.label().to_ascii_lowercase().contains(&query))
            .collect()
    }

    fn render(&mut self) {
        let height = self.window.canvas().height();
        let side = self.sidebar_width();
        let pages = self.filtered_pages();
        let current_page = self.page;
        let compact = side < 100;
        {
            let canvas = self.window.canvas_mut();
            canvas.clear(BG);
            canvas.fill_rect(0, 0, side as u32, height, SIDEBAR);
            canvas.vertical_line(side - 1, 0, height, DIVIDER);

            if compact {
                canvas.draw_text(23, 24, "S", ACCENT);
            } else {
                canvas.draw_text(16, 4, "Find a setting", MUTED);
            }
        }
        if !compact {
            self.search.x = 16;
            self.search.y = 20;
            self.search.w = (side - 32) as u32;
            self.search
                .draw(self.window.canvas_mut(), self.search_focused);
        }
        {
            let canvas = self.window.canvas_mut();
            let mut row = 0i32;
            for page in &pages {
                let y = 72 + row * 42;
                if *page == current_page {
                    rounded_fill(canvas, 10, y, (side - 20) as u32, 34, 8, ACCENT_SOFT);
                    canvas.fill_rect(10, y + 8, 3, 18, ACCENT);
                }
                if compact {
                    draw_page_icon(
                        canvas,
                        29,
                        y + 10,
                        *page,
                        if *page == current_page { ACCENT } else { MUTED },
                    );
                } else {
                    draw_page_icon(
                        canvas,
                        22,
                        y + 10,
                        *page,
                        if *page == current_page { ACCENT } else { MUTED },
                    );
                    canvas.draw_text(48, y + 9, page.label(), TEXT);
                }
                row += 1;
            }

            let content_x = side + 28;
            canvas.draw_text(content_x, 24, current_page.label(), TEXT);
            canvas.draw_text(content_x, 48, current_page.subtitle(), MUTED);
        }

        match self.page {
            Page::Home => self.render_home(),
            Page::Appearance => self.render_appearance(),
            Page::Desktop => self.render_desktop(),
            Page::System => self.render_system(),
            Page::Network => self.render_network(),
            Page::About => self.render_about(),
        }

        if !self.banner.is_empty() {
            let side = self.sidebar_width();
            let canvas = self.window.canvas_mut();
            let y = canvas.height() as i32 - 46;
            rounded_fill(
                canvas,
                side + 28,
                y,
                canvas.width().saturating_sub((side + 56) as u32),
                32,
                8,
                if self.banner_warning {
                    0xFFF3DA
                } else {
                    0xE8F5EA
                },
            );
            canvas.draw_text(
                side + 40,
                y + 8,
                &self.banner,
                if self.banner_warning {
                    WARNING
                } else {
                    SUCCESS
                },
            );
        }
        let _ = self.window.present();
    }

    fn content_rect(&self) -> (i32, i32, u32) {
        let side = self.sidebar_width();
        (
            side + 28,
            84,
            self.window
                .canvas()
                .width()
                .saturating_sub((side + 56) as u32),
        )
    }

    fn render_home(&mut self) {
        let (x, y, w) = self.content_rect();
        let renderer = renderer_name(self.snapshot.renderer_kind);
        let theme = active_theme_name(self.snapshot.active_theme);
        let canvas = self.window.canvas_mut();
        card(canvas, x, y, w, 92);
        canvas.draw_text(x + 18, y + 16, "AgenticOS", TEXT);
        canvas.draw_text(
            x + 18,
            y + 40,
            &format!(
                "x86_64  |  {}x{}  |  {} renderer",
                self.snapshot.display_width, self.snapshot.display_height, renderer
            ),
            MUTED,
        );
        let half = w.saturating_sub(12) / 2;
        card(canvas, x, y + 108, half, 112);
        canvas.draw_text(x + 16, y + 124, "Appearance", TEXT);
        canvas.draw_text(x + 16, y + 150, &format!("Theme: {}", theme), MUTED);
        canvas.draw_text(x + 16, y + 178, "Open Appearance >", ACCENT);
        card(canvas, x + half as i32 + 12, y + 108, half, 112);
        canvas.draw_text(x + half as i32 + 28, y + 124, "Desktop", TEXT);
        canvas.draw_text(x + half as i32 + 28, y + 150, "Wallpaper configured", MUTED);
        canvas.draw_text(x + half as i32 + 28, y + 178, "Open Desktop >", ACCENT);
        card(canvas, x, y + 236, w, 92);
        canvas.draw_text(x + 16, y + 252, "System status", TEXT);
        canvas.draw_text(
            x + 16,
            y + 278,
            &format!("Uptime: {}    Memory: {}", self.uptime, self.memory),
            MUTED,
        );
        canvas.draw_text(
            x + 16,
            y + 300,
            &format!("Network: {}", self.network),
            MUTED,
        );
    }

    fn render_appearance(&mut self) {
        let (x, y, w) = self.content_rect();
        let gap = 12u32;
        let tile_w = w.saturating_sub(gap * 2) / 3;
        let selected = self.snapshot.theme_preference;
        let available_aero =
            self.snapshot.theme_available_mask & runtime::THEME_AVAILABLE_AERO != 0;
        let canvas = self.window.canvas_mut();
        card(canvas, x, y, w, 230);
        canvas.draw_text(x + 18, y + 16, "Theme", TEXT);
        for index in 0..3u32 {
            let tx = x + 16 + index as i32 * (tile_w as i32 + gap as i32);
            let tw = tile_w.saturating_sub(10);
            let disabled = index == 2 && !available_aero;
            rounded_fill(
                canvas,
                tx,
                y + 48,
                tw,
                142,
                8,
                if selected == index { ACCENT_SOFT } else { BG },
            );
            canvas.rect(
                tx,
                y + 48,
                tw,
                142,
                if selected == index { ACCENT } else { DIVIDER },
            );
            draw_theme_preview(canvas, tx + 10, y + 60, tw.saturating_sub(20), index);
            let label = match index {
                0 => "Automatic",
                1 => "Classic",
                _ => "Aero Glass",
            };
            canvas.draw_text(
                tx + 10,
                y + 158,
                label,
                if disabled { 0xA0A5AD } else { TEXT },
            );
        }
        if !available_aero {
            canvas.draw_text(
                x + 18,
                y + 202,
                "Aero requires the retained compositor.",
                WARNING,
            );
        } else {
            canvas.draw_text(
                x + 18,
                y + 202,
                "Changes apply immediately to open applications.",
                MUTED,
            );
        }
        card(canvas, x, y + 246, w, 88);
        canvas.draw_text(x + 18, y + 262, "Current appearance", TEXT);
        canvas.draw_text(
            x + 18,
            y + 288,
            &format!(
                "{} (requested: {})",
                active_theme_name(self.snapshot.active_theme),
                requested_theme_name(self.snapshot.theme_preference)
            ),
            MUTED,
        );
        let saved = self.snapshot.persistence_flags & runtime::SYSTEM_PERSISTENCE_AVAILABLE != 0;
        let boot_override = self.snapshot.boot_flags & runtime::SYSTEM_BOOT_THEME_OVERRIDE != 0;
        canvas.draw_text(
            x + 18,
            y + 310,
            if boot_override {
                "Boot theme override active for this session"
            } else if saved {
                "Saved to /data"
            } else {
                "Session only"
            },
            if saved && !boot_override {
                SUCCESS
            } else {
                WARNING
            },
        );
    }

    fn render_desktop(&mut self) {
        let (x, y, w) = self.content_rect();
        let path = shorten(&self.wallpaper_path, 66);
        let canvas = self.window.canvas_mut();
        card(canvas, x, y, w, 212);
        canvas.draw_text(x + 18, y + 16, "Wallpaper", TEXT);
        rounded_fill(canvas, x + 18, y + 48, 150, 92, 8, 0x234A73);
        canvas.fill_rect(x + 28, y + 104, 130, 24, 0x15344F);
        canvas.draw_text(x + 186, y + 52, "Current image", MUTED);
        canvas.draw_text(x + 186, y + 78, &path, TEXT);
        modern_button(canvas, x + 186, y + 112, 136, "Choose image...", true);
        modern_button(canvas, x + 334, y + 112, 136, "Restore default", false);
        canvas.draw_text(
            x + 18,
            y + 166,
            "BMP images up to 16 MiB. Images stretch to fill.",
            MUTED,
        );
        if self.snapshot.wallpaper_state == 2 {
            canvas.draw_text(
                x + 18,
                y + 188,
                "Saved image unavailable; showing the bundled default.",
                WARNING,
            );
        }
    }

    fn render_system(&mut self) {
        let (x, y, w) = self.content_rect();
        let rows = [
            (
                "Display",
                format!(
                    "{} x {}",
                    self.snapshot.display_width, self.snapshot.display_height
                ),
            ),
            (
                "Renderer",
                renderer_name(self.snapshot.renderer_kind).to_string(),
            ),
            ("Uptime", self.uptime.clone()),
            ("Memory", self.memory.clone()),
            ("Architecture", String::from("x86_64")),
        ];
        draw_rows(
            self.window.canvas_mut(),
            x,
            y,
            w,
            "System information",
            &rows,
        );
    }

    fn render_network(&mut self) {
        let (x, y, w) = self.content_rect();
        let rows = [
            ("Interface activity", self.network.clone()),
            ("Resolver", self.resolver.clone()),
            (
                "Configuration",
                String::from("DHCP-managed (read-only here)"),
            ),
        ];
        draw_rows(self.window.canvas_mut(), x, y, w, "Network activity", &rows);
    }

    fn render_about(&mut self) {
        let (x, y, w) = self.content_rect();
        let canvas = self.window.canvas_mut();
        card(canvas, x, y, w, 230);
        rounded_fill(canvas, x + 20, y + 20, 54, 54, 12, ACCENT);
        canvas.draw_text(x + 39, y + 38, "A", 0xFFFFFF);
        canvas.draw_text(x + 92, y + 22, "AgenticOS", TEXT);
        canvas.draw_text(x + 92, y + 48, "Development build for x86_64", MUTED);
        canvas.horizontal_line(x + 20, y + 92, w.saturating_sub(40), DIVIDER);
        canvas.draw_text(
            x + 20,
            y + 116,
            "A bare-metal operating system for agent-based computing.",
            TEXT,
        );
        canvas.draw_text(
            x + 20,
            y + 146,
            "Settings and applications run as ordinary ring-3 processes.",
            MUTED,
        );
        canvas.draw_text(x + 20, y + 184, "github.com/Agenticos", ACCENT);
    }

    fn select_page(&mut self, page: Page) {
        self.page = page;
        self.search_focused = false;
        self.banner.clear();
        self.refresh();
    }

    fn apply_theme(&mut self, theme: u32) {
        match runtime::system_control_set_theme(theme) {
            Ok(result) => {
                self.banner = match result {
                    ApplyResult::Persisted => String::from("Appearance saved"),
                    ApplyResult::SessionOnly => {
                        String::from("Applied for this session; settings were not saved")
                    }
                };
                self.banner_warning = result == ApplyResult::SessionOnly;
            }
            Err(-95) => {
                self.banner = String::from("Aero requires the retained compositor");
                self.banner_warning = true;
            }
            Err(_) => {
                self.banner = String::from("Could not change the theme");
                self.banner_warning = true;
            }
        }
        self.refresh();
    }

    fn choose_wallpaper(&mut self) {
        match FileDialog::open("/host") {
            Ok(dialog) => {
                self.modal = Some(ActiveModal {
                    modal: Modal::File(dialog),
                    purpose: ModalPurpose::Wallpaper,
                });
            }
            Err(_) => self.show_error("Could not open the wallpaper picker."),
        }
    }

    fn reset_wallpaper(&mut self) {
        match runtime::system_control_reset_wallpaper() {
            Ok(result) => {
                self.banner = if result == ApplyResult::Persisted {
                    String::from("Default wallpaper restored")
                } else {
                    String::from("Default restored for this session")
                };
                self.banner_warning = result == ApplyResult::SessionOnly;
                self.refresh();
            }
            Err(_) => self.show_error("Could not restore the default wallpaper."),
        }
    }

    fn apply_wallpaper(&mut self, path: String) {
        if !path.to_ascii_lowercase().ends_with(".bmp") {
            self.show_error("Choose a BMP image file.");
            return;
        }
        match runtime::system_control_set_wallpaper(&path) {
            Ok(result) => {
                self.banner = if result == ApplyResult::Persisted {
                    String::from("Wallpaper saved")
                } else {
                    String::from("Wallpaper applied for this session")
                };
                self.banner_warning = result == ApplyResult::SessionOnly;
                self.refresh();
            }
            Err(_) => self.show_error("The image could not be read as a supported BMP."),
        }
    }

    fn show_error(&mut self, message: &str) {
        if let Ok(dialog) = MessageBox::error(message) {
            self.modal = Some(ActiveModal {
                modal: Modal::Message(dialog),
                purpose: ModalPurpose::Dismiss,
            });
        } else {
            self.banner = message.to_string();
            self.banner_warning = true;
        }
    }

    fn handle_global_event(&mut self, event: &GuiEvent) -> bool {
        if event.kind != gui::GUI_EVENT_THEME_CHANGED
            && event.kind != gui::GUI_EVENT_SETTINGS_CHANGED
        {
            return false;
        }
        self.refresh();
        if let Some(active) = self.modal.as_mut() {
            active.modal.refresh_theme();
        }
        self.render();
        true
    }

    fn handle_modal(&mut self, event: &GuiEvent) {
        let Some(active) = self.modal.as_mut() else {
            return;
        };
        if event.window != active.modal.window_handle() {
            return;
        }
        let status = active.modal.handle_event(event);
        if let DialogStatus::Done(outcome) = status {
            let active = self.modal.take().unwrap();
            if let (ModalPurpose::Wallpaper, Some(ModalOutcome::Path(path))) =
                (active.purpose, outcome)
            {
                self.apply_wallpaper(path);
            }
            self.render();
        }
    }

    fn handle_main(&mut self, event: GuiEvent) -> bool {
        match event.kind {
            runtime::GUI_EVENT_CLOSE => return true,
            runtime::GUI_EVENT_RESIZE => {
                self.window
                    .resize(event.payload[0].max(720), event.payload[1].max(480));
            }
            runtime::GUI_EVENT_KEY if event.payload[3] != 0 && self.modal.is_none() => {
                self.handle_key(event.payload);
            }
            runtime::GUI_EVENT_MOUSE
                if event.payload[3] == runtime::GUI_MOUSE_DOWN && self.modal.is_none() =>
            {
                self.handle_click(event.payload[0] as i32, event.payload[1] as i32);
            }
            _ => return false,
        }
        self.render();
        false
    }

    fn handle_key(&mut self, payload: [u32; 6]) {
        let key = payload[0];
        let ch = char::from_u32(payload[1]).unwrap_or('\0');
        let ctrl = payload[2] & 2 != 0;
        if ctrl && matches!(ch, 'f' | 'F') {
            self.search_focused = self.sidebar_width() >= 100;
            return;
        }
        if ctrl && key == runtime::KEY_TAB {
            let index = Page::ALL
                .iter()
                .position(|page| *page == self.page)
                .unwrap_or(0);
            self.select_page(Page::ALL[(index + 1) % Page::ALL.len()]);
            return;
        }
        if self.search_focused {
            if key == runtime::KEY_ESCAPE {
                self.search.set_text("");
                self.search_focused = false;
            } else if key == runtime::KEY_ENTER {
                if let Some(page) = self.filtered_pages().first().copied() {
                    self.select_page(page);
                }
            } else {
                self.search.key(key, ch);
            }
            return;
        }
        if key == runtime::KEY_UP || key == runtime::KEY_DOWN {
            let index = Page::ALL
                .iter()
                .position(|page| *page == self.page)
                .unwrap_or(0);
            let next = if key == runtime::KEY_UP {
                index.saturating_sub(1)
            } else {
                (index + 1).min(Page::ALL.len() - 1)
            };
            self.select_page(Page::ALL[next]);
        }
    }

    fn handle_click(&mut self, x: i32, y: i32) {
        let side = self.sidebar_width();
        if side >= 100 && self.search.hit(x, y) {
            self.search_focused = true;
            self.search.click(x);
            return;
        }
        if x < side {
            let pages = self.filtered_pages();
            let row = (y - 72) / 42;
            if row >= 0 {
                if let Some(page) = pages.get(row as usize).copied() {
                    self.select_page(page);
                }
            }
            return;
        }
        let (cx, cy, cw) = self.content_rect();
        match self.page {
            Page::Home => {
                let half = cw.saturating_sub(12) / 2;
                if point_in(x, y, cx, cy + 108, half, 112) {
                    self.select_page(Page::Appearance);
                } else if point_in(x, y, cx + half as i32 + 12, cy + 108, half, 112) {
                    self.select_page(Page::Desktop);
                }
            }
            Page::Appearance => {
                let gap = 12u32;
                let tile = cw.saturating_sub(gap * 2) / 3;
                for index in 0..3u32 {
                    let tx = cx + 16 + index as i32 * (tile as i32 + gap as i32);
                    if point_in(x, y, tx, cy + 48, tile.saturating_sub(10), 142) {
                        if index != 2
                            || self.snapshot.theme_available_mask & runtime::THEME_AVAILABLE_AERO
                                != 0
                        {
                            self.apply_theme(index);
                        }
                    }
                }
            }
            Page::Desktop => {
                if point_in(x, y, cx + 186, cy + 112, 136, 30) {
                    self.choose_wallpaper();
                } else if point_in(x, y, cx + 334, cy + 112, 136, 30) {
                    self.reset_wallpaper();
                }
            }
            _ => {}
        }
    }

    fn run(&mut self) -> i64 {
        self.render();
        loop {
            let event = match gui::next_event() {
                Ok(event) => event,
                Err(error) => return error,
            };
            if self.handle_global_event(&event) {
                continue;
            }
            if event.window == self.window.handle() {
                if self.handle_main(event) {
                    return 0;
                }
            } else {
                self.handle_modal(&event);
            }
        }
    }
}

fn rounded_fill(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, radius: u32, color: u32) {
    for row in 0..h {
        let edge = row.min(h.saturating_sub(1).saturating_sub(row));
        let inset = if edge >= radius {
            0
        } else {
            (radius - edge) / 2
        };
        canvas.fill_rect(
            x + inset as i32,
            y + row as i32,
            w.saturating_sub(inset * 2),
            1,
            color,
        );
    }
}

fn card(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
    rounded_fill(canvas, x, y, w, h, 10, CARD);
    canvas.horizontal_line(x + 6, y, w.saturating_sub(12), DIVIDER);
}

fn modern_button(canvas: &mut Canvas, x: i32, y: i32, w: u32, label: &str, primary: bool) {
    rounded_fill(
        canvas,
        x,
        y,
        w,
        30,
        8,
        if primary { ACCENT } else { ACCENT_SOFT },
    );
    canvas.draw_text(
        x + 12,
        y + 8,
        label,
        if primary { 0xFFFFFF } else { ACCENT },
    );
}

fn draw_page_icon(canvas: &mut Canvas, x: i32, y: i32, page: Page, color: u32) {
    match page {
        Page::Home => {
            canvas.fill_rect(x, y + 5, 14, 11, color);
            canvas.fill_rect(x + 3, y + 1, 8, 5, color);
        }
        Page::Appearance => {
            canvas.rect(x, y + 2, 14, 14, color);
            canvas.fill_rect(x + 4, y + 6, 6, 6, color);
        }
        Page::Desktop => {
            canvas.rect(x, y + 2, 15, 11, color);
            canvas.horizontal_line(x + 4, y + 16, 8, color);
        }
        Page::System => {
            canvas.rect(x + 2, y + 2, 11, 14, color);
            canvas.horizontal_line(x + 5, y + 6, 5, color);
        }
        Page::Network => {
            canvas.fill_rect(x + 6, y + 12, 3, 3, color);
            canvas.horizontal_line(x + 3, y + 8, 9, color);
            canvas.horizontal_line(x, y + 4, 15, color);
        }
        Page::About => {
            canvas.rect(x + 2, y + 2, 12, 14, color);
            canvas.fill_rect(x + 7, y + 5, 2, 2, color);
            canvas.fill_rect(x + 7, y + 9, 2, 5, color);
        }
    }
}

fn draw_theme_preview(canvas: &mut Canvas, x: i32, y: i32, w: u32, kind: u32) {
    let aero = kind == 2 || kind == 0;
    let title = if aero { 0x8DC4EA } else { 0x000080 };
    let face = if aero { 0xF0F0F0 } else { 0xC0C0C0 };
    rounded_fill(canvas, x, y, w, 78, if aero { 7 } else { 0 }, face);
    canvas.fill_rect(x + 2, y + 2, w.saturating_sub(4), 18, title);
    canvas.fill_rect(x + 10, y + 32, w.saturating_sub(20), 10, 0xFFFFFF);
    canvas.rect(x + 10, y + 32, w.saturating_sub(20), 10, 0x8A8A8A);
    canvas.fill_rect(
        x + 10,
        y + 52,
        w / 2,
        16,
        if aero { 0xD8ECF8 } else { 0xC0C0C0 },
    );
    canvas.rect(
        x + 10,
        y + 52,
        w / 2,
        16,
        if aero { 0x3C7FB1 } else { 0x000000 },
    );
}

fn draw_rows(canvas: &mut Canvas, x: i32, y: i32, w: u32, title: &str, rows: &[(&str, String)]) {
    let height = 52 + rows.len() as u32 * 48;
    card(canvas, x, y, w, height);
    canvas.draw_text(x + 18, y + 16, title, TEXT);
    for (index, (label, value)) in rows.iter().enumerate() {
        let ry = y + 52 + index as i32 * 48;
        canvas.horizontal_line(x + 18, ry - 8, w.saturating_sub(36), DIVIDER);
        canvas.draw_text(x + 18, ry + 5, label, TEXT);
        canvas.draw_text(x + (w as i32 / 2), ry + 5, value, MUTED);
    }
}

fn point_in(px: i32, py: i32, x: i32, y: i32, w: u32, h: u32) -> bool {
    px >= x && py >= y && px < x + w as i32 && py < y + h as i32
}

fn renderer_name(value: u32) -> &'static str {
    match value {
        1 => "retained",
        2 => "gpu",
        _ => "legacy",
    }
}

fn active_theme_name(value: u32) -> &'static str {
    if value == runtime::THEME_AERO {
        "Aero Glass"
    } else {
        "Classic"
    }
}

fn requested_theme_name(value: u32) -> &'static str {
    match value {
        runtime::THEME_CLASSIC => "Classic",
        runtime::THEME_AERO => "Aero Glass",
        _ => "Automatic",
    }
}

fn shorten(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let tail: String = value
        .chars()
        .rev()
        .take(max.saturating_sub(3))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{}", tail)
}

fn read_text(path: &[u8]) -> String {
    let fd = runtime::openat(runtime::AT_FDCWD, path, runtime::O_RDONLY, 0);
    if fd < 0 {
        return String::new();
    }
    let fd = fd as i32;
    let mut out = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let count = runtime::read(fd, &mut chunk);
        if count <= 0 {
            break;
        }
        out.extend_from_slice(&chunk[..count as usize]);
        if out.len() >= 16 * 1024 {
            break;
        }
    }
    let _ = runtime::close(fd);
    String::from_utf8(out).unwrap_or_default()
}

fn uptime_summary() -> String {
    let text = read_text(b"/proc/uptime\0");
    let seconds = text
        .split_whitespace()
        .next()
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u64>().ok());
    match seconds {
        Some(value) => format!("{}h {}m", value / 3600, (value / 60) % 60),
        None => String::from("Unavailable"),
    }
}

fn keyed_kb(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find(|line| line.starts_with(key))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
}

fn memory_summary() -> String {
    let text = read_text(b"/proc/meminfo\0");
    match (keyed_kb(&text, "MemTotal:"), keyed_kb(&text, "MemFree:")) {
        (Some(total), Some(free)) => format!(
            "{} / {} MiB used",
            (total.saturating_sub(free)) / 1024,
            total / 1024
        ),
        _ => String::from("Unavailable"),
    }
}

fn network_summary() -> String {
    let text = read_text(b"/proc/net/dev\0");
    for line in text.lines() {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<&str> = values.split_whitespace().collect();
        if fields.len() >= 9 {
            let rx = fields[0].parse::<u64>().unwrap_or(0);
            let tx = fields[8].parse::<u64>().unwrap_or(0);
            return format!(
                "{}: RX {} KiB, TX {} KiB",
                name.trim(),
                rx / 1024,
                tx / 1024
            );
        }
    }
    String::from("Unavailable")
}

fn resolver_summary() -> String {
    let text = read_text(b"/etc/resolv.conf\0");
    let servers: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("nameserver "))
        .collect();
    if servers.is_empty() {
        String::from("Unavailable")
    } else {
        servers.join(", ")
    }
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let mut app = match ControlCenter::new() {
        Ok(app) => app,
        Err(_) => runtime::exit(1),
    };
    let code = app.run();
    app.window.destroy();
    runtime::exit(code)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}
