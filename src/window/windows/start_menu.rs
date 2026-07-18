//! Windows 95/98-style Start menu used by GUIShell.
//!
//! The root menu and Programs fly-out are painted by one dynamically-sized
//! window. Keeping them in one window preserves the window manager's existing
//! single-active-popup outside-click behavior without teaching generic context
//! menus about popup groups.

use alloc::boxed::Box;

use super::base::WindowBase;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::{get_caption_font, get_default_font};
use crate::graphics::images::SvgImage;
use crate::window::event::MouseEventType;
use crate::window::theme::controls;
use crate::window::{Event, EventResult, GraphicsDevice, Point, Rect, Window, WindowId};

/// Root rows are deliberately taller than ordinary popup-menu rows, matching
/// the roomier Windows 95/98 Start-menu treatment.
pub const START_MENU_ROOT_ROW_HEIGHT: u32 = 32;
pub const START_MENU_PROGRAM_ROW_HEIGHT: u32 = 24;
pub const START_MENU_SEPARATOR_HEIGHT: u32 = 8;
pub const START_MENU_BANNER_WIDTH: u32 = 28;
pub const START_MENU_ROOT_WIDTH: u32 = 196;
pub const START_MENU_PROGRAMS_WIDTH: u32 = 144;
const PANEL_BORDER: u32 = 2;
const ROOT_ICON_SIZE: u32 = 24;
const PROGRAM_ICON_SIZE: u32 = 18;

lazy_static::lazy_static! {
    static ref START_MENU_ICONS: [SvgImage<'static>; 12] = [
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/programs.svg"))
            .expect("embedded Programs SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/documents.svg"))
            .expect("embedded Documents SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/settings.svg"))
            .expect("embedded Settings SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/run.svg"))
            .expect("embedded Run SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/shutdown.svg"))
            .expect("embedded Shut Down SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/file-manager.svg"))
            .expect("embedded File Manager SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/terminal.svg"))
            .expect("embedded Terminal SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/notepad.svg"))
            .expect("embedded Notepad SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/painting.svg"))
            .expect("embedded Painting SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/calc.svg"))
            .expect("embedded Calc SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/gl-arena.svg"))
            .expect("embedded GL Arena SVG must be valid"),
        SvgImage::from_bytes(include_bytes!("../../../assets/icons/start/task-manager.svg"))
            .expect("embedded Task Manager SVG must be valid"),
    ];
}

/// Typed actions emitted by enabled Start-menu leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMenuAction {
    Settings,
    FileManager,
    Terminal,
    Notepad,
    Painting,
    Calc,
    GlGame,
    TaskManager,
    Run,
    ShutDown,
}

/// Root/program row model. Layout and hit testing derive from these variants,
/// so separators and disabled placeholders cannot accidentally dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMenuItem {
    Action {
        label: &'static str,
        action: StartMenuAction,
    },
    Submenu {
        label: &'static str,
    },
    Disabled {
        label: &'static str,
    },
    Separator,
}

pub const START_MENU_ROOT_ITEMS: &[StartMenuItem] = &[
    StartMenuItem::Submenu { label: "Programs" },
    StartMenuItem::Disabled { label: "Documents" },
    StartMenuItem::Action {
        label: "Settings",
        action: StartMenuAction::Settings,
    },
    StartMenuItem::Action {
        label: "Run...",
        action: StartMenuAction::Run,
    },
    StartMenuItem::Separator,
    StartMenuItem::Action {
        label: "Shut Down...",
        action: StartMenuAction::ShutDown,
    },
];

pub const START_MENU_PROGRAM_ITEMS: &[StartMenuItem] = &[
    StartMenuItem::Action {
        label: "File Manager",
        action: StartMenuAction::FileManager,
    },
    StartMenuItem::Action {
        label: "Terminal",
        action: StartMenuAction::Terminal,
    },
    StartMenuItem::Action {
        label: "Notepad",
        action: StartMenuAction::Notepad,
    },
    StartMenuItem::Action {
        label: "Painting",
        action: StartMenuAction::Painting,
    },
    StartMenuItem::Action {
        label: "Calc",
        action: StartMenuAction::Calc,
    },
    StartMenuItem::Action {
        label: "GL Arena",
        action: StartMenuAction::GlGame,
    },
    StartMenuItem::Action {
        label: "Task Manager",
        action: StartMenuAction::TaskManager,
    },
];

