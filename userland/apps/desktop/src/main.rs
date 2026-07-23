#![no_std]
#![no_main]

//! `DESKTOP.ELF` — the ring-3 desktop shell.
//!
//! This is the userland replacement for the in-kernel `guishell` process. It
//! owns the taskbar panel, Start menu, notification-tray clock, Run prompt, and
//! the application launcher. It talks to the kernel compositor purely through
//! the desktop-shell protocol syscalls (`gui_shell_register`,
//! `gui_shell_list_windows`, `gui_shell_window_action`,
//! `gui_shell_spawn_terminal`) plus the `GUI_WINDOW_PANEL` /
//! `GUI_WINDOW_UNDECORATED` chrome flags, and launches applications with plain
//! POSIX `fork`+`execve`. The kernel keeps the compositor, the desktop-root
//! wallpaper, and the terminal/PTY service.
//!
//! The Start menu mirrors the old kernel `guishell`: an "AgenticOS" banner, a
//! two-column Programs fly-out, per-item icons, and a Win95-style layout.
//!
//! See `docs/plans/2026-07-21-001-feat-ring3-desktop-shell-plan.md`.

extern crate alloc;

mod icons;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use gui::theme::{self, ButtonState, Palette};
use gui::{
    Canvas, Window, WindowOptions, FONT_CELL_WIDTH, FONT_LINE_HEIGHT, GUI_EVENT_CLOSE,
    GUI_EVENT_FOCUS_CHANGE, GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_THEME_CHANGED, GUI_MOUSE_DOWN,
    GUI_MOUSE_MOVE,
};
use icons::Icon;

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

const TASKBAR_H: u32 = 30;
const START_BTN_X: i32 = 4;
const START_BTN_W: i32 = 58;
const BTN_Y: i32 = 3;
const BTN_H: i32 = TASKBAR_H as i32 - 6;
const TASK_BTN_MAX_W: i32 = 150;
const TASK_BTN_GAP: i32 = 4;

const CLOCK_REALTIME: i32 = 0;
const MAX_TASK_WINDOWS: usize = 32;
const ZSH_PATH: &str = "/host/ZSH.ELF";
const ICON_ACCENT: u32 = 0x3C8CF0;

// Start-menu geometry, mirroring the kernel `start_menu.rs`.
const BANNER_W: i32 = 28;
const ROOT_W: i32 = 196;
const ROOT_ROW_H: i32 = 32;
const SEP_H: i32 = 8;
const PROG_W: i32 = 144;
const PROG_ROW_H: i32 = 24;
const MENU_BORDER: i32 = 2;
const ROOT_ICON: i32 = 22;
const PROG_ICON: i32 = 16;

// Run dialog (decorated) client size.
const RUN_W: u32 = 420;
const RUN_H: u32 = 120;

// ---------------------------------------------------------------------------
// Menu model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Terminal,
    FileManager,
    WebBrowser,
    Notepad,
    Painting,
    Calc,
    GlGame,
    TaskManager,
    Settings,
    Run,
    ShutDown,
}