pub type StartMenuSelectCallback = Box<dyn FnMut(StartMenuAction) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    Root(usize),
    Program(usize),
}

pub struct StartMenuWindow {
    base: WindowBase,
    root_origin: Point,
    programs_open: bool,
    hover: Option<HoverTarget>,
    pressed: Option<HoverTarget>,
    on_select: Option<StartMenuSelectCallback>,
}

impl StartMenuWindow {
    pub fn new_with_id(id: WindowId, origin: Point) -> Self {
        let root_height = Self::root_height();
        Self {
            base: WindowBase::new_with_id(
                id,
                Rect::new(origin.x, origin.y, START_MENU_ROOT_WIDTH, root_height),
            ),
            root_origin: origin,
            programs_open: false,
            hover: None,
            pressed: None,
            on_select: None,
        }
    }

    pub const fn root_height() -> u32 {
        let mut height = PANEL_BORDER * 2;
        let mut index = 0;
        while index < START_MENU_ROOT_ITEMS.len() {
            height += match START_MENU_ROOT_ITEMS[index] {
                StartMenuItem::Separator => START_MENU_SEPARATOR_HEIGHT,
                _ => START_MENU_ROOT_ROW_HEIGHT,
            };
            index += 1;
        }
        height
    }

    /// Maximum width after the Programs fly-out opens.
    pub const fn maximum_width() -> u32 {
        START_MENU_ROOT_WIDTH + START_MENU_PROGRAMS_WIDTH - PANEL_BORDER
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn programs_open(&self) -> bool {
        self.programs_open
    }

    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(StartMenuAction) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    fn set_programs_open(&mut self, open: bool) {
        if self.programs_open == open {
            return;
        }
        self.programs_open = open;
        let width = if open {
            Self::maximum_width()
        } else {
            START_MENU_ROOT_WIDTH
        };
        self.base.set_bounds(Rect::new(
            self.root_origin.x,
            self.root_origin.y,
            width,
            Self::root_height(),
        ));
        self.base.invalidate();
    }

    fn row_at(items: &[StartMenuItem], y: i32, row_height: u32) -> Option<usize> {
        if y < PANEL_BORDER as i32 {
            return None;
        }
        let mut top = PANEL_BORDER as i32;
        for (index, item) in items.iter().enumerate() {
            let height = match item {
                StartMenuItem::Separator => START_MENU_SEPARATOR_HEIGHT,
                _ => row_height,
            } as i32;
            if y >= top && y < top + height {
                return (!matches!(item, StartMenuItem::Separator)).then_some(index);
            }
            top += height;
        }
        None
    }

    fn target_at(&self, point: Point) -> Option<HoverTarget> {
        if point.x >= START_MENU_BANNER_WIDTH as i32 && point.x < START_MENU_ROOT_WIDTH as i32 {
            return Self::row_at(START_MENU_ROOT_ITEMS, point.y, START_MENU_ROOT_ROW_HEIGHT)
                .map(HoverTarget::Root);
        }
        if self.programs_open
            && point.x >= (START_MENU_ROOT_WIDTH - PANEL_BORDER) as i32
            && point.x < (START_MENU_ROOT_WIDTH + START_MENU_PROGRAMS_WIDTH - PANEL_BORDER) as i32
        {
            return Self::row_at(
                START_MENU_PROGRAM_ITEMS,
                point.y,
                START_MENU_PROGRAM_ROW_HEIGHT,
            )
            .map(HoverTarget::Program);
        }
        None
    }

    fn item_for_target(target: HoverTarget) -> &'static StartMenuItem {
        match target {
            HoverTarget::Root(index) => &START_MENU_ROOT_ITEMS[index],
            HoverTarget::Program(index) => &START_MENU_PROGRAM_ITEMS[index],
        }
    }

    fn target_enabled(target: HoverTarget) -> bool {
        matches!(
            Self::item_for_target(target),
            StartMenuItem::Action { .. } | StartMenuItem::Submenu { .. }
        )
    }