enum Row {
    Action(&'static str, MenuAction, Icon),
    Submenu(&'static str, Icon),
    Disabled(&'static str, Icon),
    Separator,
}

const ROOT: &[Row] = &[
    Row::Submenu("Programs", Icon::Programs),
    Row::Disabled("Documents", Icon::Documents),
    Row::Action("Settings", MenuAction::Settings, Icon::Settings),
    Row::Action("Run...", MenuAction::Run, Icon::Run),
    Row::Separator,
    Row::Action("Shut Down...", MenuAction::ShutDown, Icon::ShutDown),
];

const PROGRAMS: &[Row] = &[
    Row::Action("File Manager", MenuAction::FileManager, Icon::FileManager),
    Row::Action("Web Browser", MenuAction::WebBrowser, Icon::WebBrowser),
    Row::Action("Terminal", MenuAction::Terminal, Icon::Terminal),
    Row::Action("Notepad", MenuAction::Notepad, Icon::Notepad),
    Row::Action("Painting", MenuAction::Painting, Icon::Painting),
    Row::Action("Calc", MenuAction::Calc, Icon::Calc),
    Row::Action("GL Arena", MenuAction::GlGame, Icon::GlGame),
    Row::Action("Task Manager", MenuAction::TaskManager, Icon::TaskManager),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Root(usize),
    Program(usize),
}

enum MenuOutcome {
    None,
    Close,
    Activate(MenuAction),
}

fn column_height(items: &[Row], row_h: i32) -> i32 {
    let mut height = MENU_BORDER * 2;
    for item in items {
        height += if matches!(item, Row::Separator) {
            SEP_H
        } else {
            row_h
        };
    }
    height
}

fn root_height() -> i32 {
    column_height(ROOT, ROOT_ROW_H)
}

fn programs_height() -> i32 {
    column_height(PROGRAMS, PROG_ROW_H)
}

fn item_at(target: Target) -> &'static Row {
    match target {
        Target::Root(index) => &ROOT[index],
        Target::Program(index) => &PROGRAMS[index],
    }
}

fn target_enabled(target: Target) -> bool {
    matches!(item_at(target), Row::Action(..) | Row::Submenu(..))
}

/// Find the row index at local `y` in a column, skipping separators.
fn row_at(items: &[Row], y: i32, row_h: i32) -> Option<usize> {
    if y < MENU_BORDER {
        return None;
    }
    let mut top = MENU_BORDER;
    for (index, item) in items.iter().enumerate() {
        let height = if matches!(item, Row::Separator) {
            SEP_H
        } else {
            row_h
        };
        if y >= top && y < top + height {
            return (!matches!(item, Row::Separator)).then_some(index);
        }
        top += height;
    }
    None
}

// ---------------------------------------------------------------------------
// Start menu
// ---------------------------------------------------------------------------

/// The Start menu is two independent, side-by-side windows: the fixed-height
/// root (`root`) and the Programs fly-out (`flyout`), which has its own height.
/// The fly-out is created without focus so it does not dismiss the root popup;
/// both bottom-align to the taskbar so opening Programs never moves the root.
struct Menu {
    root: Window,
    flyout: Option<Window>,
    origin_x: i32,
    screen_h: i32,
    root_hover: Option<usize>,
    prog_hover: Option<usize>,
}

impl Menu {
    fn open(origin_x: i32, screen_h: i32) -> Option<Menu> {
        let height = root_height();
        let y = (screen_h - TASKBAR_H as i32 - height).max(0);
        let root = Window::new_undecorated(ROOT_W as u32, height as u32, origin_x, y).ok()?;
        let mut menu = Menu {
            root,
            flyout: None,
            origin_x,
            screen_h,
            root_hover: None,
            prog_hover: None,
        };
        menu.draw_root();
        Some(menu)
    }

    fn owns(&self, window: u32) -> bool {
        self.root.handle() == window
            || self.flyout.as_ref().is_some_and(|f| f.handle() == window)
    }

    fn ensure_flyout(&mut self) {
        if self.flyout.is_some() {
            return;
        }
        let height = programs_height();
        let x = self.origin_x + ROOT_W - MENU_BORDER;
        let y = (self.screen_h - TASKBAR_H as i32 - height).max(0);
        if let Ok(window) =
            Window::new_undecorated_unfocused(PROG_W as u32, height as u32, x, y)
        {
            self.flyout = Some(window);
            self.prog_hover = None;
            self.draw_flyout();
        }
    }

    fn close_flyout(&mut self) {
        self.flyout = None; // dropped → destroyed
        self.prog_hover = None;
    }

    fn redraw(&mut self) {
        self.draw_root();
        if self.flyout.is_some() {
            self.draw_flyout();
        }
    }

    fn draw_root(&mut self) {
        // The parent row stays highlighted while its fly-out is open.
        let hover = if self.flyout.is_some() {
            Some(Target::Root(0))
        } else {
            self.root_hover.map(Target::Root)
        };
        let h = root_height();
        let palette = theme::palette();
        let canvas = self.root.canvas_mut();
        theme::draw_menu_surface(canvas, 0, 0, ROOT_W as u32, h as u32);
        canvas.fill_rect(2, 2, (BANNER_W - 2) as u32, (h - 4) as u32, palette.selection_bg);
        canvas.draw_vertical_banner(2, 2, h - 4, "AgenticOS", palette.selection_text);
        paint_column(canvas, ROOT, true, 0, ROOT_W, hover, palette);
        let _ = self.root.present();
    }

    fn draw_flyout(&mut self) {
        let hover = self.prog_hover.map(Target::Program);
        let h = programs_height();
        let palette = theme::palette();
        let Some(flyout) = self.flyout.as_mut() else {
            return;
        };
        let canvas = flyout.canvas_mut();
        theme::draw_menu_surface(canvas, 0, 0, PROG_W as u32, h as u32);
        paint_column(canvas, PROGRAMS, false, 0, PROG_W, hover, palette);
        let _ = flyout.present();
    }

    fn on_root_move(&mut self, x: i32, y: i32) {
        let target = if x >= BANNER_W && x < ROOT_W {
            row_at(ROOT, y, ROOT_ROW_H)
        } else {
            None
        };
        match target {
            Some(0) => {
                self.root_hover = Some(0);
                self.ensure_flyout();
            }
            Some(index) if target_enabled(Target::Root(index)) => {
                self.root_hover = Some(index);
                self.close_flyout();
            }
            Some(_) => {
                self.root_hover = None;
                self.close_flyout();
            }
            None => self.root_hover = None,
        }
        self.draw_root();
    }

    fn on_root_click(&mut self, x: i32, y: i32) -> MenuOutcome {
        let target = if x >= BANNER_W && x < ROOT_W {
            row_at(ROOT, y, ROOT_ROW_H).map(Target::Root)
        } else {
            None
        };
        match target {
            Some(t) => match item_at(t) {
                Row::Action(_, action, _) => MenuOutcome::Activate(*action),
                Row::Submenu(..) => {
                    self.root_hover = Some(0);
                    self.ensure_flyout();
                    self.draw_root();
                    MenuOutcome::None
                }
                _ => MenuOutcome::None,
            },
            None => MenuOutcome::None,
        }
    }

    fn on_flyout_move(&mut self, x: i32, y: i32) {
        let target = if x >= 0 && x < PROG_W {
            row_at(PROGRAMS, y, PROG_ROW_H)
        } else {
            None
        };
        let hover = target.filter(|index| target_enabled(Target::Program(*index)));
        if hover != self.prog_hover {
            self.prog_hover = hover;
            self.draw_flyout();
        }
    }

    fn on_flyout_click(&mut self, x: i32, y: i32) -> MenuOutcome {
        let target = if x >= 0 && x < PROG_W {
            row_at(PROGRAMS, y, PROG_ROW_H).map(Target::Program)
        } else {
            None
        };
        match target {
            Some(t) => match item_at(t) {
                Row::Action(_, action, _) => MenuOutcome::Activate(*action),
                _ => MenuOutcome::None,
            },
            None => MenuOutcome::None,
        }
    }
}

fn paint_column(
    canvas: &mut Canvas,
    items: &[Row],
    root: bool,
    panel_x: i32,
    panel_w: i32,
    hover: Option<Target>,
    palette: &Palette,
) {
    let row_h = if root { ROOT_ROW_H } else { PROG_ROW_H };
    let icon_size = if root { ROOT_ICON } else { PROG_ICON };
    let icon_left = if root {
        panel_x + BANNER_W + 5
    } else {
        panel_x + 6
    };
    let text_left = icon_left + icon_size + 6;
    let content_left = if root {
        panel_x + BANNER_W + 2
    } else {
        panel_x + 2
    };
    let content_right = panel_x + panel_w - 3;

    let mut item_y = MENU_BORDER;
    for (index, item) in items.iter().enumerate() {
        if matches!(item, Row::Separator) {
            let line_y = item_y + SEP_H / 2 - 1;
            canvas.horizontal_line(
                content_left + 4,
                line_y,
                (content_right - content_left - 7).max(0) as u32,
                palette.border,
            );
            item_y += SEP_H;
            continue;
        }

        let (label, icon, disabled, submenu) = match item {
            Row::Action(label, _, icon) => (*label, *icon, false, false),
            Row::Submenu(label, icon) => (*label, *icon, false, true),
            Row::Disabled(label, icon) => (*label, *icon, true, false),
            Row::Separator => unreachable!(),
        };
        let target = if root {
            Target::Root(index)
        } else {
            Target::Program(index)
        };
        let highlighted = hover == Some(target) && !disabled;
        if highlighted {
            theme::draw_selection(
                canvas,
                content_left,
                item_y,
                (content_right - content_left + 1).max(0) as u32,
                row_h as u32,
            );
        }

        let icon_top = item_y + (row_h - icon_size) / 2;
        let (icon_fg, icon_accent) = if disabled {
            (palette.disabled_text, palette.disabled_text)
        } else if highlighted {
            (palette.selection_text, palette.selection_text)
        } else {
            (palette.text, ICON_ACCENT)
        };
        icons::draw(canvas, icon, icon_left, icon_top, icon_size, icon_fg, icon_accent);

        let text_y = item_y + (row_h - FONT_LINE_HEIGHT) / 2;
        let color = if disabled {
            palette.disabled_text
        } else if highlighted {
            palette.selection_text
        } else {
            palette.text
        };
        canvas.draw_text(text_left, text_y, label, color);
        if submenu {
            draw_arrow(canvas, content_right - 8, item_y + row_h / 2, color);
        }
        item_y += row_h;
    }
}

fn draw_arrow(canvas: &mut Canvas, x: i32, y: i32, color: u32) {
    for column in 0..4i32 {
        canvas.vertical_line(x + column, y - column, (2 * column + 1) as u32, color);
    }
}

// ---------------------------------------------------------------------------
// Run dialog (decorated window)
// ---------------------------------------------------------------------------

struct RunDialog {
    window: Window,
    input: String,
}

fn run_buttons() -> ((i32, i32, i32, i32), (i32, i32, i32, i32)) {
    let ok = (RUN_W as i32 - 178, 88, 80, 24);
    let cancel = (RUN_W as i32 - 92, 88, 80, 24);
    (ok, cancel)
}

impl RunDialog {
    fn render(&mut self) {
        let input = self.input.clone();
        let palette = theme::palette();
        let (ok, cancel) = run_buttons();
        let canvas = self.window.canvas_mut();
        let (w, h) = (canvas.width(), canvas.height());
        canvas.fill_rect(0, 0, w, h, palette.content_bg);
        canvas.draw_text(14, 12, "Type the name of a program or command,", palette.text);
        canvas.draw_text(14, 30, "and AgenticOS will open it for you.", palette.text);
        theme::draw_field(canvas, 14, 52, w - 28, 24, true);
        let mut line = input.clone();
        line.push('_');
        canvas.draw_text(20, 58, &line, palette.field_text);
        draw_labeled_button(canvas, ok, "OK", ButtonState::Hot);
        draw_labeled_button(canvas, cancel, "Cancel", ButtonState::Normal);
        let _ = self.window.present();
    }
}

fn draw_labeled_button(canvas: &mut Canvas, rect: (i32, i32, i32, i32), label: &str, state: ButtonState) {
    let (x, y, w, h) = rect;
    theme::draw_button(canvas, x, y, w as u32, h as u32, state);
    let text_w = label.chars().count() as i32 * FONT_CELL_WIDTH;
    let text_x = x + (w - text_w) / 2;
    let text_y = y + (h - FONT_LINE_HEIGHT) / 2 + theme::pressed_label_shift(state);
    canvas.draw_text(text_x, text_y, label, theme::button_text(state));
}

fn hit_rect(rect: (i32, i32, i32, i32), px: i32, py: i32) -> bool {
    let (x, y, w, h) = rect;
    px >= x && px < x + w && py >= y && py < y + h
}

// ---------------------------------------------------------------------------
// A tracked top-level window (taskbar button)
// ---------------------------------------------------------------------------

struct TaskButton {
    frame_id: u64,
    title: String,
    state: u8,
    x: i32,
    w: i32,
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

struct Shell {
    panel: Window,
    screen_w: u32,
    screen_h: u32,
    envp: Vec<*const u8>,
    buttons: Vec<TaskButton>,
    menu: Option<Menu>,
    run: Option<RunDialog>,
    children: Vec<i32>,
    clock: String,
    last_minute: i64,
    dirty: bool,
    iter: u64,
    menu_closed_iter: u64,
}

impl Shell {
    fn new(envp: Vec<*const u8>) -> Result<Self, i64> {
        let (screen_w, screen_h) = match runtime::system_control_snapshot() {
            Ok(snapshot) if snapshot.display_width > 0 => {
                (snapshot.display_width, snapshot.display_height)
            }
            _ => (1024, 768),
        };
        let panel = Window::new_panel(screen_w, TASKBAR_H)?;
        Ok(Self {
            panel,
            screen_w,
            screen_h,
            envp,
            buttons: Vec::new(),
            menu: None,
            run: None,
            children: Vec::new(),
            clock: String::new(),
            last_minute: -1,
            dirty: true,
            iter: 0,
            menu_closed_iter: 0,
        })
    }

    fn run_loop(&mut self) -> i64 {
        self.refresh_windows();
        self.update_clock();
        self.render_panel();
        loop {
            self.iter = self.iter.wrapping_add(1);
            loop {
                match gui::try_next_event() {
                    Ok(Some(event)) => self.handle_event(event),
                    Ok(None) => break,
                    Err(error) => return error,
                }
            }
            self.reap_children();
            if self.update_clock() {
                self.dirty = true;
            }
            if self.refresh_windows() {
                self.dirty = true;
            }
            if self.dirty {
                self.render_panel();
                self.dirty = false;
            }
            let request = runtime::Timespec {
                tv_sec: 0,
                tv_nsec: 40_000_000,
            };
            runtime::nanosleep(&request, None);
        }
    }

    // -- events ------------------------------------------------------------

    fn handle_event(&mut self, event: gui::GuiEvent) {
        if event.kind == GUI_EVENT_THEME_CHANGED || event.kind == gui::GUI_EVENT_SETTINGS_CHANGED {
            self.dirty = true;
            if let Some(menu) = self.menu.as_mut() {
                menu.redraw();
            }
            if let Some(run) = self.run.as_mut() {
                run.render();
            }
            return;
        }
        let window = event.window;
        if window == self.panel.handle() {
            self.handle_panel_event(event);
        } else if self.menu.as_ref().is_some_and(|m| m.owns(window)) {
            self.handle_menu_event(event);
        } else if self.run.as_ref().is_some_and(|r| r.window.handle() == window) {
            self.handle_run_event(event);
        }
    }

    fn handle_panel_event(&mut self, event: gui::GuiEvent) {
        if event.kind != GUI_EVENT_MOUSE || event.payload[3] != GUI_MOUSE_DOWN {
            return;
        }
        let x = event.payload[0] as i32;
        if x >= START_BTN_X && x < START_BTN_X + START_BTN_W {
            if self.menu.is_some() {
                self.close_menu();
            } else if self.iter.saturating_sub(self.menu_closed_iter) > 1 {
                self.open_menu();
            }
            self.dirty = true;
            return;
        }
        let clicked = self
            .buttons
            .iter()
            .find(|button| x >= button.x && x < button.x + button.w)
            .map(|button| button.frame_id);
        if let Some(frame_id) = clicked {
            runtime::gui_shell_window_action(frame_id, runtime::SHELL_WINDOW_ACTIVATE);
            self.dirty = true;
        }
    }

    fn handle_menu_event(&mut self, event: gui::GuiEvent) {
        let outcome = {
            let window = event.window;
            let Some(menu) = self.menu.as_mut() else {
                return;
            };
            let x = event.payload[0] as i32;
            let y = event.payload[1] as i32;
            let is_root = menu.root.handle() == window;
            match event.kind {
                // Only the focused root popup dismisses on focus loss; the
                // fly-out is unfocused and never emits it.
                GUI_EVENT_FOCUS_CHANGE if is_root && event.payload[0] == 0 => MenuOutcome::Close,
                GUI_EVENT_CLOSE => MenuOutcome::Close,
                GUI_EVENT_MOUSE if event.payload[3] == GUI_MOUSE_MOVE => {
                    if is_root {
                        menu.on_root_move(x, y);
                    } else {
                        menu.on_flyout_move(x, y);
                    }
                    MenuOutcome::None
                }
                GUI_EVENT_MOUSE if event.payload[3] == GUI_MOUSE_DOWN => {
                    if is_root {
                        menu.on_root_click(x, y)
                    } else {
                        menu.on_flyout_click(x, y)
                    }
                }
                _ => MenuOutcome::None,
            }
        };
        match outcome {
            MenuOutcome::None => {}
            MenuOutcome::Close => self.close_menu(),
            MenuOutcome::Activate(action) => {
                self.close_menu();
                self.activate(action);
            }
        }
    }

    fn handle_run_event(&mut self, event: gui::GuiEvent) {
        match event.kind {
            GUI_EVENT_CLOSE => self.close_run(),
            GUI_EVENT_MOUSE if event.payload[3] == GUI_MOUSE_DOWN => {
                let x = event.payload[0] as i32;
                let y = event.payload[1] as i32;
                let (ok, cancel) = run_buttons();
                if hit_rect(ok, x, y) {
                    self.submit_run();
                } else if hit_rect(cancel, x, y) {
                    self.close_run();
                }
            }
            GUI_EVENT_KEY if event.payload[3] != 0 => match event.payload[0] {
                runtime::KEY_ESCAPE => self.close_run(),
                runtime::KEY_ENTER => self.submit_run(),
                runtime::KEY_BACKSPACE => {
                    if let Some(dialog) = self.run.as_mut() {
                        dialog.input.pop();
                        dialog.render();
                    }
                }
                _ => {
                    let character = char::from_u32(event.payload[1]).unwrap_or('\0');
                    if character >= ' ' && character != '\u{7f}' {
                        if let Some(dialog) = self.run.as_mut() {
                            if dialog.input.len() < 200 {
                                dialog.input.push(character);
                                dialog.render();
                            }
                        }
                    }
                }
            },
            _ => {}
        }
    }

    // -- start menu --------------------------------------------------------

    fn open_menu(&mut self) {
        if let Some(menu) = Menu::open(START_BTN_X, self.screen_h as i32) {
            self.menu = Some(menu);
        }
    }

    fn close_menu(&mut self) {
        self.menu = None; // Menu drops → Window drops → destroyed
        self.menu_closed_iter = self.iter;
    }

    // -- run dialog --------------------------------------------------------

    fn open_run(&mut self) {
        if self.run.is_some() {
            return;
        }
        let x = ((self.screen_w as i32 - RUN_W as i32) / 2).max(0);
        let y = ((self.screen_h as i32 - RUN_H as i32) / 3).max(0);
        let options = WindowOptions { resizable: false };
        if let Ok(window) = Window::new_with_options(RUN_W, RUN_H, "Run", options) {
            // The kernel centers via the frame; nudge is not required.
            let _ = (x, y);
            let mut dialog = RunDialog {
                window,
                input: String::new(),
            };
            dialog.render();
            self.run = Some(dialog);
        }
    }

    fn submit_run(&mut self) {
        let command = self.run.as_ref().map(|dialog| dialog.input.clone());
        self.close_run();
        if let Some(command) = command {
            let trimmed = command.trim();
            if !trimmed.is_empty() {
                self.spawn(ZSH_PATH, &["zsh", "-c", trimmed]);
            }
        }
    }

    fn close_run(&mut self) {
        self.run = None;
    }

    // -- launching ---------------------------------------------------------

    fn activate(&mut self, action: MenuAction) {
        match action {
            MenuAction::Terminal => {
                runtime::gui_shell_spawn_terminal();
            }
            MenuAction::FileManager => self.spawn("/host/FILEMAN.ELF", &["explorer"]),
            MenuAction::WebBrowser => self.spawn(
                "/host/LINKS.ELF",
                &["links2", "-g", "-driver", "agenticos", "-no-connect"],
            ),
            MenuAction::Notepad => self.spawn("/host/NOTEPAD.ELF", &["notepad"]),
            MenuAction::Painting => self.spawn("/host/PAINTING.ELF", &["painting"]),
            MenuAction::Calc => self.spawn("/host/CALC.ELF", &["calc"]),
            MenuAction::GlGame => self.spawn("/host/GLGAME.ELF", &["glgame"]),
            MenuAction::TaskManager => self.spawn("/host/TASKMGR.ELF", &["taskmgr"]),
            MenuAction::Settings => self.spawn("/host/CONTROL.ELF", &["control"]),
            MenuAction::Run => self.open_run(),
            MenuAction::ShutDown => {}
        }
    }

    /// Launch `path` with `argv` via `fork`+`execve`, inheriting our
    /// environment. Tracks the child pid for later reaping.
    fn spawn(&mut self, path: &str, argv: &[&str]) {
        let path_c = gui::c_path(path);
        let arg_bytes: Vec<Vec<u8>> = argv.iter().map(|arg| gui::c_path(arg)).collect();
        let mut arg_ptrs: Vec<*const u8> = arg_bytes.iter().map(|arg| arg.as_ptr()).collect();
        arg_ptrs.push(core::ptr::null());
        let pid = runtime::fork();
        if pid == 0 {
            let result = runtime::execve(&path_c, &arg_ptrs, &self.envp);
            unsafe { runtime::exit(if result < 0 { 126 } else { 0 }) }
        }
        if pid > 0 {
            self.children.push(pid as i32);
        }
    }

    fn reap_children(&mut self) {
        loop {
            let mut status = 0u32;
            let pid = runtime::wait4(-1, Some(&mut status), runtime::WNOHANG);
            if pid <= 0 {
                break;
            }
            self.children.retain(|&child| child != pid as i32);
        }
    }

    // -- taskbar state -----------------------------------------------------

    /// Refresh the tracked top-level window list. Returns whether it changed.
    fn refresh_windows(&mut self) -> bool {
        let mut records = [runtime::ShellWindowRecord {
            id: 0,
            state: 0,
            reserved: 0,
            title: [0; 64],
        }; MAX_TASK_WINDOWS];
        let count = runtime::gui_shell_list_windows(&mut records);
        if count < 0 {
            return false;
        }
        let count = count as usize;

        let changed = count != self.buttons.len()
            || records[..count]
                .iter()
                .zip(self.buttons.iter())
                .any(|(record, button)| {
                    record.id != button.frame_id
                        || record.state as u8 != button.state
                        || record.title_str() != button.title
                });
        if !changed {
            return false;
        }

        self.buttons.clear();
        for record in &records[..count] {
            self.buttons.push(TaskButton {
                frame_id: record.id,
                title: String::from(record.title_str()),
                state: record.state as u8,
                x: 0,
                w: 0,
            });
        }
        self.layout_buttons();
        true
    }

    fn layout_buttons(&mut self) {
        let tray_w = self.tray_width();
        let start = START_BTN_X + START_BTN_W + TASK_BTN_GAP;
        let end = self.screen_w as i32 - tray_w - TASK_BTN_GAP;
        let available = (end - start).max(0);
        let count = self.buttons.len() as i32;
        if count == 0 {
            return;
        }
        let each =
            ((available - (count - 1) * TASK_BTN_GAP) / count).clamp(24, TASK_BTN_MAX_W);
        let mut x = start;
        for button in self.buttons.iter_mut() {
            button.x = x;
            button.w = each;
            x += each + TASK_BTN_GAP;
        }
    }

    fn tray_width(&self) -> i32 {
        (self.clock.chars().count() as i32 * FONT_CELL_WIDTH) + 16
    }

    /// Recompute the tray clock string. Returns whether the visible minute
    /// changed.
    fn update_clock(&mut self) -> bool {
        let mut now = runtime::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if runtime::clock_gettime(CLOCK_REALTIME, &mut now) < 0 {
            return false;
        }
        let minute = now.tv_sec / 60;
        if minute == self.last_minute {
            return false;
        }
        self.last_minute = minute;
        self.clock = format_clock(now.tv_sec);
        true
    }

    // -- rendering ---------------------------------------------------------

    fn render_panel(&mut self) {
        self.layout_buttons();
        let width = self.screen_w;
        let tray_w = self.tray_width();
        let clock = self.clock.clone();
        let buttons: Vec<(i32, i32, String, bool)> = self
            .buttons
            .iter()
            .map(|button| (button.x, button.w, button.title.clone(), button.state == 1))
            .collect();
        let menu_open = self.menu.is_some();

        let canvas = self.panel.canvas_mut();
        theme::draw_taskbar_surface(canvas, 0, 0, width, TASKBAR_H);

        draw_task_button(canvas, START_BTN_X, START_BTN_W, "Start", ButtonState::Normal, menu_open);

        for (x, w, title, minimized) in &buttons {
            let state = if *minimized {
                ButtonState::Disabled
            } else {
                ButtonState::Normal
            };
            draw_task_button(canvas, *x, *w, title, state, false);
        }

        let tray_x = width as i32 - tray_w + 8;
        let tray_y = (TASKBAR_H as i32 - FONT_LINE_HEIGHT) / 2;
        canvas.draw_text(tray_x, tray_y, &clock, theme::taskbar_text());

        let _ = self.panel.present();
    }
}

// ---------------------------------------------------------------------------
// Panel drawing helpers
// ---------------------------------------------------------------------------

fn draw_task_button(
    canvas: &mut Canvas,
    x: i32,
    w: i32,
    label: &str,
    state: ButtonState,
    accent: bool,
) {
    if w < 8 {
        return;
    }
    theme::draw_task_button(canvas, x, BTN_Y, w as u32, BTN_H as u32, state, accent);
    let text = if accent {
        theme::taskbar_text()
    } else {
        theme::task_button_text(state)
    };
    let max_chars = ((w - 12) / FONT_CELL_WIDTH).max(0) as usize;
    let clipped = clip_label(label, max_chars);
    let text_w = clipped.chars().count() as i32 * FONT_CELL_WIDTH;
    let label_x = x + ((w - text_w) / 2).max(6);
    let label_y = BTN_Y + (BTN_H - FONT_LINE_HEIGHT) / 2;
    canvas.draw_text(label_x, label_y, &clipped, text);
}

fn clip_label(label: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if label.chars().count() <= max_chars {
        return String::from(label);
    }
    if max_chars <= 1 {
        return label.chars().take(max_chars).collect();
    }
    let mut out: String = label.chars().take(max_chars - 1).collect();
    out.push('\u{2026}');
    out
}

/// Format `epoch_secs` (UTC) as `HH:MM UTC  YYYY-MM-DD`.
fn format_clock(epoch_secs: i64) -> String {
    let secs = epoch_secs.rem_euclid(86_400);
    let days = epoch_secs.div_euclid(86_400);
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:02}:{:02} UTC  {:04}-{:02}-{:02}",
        hours, minutes, year, month, day
    )
}

/// Convert days-since-Unix-epoch to a proleptic-Gregorian `(year, month, day)`.
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {}",
        "ud2",
        sym desktop_main,
    );
}

unsafe extern "C" fn desktop_main(stack: *const u64) -> ! {
    let startup = runtime::startup_from_stack(stack);
    let mut envp = startup.envp.to_vec();
    envp.push(core::ptr::null());

    if runtime::gui_shell_register(0) < 0 {
        runtime::exit(1);
    }

    let code = match Shell::new(envp) {
        Ok(mut shell) => shell.run_loop(),
        Err(error) => error,
    };
    runtime::exit(if code == 0 { 0 } else { 1 })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}