    fn update_hover(&mut self, target: Option<HoverTarget>) {
        let hover = target.filter(|target| Self::target_enabled(*target));
        if self.hover != hover {
            self.hover = hover;
            self.base.invalidate();
        }

        match target {
            Some(HoverTarget::Root(0)) | Some(HoverTarget::Program(_)) => {
                self.set_programs_open(true)
            }
            Some(HoverTarget::Root(_)) => self.set_programs_open(false),
            None => {}
        }
    }

    fn activate(&mut self, target: HoverTarget) {
        match *Self::item_for_target(target) {
            StartMenuItem::Action { action, .. } => {
                if let Some(callback) = self.on_select.as_mut() {
                    callback(action);
                }
            }
            StartMenuItem::Submenu { .. } => self.set_programs_open(true),
            StartMenuItem::Disabled { .. } | StartMenuItem::Separator => {}
        }
    }

    fn draw_rotated_banner_text(device: &mut dyn GraphicsDevice, banner: Rect, color: Color) {
        let font = get_caption_font();
        let total_advance: i32 = "AgenticOS"
            .chars()
            .filter_map(|ch| font.glyph(ch).map(|glyph| glyph.advance as i32))
            .sum();
        let mut pen_x = 0i32;
        let bottom = banner.y + banner.height as i32 - 5;
        let text_top = bottom - total_advance;
        let baseline = font.ascent() as i32;

        for ch in "AgenticOS".chars() {
            let Some(glyph) = font.glyph(ch) else {
                continue;
            };
            for row in 0..glyph.height as i32 {
                for col in 0..glyph.width as i32 {
                    let alpha =
                        glyph.coverage[(row as usize * glyph.width as usize) + col as usize];
                    if alpha == 0 {
                        continue;
                    }
                    let source_x = pen_x + glyph.x_offset + col;
                    let source_y = baseline + glyph.y_offset + row;
                    let dst_x = banner.x + 4 + source_y;
                    let dst_y = bottom - source_x;
                    if dst_y < text_top || dst_y > bottom {
                        continue;
                    }
                    if alpha == u8::MAX {
                        device.draw_pixel(dst_x, dst_y, color);
                    } else {
                        let bg = device.read_pixel(dst_x, dst_y);
                        device.draw_pixel(dst_x, dst_y, bg.blend(&color, alpha));
                    }
                }
            }
            pen_x += glyph.advance as i32;
        }
    }

    fn draw_arrow(device: &mut dyn GraphicsDevice, x: i32, y: i32, color: Color) {
        for column in 0..4i32 {
            let half = column;
            device.draw_line(x + column, y - half, x + column, y + half, color);
        }
    }

    fn icon_for_item(item: &StartMenuItem) -> &'static SvgImage<'static> {
        let index = match item {
            StartMenuItem::Submenu { .. } => 0,
            StartMenuItem::Disabled { .. } => 1,
            StartMenuItem::Action { action, .. } => match action {
                StartMenuAction::Settings => 2,
                StartMenuAction::Run => 3,
                StartMenuAction::ShutDown => 4,
                StartMenuAction::FileManager => 5,
                StartMenuAction::Terminal => 6,
                StartMenuAction::Notepad => 7,
                StartMenuAction::Painting => 8,
                StartMenuAction::Calc => 9,
                StartMenuAction::GlGame => 10,
                StartMenuAction::TaskManager => 11,
            },
            StartMenuItem::Separator => 0,
        };
        &START_MENU_ICONS[index]
    }

    fn paint_items(
        &self,
        device: &mut dyn GraphicsDevice,
        items: &[StartMenuItem],
        panel: Rect,
        root: bool,
    ) {
        let font = get_default_font();
        let palette = controls::palette();
        let row_height = if root {
            START_MENU_ROOT_ROW_HEIGHT
        } else {
            START_MENU_PROGRAM_ROW_HEIGHT
        };
        let mut item_y = panel.y + PANEL_BORDER as i32;
        let icon_size = if root {
            ROOT_ICON_SIZE
        } else {
            PROGRAM_ICON_SIZE
        };
        let icon_left = if root {
            panel.x + START_MENU_BANNER_WIDTH as i32 + 5
        } else {
            panel.x + 5
        };
        let text_left = icon_left + icon_size as i32 + 5;
        let content_left = if root {
            panel.x + START_MENU_BANNER_WIDTH as i32 + 2
        } else {
            panel.x + 2
        };
        let content_right = panel.right() - 3;

        for (index, item) in items.iter().enumerate() {
            match item {
                StartMenuItem::Separator => {
                    let line_y = item_y + START_MENU_SEPARATOR_HEIGHT as i32 / 2 - 1;
                    controls::draw_menu_separator(
                        device,
                        content_left + 4,
                        line_y,
                        (content_right - content_left - 7).max(0) as u32,
                    );
                    item_y += START_MENU_SEPARATOR_HEIGHT as i32;
                }
                StartMenuItem::Action { label, .. }
                | StartMenuItem::Submenu { label }
                | StartMenuItem::Disabled { label } => {
                    let target = if root {
                        HoverTarget::Root(index)
                    } else {
                        HoverTarget::Program(index)
                    };
                    let highlighted = self.hover == Some(target)
                        && !matches!(item, StartMenuItem::Disabled { .. });
                    if highlighted {
                        controls::draw_selection(
                            device,
                            Rect::new(
                                content_left,
                                item_y,
                                (content_right - content_left + 1) as u32,
                                row_height,
                            ),
                        );
                    }
                    let icon_top = item_y + (row_height.saturating_sub(icon_size) / 2) as i32;
                    device.draw_image_scaled(
                        icon_left,
                        icon_top,
                        icon_size,
                        icon_size,
                        Self::icon_for_item(item),
                    );
                    let text_y =
                        item_y + (row_height.saturating_sub(font.line_height()) / 2) as i32;
                    if matches!(item, StartMenuItem::Disabled { .. }) {
                        device.draw_text(
                            text_left,
                            text_y,
                            label,
                            font.as_font(),
                            palette.disabled_text,
                        );
                    } else {
                        let color = if highlighted {
                            palette.selection_text
                        } else {
                            palette.text
                        };
                        device.draw_text(text_left, text_y, label, font.as_font(), color);
                        if matches!(item, StartMenuItem::Submenu { .. }) {
                            Self::draw_arrow(
                                device,
                                content_right - 8,
                                item_y + row_height as i32 / 2,
                                color,
                            );
                        }
                    }
                    item_y += row_height as i32;
                }
            }
        }
    }
}

impl Window for StartMenuWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }
        let bounds = self.base.bounds();
        let root = Rect::new(
            bounds.x,
            bounds.y,
            START_MENU_ROOT_WIDTH,
            Self::root_height(),
        );
        controls::draw_menu_surface(device, root);
        let palette = controls::palette();
        let banner = Rect::new(
            root.x + 2,
            root.y + 2,
            START_MENU_BANNER_WIDTH - 2,
            root.height - 4,
        );
        device.fill_rect(
            banner.x,
            banner.y,
            banner.width,
            banner.height,
            palette.selection_bg,
        );
        Self::draw_rotated_banner_text(device, banner, palette.selection_text);
        self.paint_items(device, START_MENU_ROOT_ITEMS, root, true);

        if self.programs_open {
            let programs = Rect::new(
                root.x + START_MENU_ROOT_WIDTH as i32 - PANEL_BORDER as i32,
                root.y,
                START_MENU_PROGRAMS_WIDTH,
                root.height,
            );
            controls::draw_menu_surface(device, programs);
            self.paint_items(device, START_MENU_PROGRAM_ITEMS, programs, false);
        }
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        let target = self.target_at(mouse.position);
        match mouse.event_type {
            MouseEventType::Move => {
                self.update_hover(target);
                EventResult::Handled
            }
            MouseEventType::ButtonDown if mouse.buttons.left => {
                self.update_hover(target);
                self.pressed = target.filter(|target| Self::target_enabled(*target));
                EventResult::Handled
            }
            MouseEventType::ButtonUp => {
                let pressed = self.pressed.take();
                self.update_hover(target);
                if let Some(pressed) = pressed.filter(|pressed| Some(*pressed) == target) {
                    self.activate(pressed);
                }
                EventResult::Handled
            }
            _ => EventResult::Handled,
        }
    }
}
