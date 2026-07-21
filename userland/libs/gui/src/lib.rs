#![no_std]

extern crate alloc;

mod controls;
pub mod file_ui;
mod font;
mod input;
mod menu;
mod scrollbar;
mod slider;
mod text_area;
pub mod theme;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub use controls::{CheckBox, ProgressBar, RadioButton, StatusBar, ToggleAction, Toolbar};
pub use gui_core::{
    Axis, ControlInput, ControlResponse, KeyInput, Modifiers, MouseButtons, PointerInput,
    PointerKind, Rect, ScrollState, ScrollbarPolicy, TextEdit,
};
pub use input::decode_control_input;
pub use menu::{MenuEntry, MenuEntryFlags, MenuPopup, MenuPopupAction};
pub use runtime::{
    GuiEvent, GUI_EVENT_CLOSE, GUI_EVENT_FOCUS_CHANGE, GUI_EVENT_KEY, GUI_EVENT_MOUSE,
    GUI_EVENT_RESIZE, GUI_EVENT_SETTINGS_CHANGED, GUI_EVENT_THEME_CHANGED, GUI_MOUSE_DOWN,
    GUI_MOUSE_MOVE, GUI_MOUSE_SCROLL, GUI_MOUSE_UP, GUI_WINDOW_FIXED_SIZE,
};
pub use scrollbar::{Scrollbar, ScrollbarAction, SCROLLBAR_THICKNESS};
pub use slider::{Slider, SliderAction};
pub use text_area::{TextArea, TextAreaAction, TextAreaOptions, WrapMode};
pub use theme::{ButtonState, Theme};

pub const FONT_CELL_WIDTH: i32 = font::CELL_WIDTH;
pub const FONT_LINE_HEIGHT: i32 = font::LINE_HEIGHT;

// Legacy theme-agnostic values. Widgets now derive their colors from the
// active theme (`theme::palette()`); these remain for app-specific chrome
// that intentionally does not follow the theme.
pub const COLOR_BLACK: u32 = 0x000000;
pub const COLOR_WHITE: u32 = 0xFFFFFF;
pub const COLOR_TEXT: u32 = 0x202020;
pub const COLOR_PANEL: u32 = 0xF0F0F0;
pub const COLOR_BORDER: u32 = 0x707070;
pub const COLOR_HIGHLIGHT: u32 = 0x0078D7;
/// Muted text for secondary/disabled rows (e.g. kernel threads in a
/// process list).
pub const COLOR_TEXT_DIM: u32 = 0x8A8A8A;
/// Fill shade under [`TimeSeriesGraph`]'s primary series (light accent).
pub const COLOR_ACCENT_FILL: u32 = 0xCCE4F7;
/// Secondary graph series (green) and its fill shade.
pub const COLOR_ACCENT2: u32 = 0x107C10;

enum PixelBuffer {
    Owned(Vec<u32>),
    Borrowed { pointer: *mut u32, len: usize },
}

impl PixelBuffer {
    fn as_slice(&self) -> &[u32] {
        match self {
            Self::Owned(pixels) => pixels,
            Self::Borrowed { pointer, len } => unsafe {
                core::slice::from_raw_parts(*pointer, *len)
            },
        }
    }

    fn as_mut_slice(&mut self) -> &mut [u32] {
        match self {
            Self::Owned(pixels) => pixels,
            Self::Borrowed { pointer, len } => unsafe {
                core::slice::from_raw_parts_mut(*pointer, *len)
            },
        }
    }
}

pub struct Canvas {
    width: u32,
    height: u32,
    pixels: PixelBuffer,
    clip: Option<Rect>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: PixelBuffer::Owned(vec![0; width as usize * height as usize]),
            clip: None,
        }
    }

    /// Wraps an existing tightly packed XRGB8888 pixel buffer without taking
    /// ownership of it.
    ///
    /// # Safety
    ///
    /// `pointer` must remain valid and exclusively borrowed for at least
    /// `width * height` `u32` pixels for the entire lifetime of the canvas.
    /// The canvas must be dropped before the backing allocation is resized or
    /// released.
    pub unsafe fn from_borrowed(width: u32, height: u32, pointer: *mut u32) -> Option<Self> {
        let len = (width as usize).checked_mul(height as usize)?;
        if len != 0 && pointer.is_null() {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels: PixelBuffer::Borrowed { pointer, len },
            clip: None,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn pixels(&self) -> &[u32] {
        self.pixels.as_slice()
    }
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        self.pixels.as_mut_slice()
    }
    pub fn is_borrowed(&self) -> bool {
        matches!(self.pixels, PixelBuffer::Borrowed { .. })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let len = width as usize * height as usize;
        match &mut self.pixels {
            PixelBuffer::Owned(pixels) => pixels.resize(len, 0),
            PixelBuffer::Borrowed { len: current, .. } => {
                assert_eq!(*current, len, "cannot resize a borrowed canvas buffer")
            }
        }
        self.width = width;
        self.height = height;
        self.clip = None;
    }

    pub fn clip(&self) -> Option<Rect> {
        self.clip
    }

    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
    }

    pub fn clear(&mut self, color: u32) {
        self.pixels.as_mut_slice().fill(color);
    }

    pub fn pixel(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if self.clip.map(|clip| !clip.contains(x, y)).unwrap_or(false) {
            return;
        }
        self.pixels.as_mut_slice()[y as usize * self.width as usize + x as usize] = color;
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: u32, alpha: u8) {
        if alpha == 0 || x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if self.clip.map(|clip| !clip.contains(x, y)).unwrap_or(false) {
            return;
        }
        if alpha == u8::MAX {
            self.pixel(x, y, color);
            return;
        }
        let index = y as usize * self.width as usize + x as usize;
        let background = self.pixels.as_slice()[index];
        let alpha = alpha as u32;
        let inverse = 255 - alpha;
        let blend = |shift: u32| {
            ((((color >> shift) & 0xff_u32) * alpha
                + ((background >> shift) & 0xff_u32) * inverse
                + 127_u32)
                / 255)
                << shift
        };
        self.pixels.as_mut_slice()[index] = blend(16) | blend(8) | blend(0);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: u32) {
        let mut left = x.max(0);
        let mut top = y.max(0);
        let mut right = (x.saturating_add(width as i32))
            .max(0)
            .min(self.width as i32);
        let mut bottom = (y.saturating_add(height as i32))
            .max(0)
            .min(self.height as i32);
        if let Some(clip) = self.clip {
            left = left.max(clip.x);
            top = top.max(clip.y);
            right = right.min(clip.right());
            bottom = bottom.min(clip.bottom());
        }
        let left = left.max(0) as u32;
        let top = top.max(0) as u32;
        let right = right.max(0) as u32;
        let bottom = bottom.max(0) as u32;
        if right <= left || bottom <= top {
            return;
        }
        for row in top..bottom {
            let start = row as usize * self.width as usize + left as usize;
            let end = row as usize * self.width as usize + right as usize;
            self.pixels.as_mut_slice()[start..end].fill(color);
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
        let font = font::canvas_font();
        let Some(glyph) = font.glyph(character) else {
            return;
        };
        let left = x + glyph.x_offset as i32;
        let top = y + font.ascent() + glyph.y_offset as i32;
        for row in 0..glyph.height as usize {
            for column in 0..glyph.width as usize {
                self.blend_pixel(
                    left + column as i32,
                    top + row as i32,
                    color,
                    glyph.coverage[row * glyph.width as usize + column],
                );
            }
        }
    }

    pub fn draw_text(&mut self, mut x: i32, y: i32, text: &str, color: u32) {
        for character in text.chars() {
            if character == '\n' {
                break;
            }
            self.draw_char(x, y, character, color);
            x += FONT_CELL_WIDTH;
        }
    }
}

pub struct Window {
    handle: u32,
    canvas: Canvas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowOptions {
    pub resizable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self { resizable: true }
    }
}

impl Window {
    pub fn new(width: u32, height: u32, title: &str) -> Result<Self, i64> {
        Self::new_with_options(width, height, title, WindowOptions::default())
    }

    pub fn new_with_options(
        width: u32,
        height: u32,
        title: &str,
        options: WindowOptions,
    ) -> Result<Self, i64> {
        let flags = if options.resizable {
            0
        } else {
            GUI_WINDOW_FIXED_SIZE
        };
        let result = runtime::gui_win_create(width, height, title, flags);
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

    pub fn set_title(&self, title: &str) -> Result<(), i64> {
        let result = runtime::gui_win_set_title(self.handle, title);
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
        theme::apply_system_event(&event);
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
        theme::apply_system_event(&event);
        Ok(Some(event))
    }
}

pub struct MenuBar<'a> {
    pub label: &'a str,
    pub items: &'a [&'a str],
    pub open: bool,
    title_hovered: bool,
    hover_index: Option<usize>,
}

impl<'a> MenuBar<'a> {
    pub const HEIGHT: u32 = 24;
    pub const ITEM_HEIGHT: u32 = 22;

    pub const fn new(label: &'a str, items: &'a [&'a str]) -> Self {
        Self {
            label,
            items,
            open: false,
            title_hovered: false,
            hover_index: None,
        }
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let palette = theme::palette();
        canvas.fill_rect(0, 0, canvas.width(), Self::HEIGHT, palette.content_bg);
        canvas.horizontal_line(0, Self::HEIGHT as i32 - 1, canvas.width(), palette.border);
        let title_highlighted = self.open || self.title_hovered;
        if title_highlighted {
            theme::draw_selection(canvas, 4, 3, 48, 18);
        }
        canvas.draw_text(
            10,
            (Self::HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
            self.label,
            if title_highlighted {
                palette.selection_text
            } else {
                palette.text
            },
        );
        if self.open {
            let width = 128;
            let height = Self::ITEM_HEIGHT * self.items.len() as u32 + 4;
            theme::draw_menu_surface(canvas, 4, Self::HEIGHT as i32, width, height);
            for (index, item) in self.items.iter().enumerate() {
                let highlighted = self.hover_index == Some(index);
                if highlighted {
                    theme::draw_selection(
                        canvas,
                        6,
                        Self::HEIGHT as i32 + 2 + index as i32 * Self::ITEM_HEIGHT as i32,
                        width - 4,
                        Self::ITEM_HEIGHT,
                    );
                }
                canvas.draw_text(
                    12,
                    Self::HEIGHT as i32
                        + index as i32 * Self::ITEM_HEIGHT as i32
                        + (Self::ITEM_HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                    item,
                    if highlighted {
                        palette.selection_text
                    } else {
                        palette.text
                    },
                );
            }
        }
    }

    /// Update menu hover state. Returns whether a repaint is needed.
    pub fn pointer_move(&mut self, x: i32, y: i32) -> bool {
        let title_hovered = Self::title_hit(x, y);
        let hover_index = self.item_at(x, y);
        if self.title_hovered == title_hovered && self.hover_index == hover_index {
            return false;
        }
        self.title_hovered = title_hovered;
        self.hover_index = hover_index;
        true
    }

    pub fn click(&mut self, x: i32, y: i32) -> Option<usize> {
        if Self::title_hit(x, y) {
            self.open = !self.open;
            if !self.open {
                self.hover_index = None;
            }
            return None;
        }
        if let Some(index) = self.item_at(x, y) {
            self.open = false;
            self.hover_index = None;
            return Some(index);
        }
        self.open = false;
        self.hover_index = None;
        None
    }

    fn title_hit(x: i32, y: i32) -> bool {
        y >= 0 && y < Self::HEIGHT as i32 && x >= 4 && x < 60
    }

    fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.open || !(4..132).contains(&x) || y < Self::HEIGHT as i32 {
            return None;
        }
        let index = ((y - Self::HEIGHT as i32) / Self::ITEM_HEIGHT as i32) as usize;
        (index < self.items.len()).then_some(index)
    }
}

/// Byte index of the character boundary immediately before `index`.
///
/// Shared caret helper used by `TextField` and text-editing apps; moves one
/// grapheme-free `char` to the left, clamped at 0.
pub fn previous_boundary(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// Byte index of the character boundary immediately after `index`.
///
/// Moves one `char` to the right, clamped at `text.len()`.
pub fn next_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(next, _)| index + next)
        .unwrap_or(text.len())
}

/// A clickable push button with a centered system-font label.
///
/// Positioned manually by the caller (no layout engine), matching the
/// `MenuBar` idiom. The typed handlers provide hover, press, disabled, and
/// release-to-activate behavior; the older drawing and hit-test methods remain
/// available for compatibility.
pub struct Button {
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    enabled: bool,
    hot: bool,
    pressed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    Activated,
}

impl Button {
    pub fn new(label: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            label: String::from(label),
            x,
            y,
            w,
            h,
            enabled: true,
            hot: false,
            pressed: false,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.hot = false;
            self.pressed = false;
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn cancel_interaction(&mut self) {
        self.hot = false;
        self.pressed = false;
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ButtonAction> {
        match input.kind {
            PointerKind::Cancel => {
                let repaint = self.hot || self.pressed;
                self.cancel_interaction();
                ControlResponse::consumed(repaint, None)
            }
            PointerKind::Move => {
                let hot = self.enabled && self.hit(input.x, input.y);
                let repaint = hot != self.hot;
                self.hot = hot;
                ControlResponse {
                    consumed: hot || self.pressed,
                    repaint,
                    action: None,
                }
            }
            PointerKind::Down if self.enabled && self.hit(input.x, input.y) => {
                self.hot = true;
                self.pressed = true;
                ControlResponse::consumed(true, None)
            }
            PointerKind::Up if self.pressed => {
                let activate = self.enabled && self.hit(input.x, input.y);
                self.pressed = false;
                self.hot = activate;
                ControlResponse::consumed(true, activate.then_some(ButtonAction::Activated))
            }
            _ => ControlResponse::ignored(),
        }
    }

    pub fn handle_key(&mut self, input: KeyInput, focused: bool) -> ControlResponse<ButtonAction> {
        if !self.enabled || !focused || !input.pressed {
            return ControlResponse::ignored();
        }
        if input.key == runtime::KEY_ENTER || input.character == ' ' {
            ControlResponse::consumed(true, Some(ButtonAction::Activated))
        } else {
            ControlResponse::ignored()
        }
    }

    pub fn draw_control(&self, canvas: &mut Canvas, default: bool) {
        self.draw_state(
            canvas,
            if !self.enabled {
                ButtonState::Disabled
            } else if self.pressed {
                ButtonState::Pressed
            } else if self.hot || default {
                ButtonState::Hot
            } else {
                ButtonState::Normal
            },
        );
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w as i32 && y >= self.y && y < self.y + self.h as i32
    }

    /// Draw with the two classic states: `hot` marks the default / accent
    /// button (Aero: blue border + glow; Classic: black rim).
    pub fn draw(&self, canvas: &mut Canvas, hot: bool) {
        self.draw_state(
            canvas,
            if hot {
                ButtonState::Hot
            } else {
                ButtonState::Normal
            },
        );
    }

    /// Draw with a full [`ButtonState`] for apps that track pressed /
    /// disabled states themselves.
    pub fn draw_state(&self, canvas: &mut Canvas, state: ButtonState) {
        theme::draw_button(canvas, self.x, self.y, self.w, self.h, state);
        let text_width = self.label.chars().count() as i32 * FONT_CELL_WIDTH;
        let text_x = self.x + (self.w as i32 - text_width) / 2;
        let text_y = self.y + (self.h as i32 - FONT_LINE_HEIGHT) / 2;
        let shift = theme::pressed_label_shift(state);
        canvas.draw_text(
            text_x.max(self.x + 2) + shift,
            text_y + shift,
            &self.label,
            theme::button_text(state),
        );
    }
}

/// A single-line editable text field with a byte-index caret.
///
/// Owns its `text` and renders a box, clipped/scrolled text (so the caret is
/// always visible), selection, and a caret line. Prefer [`TextField::handle_input`]
/// for typed key and pointer routing; the direct key/click methods remain for
/// compatibility.
pub struct TextField {
    pub text: String,
    pub caret: usize,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    scroll: usize,
    anchor: Option<usize>,
    selecting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFieldAction {
    Changed,
    SelectionChanged,
}

impl TextField {
    const PAD: i32 = 5;

    pub fn new(x: i32, y: i32, w: u32, h: u32, text: &str) -> Self {
        let text = String::from(text);
        let caret = text.len();
        Self {
            text,
            caret,
            x,
            y,
            w,
            h,
            scroll: 0,
            anchor: None,
            selecting: false,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = String::from(text);
        self.caret = self.text.len();
        self.scroll = 0;
        self.anchor = None;
        self.selecting = false;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.caret).then_some((anchor.min(self.caret), anchor.max(self.caret)))
    }

    fn begin_move(&mut self, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.caret = start;
        self.anchor = None;
        true
    }

    /// Number of `char`s in `text[..self.caret]` — the caret's column.
    fn caret_column(&self) -> usize {
        self.text[..self.caret].chars().count()
    }

    fn visible_columns(&self) -> usize {
        ((self.w as i32 - Self::PAD * 2) / FONT_CELL_WIDTH).max(1) as usize
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w as i32 && y >= self.y && y < self.y + self.h as i32
    }

    /// Place the caret nearest the pixel column of `x` (window coordinates).
    pub fn click(&mut self, x: i32) {
        let column = ((x - self.x - Self::PAD).max(0) / FONT_CELL_WIDTH) as usize + self.scroll;
        self.caret = self
            .text
            .char_indices()
            .nth(column)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len());
        self.anchor = None;
    }

    /// Handle a key press. Returns `true` if the text content changed.
    pub fn key(&mut self, key: u32, character: char) -> bool {
        self.key_input(KeyInput {
            key,
            character,
            modifiers: Modifiers::default(),
            pressed: true,
        }) == Some(TextFieldAction::Changed)
    }

    fn key_input(&mut self, input: KeyInput) -> Option<TextFieldAction> {
        if !input.pressed {
            return None;
        }
        let before = (self.caret, self.selection());
        let changed = match input.key {
            _ if input.modifiers.ctrl && input.character.eq_ignore_ascii_case(&'a') => {
                self.anchor = Some(0);
                self.caret = self.text.len();
                false
            }
            runtime::KEY_LEFT => {
                self.begin_move(input.modifiers.shift);
                if self.caret > 0 {
                    self.caret = previous_boundary(&self.text, self.caret);
                }
                false
            }
            runtime::KEY_RIGHT => {
                self.begin_move(input.modifiers.shift);
                if self.caret < self.text.len() {
                    self.caret = next_boundary(&self.text, self.caret);
                }
                false
            }
            runtime::KEY_HOME => {
                self.begin_move(input.modifiers.shift);
                self.caret = 0;
                false
            }
            runtime::KEY_END => {
                self.begin_move(input.modifiers.shift);
                self.caret = self.text.len();
                false
            }
            runtime::KEY_BACKSPACE => {
                if self.delete_selection() {
                    true
                } else if self.caret > 0 {
                    let previous = previous_boundary(&self.text, self.caret);
                    self.text.replace_range(previous..self.caret, "");
                    self.caret = previous;
                    true
                } else {
                    false
                }
            }
            runtime::KEY_DELETE => {
                if self.delete_selection() {
                    true
                } else if self.caret < self.text.len() {
                    let next = next_boundary(&self.text, self.caret);
                    self.text.replace_range(self.caret..next, "");
                    true
                } else {
                    false
                }
            }
            _ if input.character >= ' ' && input.character != '\u{7f}' => {
                self.delete_selection();
                self.text.insert(self.caret, input.character);
                self.caret += input.character.len_utf8();
                self.anchor = None;
                true
            }
            _ => false,
        };
        if changed {
            Some(TextFieldAction::Changed)
        } else if before != (self.caret, self.selection()) {
            Some(TextFieldAction::SelectionChanged)
        } else {
            None
        }
    }

    pub fn handle_input(
        &mut self,
        input: ControlInput,
        focused: bool,
    ) -> ControlResponse<TextFieldAction> {
        match input {
            ControlInput::Key(key) if focused => {
                let action = self.key_input(key);
                ControlResponse {
                    consumed: action.is_some(),
                    repaint: action.is_some(),
                    action,
                }
            }
            ControlInput::Pointer(pointer) => match pointer.kind {
                PointerKind::Down if self.hit(pointer.x, pointer.y) => {
                    let old = self.caret;
                    if pointer.modifiers.shift {
                        if self.anchor.is_none() {
                            self.anchor = Some(self.caret);
                        }
                        let anchor = self.anchor;
                        self.click(pointer.x);
                        self.anchor = anchor;
                    } else {
                        self.click(pointer.x);
                    }
                    self.selecting = true;
                    ControlResponse::consumed(
                        true,
                        (old != self.caret).then_some(TextFieldAction::SelectionChanged),
                    )
                }
                PointerKind::Move if self.selecting => {
                    let anchor = self.anchor.or(Some(self.caret));
                    self.click(pointer.x);
                    self.anchor = anchor;
                    ControlResponse::consumed(true, Some(TextFieldAction::SelectionChanged))
                }
                PointerKind::Up if self.selecting => {
                    self.selecting = false;
                    ControlResponse::consumed(true, None)
                }
                PointerKind::Cancel => {
                    self.selecting = false;
                    ControlResponse::consumed(true, None)
                }
                _ => ControlResponse::ignored(),
            },
            _ => ControlResponse::ignored(),
        }
    }

    pub fn draw(&mut self, canvas: &mut Canvas, focused: bool) {
        let column = self.caret_column();
        let visible = self.visible_columns();
        if column < self.scroll {
            self.scroll = column;
        } else if column >= self.scroll + visible {
            self.scroll = column + 1 - visible;
        }
        theme::draw_field(canvas, self.x, self.y, self.w, self.h, focused);
        let text_color = theme::palette().field_text;
        let palette = theme::palette();
        let text_y = self.y + (self.h as i32 - FONT_LINE_HEIGHT) / 2;
        let mut pixel_x = self.x + Self::PAD;
        let selection = self.selection();
        for (column, (index, character)) in self.text.char_indices().enumerate() {
            if column < self.scroll {
                continue;
            }
            if column >= self.scroll + visible {
                break;
            }
            let selected = selection
                .map(|(start, end)| index >= start && index < end)
                .unwrap_or(false);
            if selected {
                canvas.fill_rect(
                    pixel_x,
                    text_y - 1,
                    FONT_CELL_WIDTH as u32,
                    FONT_LINE_HEIGHT as u32 + 2,
                    palette.selection_bg,
                );
            }
            canvas.draw_char(
                pixel_x,
                text_y,
                character,
                if selected {
                    palette.selection_text
                } else {
                    text_color
                },
            );
            pixel_x += FONT_CELL_WIDTH;
        }
        if focused {
            let caret_x = self.x + Self::PAD + (column - self.scroll) as i32 * FONT_CELL_WIDTH;
            canvas.vertical_line(caret_x, self.y + 4, self.h.saturating_sub(8), text_color);
        }
    }
}

/// Result of routing a click or key into a [`ListView`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListEvent {
    /// Nothing happened (click outside the rows, no selection to move).
    None,
    /// The selection moved to this row.
    Selected(usize),
    /// This row was activated (Enter on a selection, or a second click on the
    /// already-selected row).
    Activated(usize),
}

/// A scrollable single-column list over `Vec<String>` rows.
///
/// Handles wheel scroll, click-to-select, keyboard selection movement, and
/// activation. The selected row is drawn inverted; a minimal scrollbar gutter
/// appears when the rows overflow the visible page.
pub struct ListView {
    pub rows: Vec<String>,
    pub first_row: usize,
    pub selected: Option<usize>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    scrollbar: Scrollbar,
}

impl ListView {
    pub const ROW_HEIGHT: u32 = 16;

    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            rows: Vec::new(),
            first_row: 0,
            selected: None,
            x,
            y,
            w,
            h,
            scrollbar: Scrollbar::new(Axis::Vertical, Rect::default()),
        }
    }

    pub fn set_rows(&mut self, rows: Vec<String>) {
        self.rows = rows;
        self.first_row = 0;
        self.selected = None;
    }

    pub fn visible_rows(&self) -> usize {
        (self.h / Self::ROW_HEIGHT).max(1) as usize
    }

    fn ensure_visible(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let visible = self.visible_rows();
        if selected < self.first_row {
            self.first_row = selected;
        } else if selected >= self.first_row + visible {
            self.first_row = selected + 1 - visible;
        }
    }

    pub fn scroll(&mut self, delta: i32) {
        if delta < 0 {
            self.first_row = self.first_row.saturating_sub((-delta) as usize);
        } else {
            let max_first = self.rows.len().saturating_sub(self.visible_rows());
            self.first_row = (self.first_row + delta as usize).min(max_first);
        }
        self.sync_scrollbar();
    }

    fn overflow(&self) -> bool {
        self.rows.len() > self.visible_rows()
    }

    fn scrollbar_bounds(&self) -> Rect {
        Rect::new(
            self.x + self.w as i32 - SCROLLBAR_THICKNESS as i32 - 2,
            self.y + 2,
            SCROLLBAR_THICKNESS,
            self.h.saturating_sub(4),
        )
    }

    fn sync_scrollbar(&mut self) {
        self.scrollbar.set_bounds(self.scrollbar_bounds());
        self.scrollbar.set_line_step(Self::ROW_HEIGHT);
        self.scrollbar.set_extents(
            self.rows.len() as u32 * Self::ROW_HEIGHT,
            self.visible_rows() as u32 * Self::ROW_HEIGHT,
        );
        self.scrollbar
            .set_offset(self.first_row as u32 * Self::ROW_HEIGHT);
        self.scrollbar.set_enabled(self.overflow());
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ListEvent> {
        self.sync_scrollbar();
        if self.overflow()
            && (self.scrollbar.is_captured() || self.scrollbar.bounds().contains(input.x, input.y))
        {
            let response = self.scrollbar.handle_pointer(input);
            self.first_row = (self.scrollbar.offset() / Self::ROW_HEIGHT) as usize;
            return ControlResponse {
                consumed: response.consumed,
                repaint: response.repaint || response.action.is_some(),
                action: None,
            };
        }
        match input.kind {
            PointerKind::Scroll { delta_y, .. } if self.hit(input.x, input.y) => {
                let before = self.first_row;
                self.scroll(delta_y);
                ControlResponse::consumed(before != self.first_row, None)
            }
            PointerKind::Down if self.hit(input.x, input.y) => {
                let event = self.click(input.x, input.y);
                ControlResponse {
                    consumed: event != ListEvent::None,
                    repaint: event != ListEvent::None,
                    action: (event != ListEvent::None).then_some(event),
                }
            }
            _ => ControlResponse::ignored(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = match self.selected {
            None => {
                if delta < 0 {
                    last
                } else {
                    0
                }
            }
            Some(current) => {
                if delta < 0 {
                    current.saturating_sub((-delta) as usize)
                } else {
                    (current + delta as usize).min(last)
                }
            }
        };
        self.selected = Some(next);
        self.ensure_visible();
    }

    /// Handle a key press. Returns [`ListEvent::Activated`] on Enter over a
    /// selected row, [`ListEvent::Selected`] on a movement, else `None`.
    pub fn key(&mut self, key: u32) -> ListEvent {
        let page = self.visible_rows() as isize;
        match key {
            runtime::KEY_UP => self.move_selection(-1),
            runtime::KEY_DOWN => self.move_selection(1),
            runtime::KEY_PAGE_UP => self.move_selection(-page),
            runtime::KEY_PAGE_DOWN => self.move_selection(page),
            runtime::KEY_HOME => self.move_selection(-(self.rows.len() as isize)),
            runtime::KEY_END => self.move_selection(self.rows.len() as isize),
            runtime::KEY_ENTER => {
                if let Some(selected) = self.selected {
                    return ListEvent::Activated(selected);
                }
                return ListEvent::None;
            }
            _ => return ListEvent::None,
        }
        self.selected
            .map(ListEvent::Selected)
            .unwrap_or(ListEvent::None)
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w as i32 && y >= self.y && y < self.y + self.h as i32
    }

    /// Route a click. A click on the already-selected row activates it
    /// (second-click idiom); a click on a different row selects it.
    pub fn click(&mut self, x: i32, y: i32) -> ListEvent {
        if !self.hit(x, y) {
            return ListEvent::None;
        }
        if self.overflow() && self.scrollbar_bounds().contains(x, y) {
            return ListEvent::None;
        }
        let row = self.first_row + ((y - self.y) / Self::ROW_HEIGHT as i32) as usize;
        if row >= self.rows.len() {
            return ListEvent::None;
        }
        if self.selected == Some(row) {
            ListEvent::Activated(row)
        } else {
            self.selected = Some(row);
            ListEvent::Selected(row)
        }
    }

    pub fn draw(&mut self, canvas: &mut Canvas) {
        self.sync_scrollbar();
        let palette = theme::palette();
        canvas.fill_rect(self.x, self.y, self.w, self.h, palette.field_bg);
        let visible = self.visible_rows();
        let overflow = self.rows.len() > visible;
        let gutter = if overflow {
            SCROLLBAR_THICKNESS as i32 + 2
        } else {
            0
        };
        let text_width = self.w as i32 - gutter - 4;
        let max_chars = (text_width / FONT_CELL_WIDTH).max(1) as usize;
        for slot in 0..visible {
            let row = self.first_row + slot;
            let Some(text) = self.rows.get(row) else {
                break;
            };
            let row_y = self.y + slot as i32 * Self::ROW_HEIGHT as i32;
            let selected = self.selected == Some(row);
            if selected {
                theme::draw_selection(
                    canvas,
                    self.x,
                    row_y,
                    self.w.saturating_sub(gutter as u32),
                    Self::ROW_HEIGHT,
                );
            }
            let fg = if selected {
                palette.selection_text
            } else {
                palette.field_text
            };
            let clipped: String = text.chars().take(max_chars).collect();
            canvas.draw_text(
                self.x + 4,
                row_y + (Self::ROW_HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                &clipped,
                fg,
            );
        }
        theme::draw_field_border(canvas, self.x, self.y, self.w, self.h, false);
        if overflow {
            self.scrollbar.draw(canvas);
        }
    }
}

/// Horizontal tab strip. Plain retained struct in the `MenuBar` idiom:
/// the host draws it, hit-tests clicks, and owns the active index.
pub struct TabBar {
    pub tabs: Vec<String>,
    pub active: usize,
    pub x: i32,
    pub y: i32,
    pub w: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarAction {
    Changed(usize),
}

impl TabBar {
    pub const HEIGHT: u32 = 26;
    const PAD: i32 = 12;

    pub fn new(x: i32, y: i32, w: u32, tabs: &[&str]) -> Self {
        Self {
            tabs: tabs.iter().map(|t| String::from(*t)).collect(),
            active: 0,
            x,
            y,
            w,
        }
    }

    fn tab_width(label: &str) -> i32 {
        label.chars().count() as i32 * FONT_CELL_WIDTH + Self::PAD * 2
    }

    /// Which tab a click at `(x, y)` lands on, if any.
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        if y < self.y || y >= self.y + Self::HEIGHT as i32 {
            return None;
        }
        let mut tab_x = self.x;
        for (i, label) in self.tabs.iter().enumerate() {
            let tw = Self::tab_width(label);
            if x >= tab_x && x < tab_x + tw {
                return Some(i);
            }
            tab_x += tw;
        }
        None
    }

    /// Advance to the next tab (Ctrl+Tab idiom); wraps.
    pub fn cycle(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    pub fn handle_input(&mut self, input: ControlInput) -> ControlResponse<TabBarAction> {
        let before = self.active;
        match input {
            ControlInput::Pointer(pointer) if matches!(pointer.kind, PointerKind::Down) => {
                let Some(tab) = self.hit(pointer.x, pointer.y) else {
                    return ControlResponse::ignored();
                };
                self.active = tab;
            }
            ControlInput::Key(key) if key.pressed && !self.tabs.is_empty() => match key.key {
                runtime::KEY_LEFT => self.active = self.active.saturating_sub(1),
                runtime::KEY_RIGHT => self.active = (self.active + 1).min(self.tabs.len() - 1),
                runtime::KEY_HOME => self.active = 0,
                runtime::KEY_END => self.active = self.tabs.len() - 1,
                _ => return ControlResponse::ignored(),
            },
            _ => return ControlResponse::ignored(),
        }
        let changed = before != self.active;
        ControlResponse::consumed(
            changed,
            changed.then_some(TabBarAction::Changed(self.active)),
        )
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let palette = theme::palette();
        canvas.fill_rect(self.x, self.y, self.w, Self::HEIGHT, palette.content_bg);
        canvas.horizontal_line(
            self.x,
            self.y + Self::HEIGHT as i32 - 1,
            self.w,
            palette.border,
        );
        let mut tab_x = self.x;
        for (i, label) in self.tabs.iter().enumerate() {
            let tw = Self::tab_width(label);
            let active = i == self.active;
            if active {
                // Active tab: raised white panel that merges into the
                // content area, with an accent line across its top.
                canvas.fill_rect(tab_x, self.y, tw as u32, Self::HEIGHT, palette.field_bg);
                canvas.fill_rect(tab_x, self.y, tw as u32, 2, palette.selection_bg);
                canvas.vertical_line(tab_x, self.y, Self::HEIGHT, palette.border);
                canvas.vertical_line(tab_x + tw - 1, self.y, Self::HEIGHT, palette.border);
            }
            let fg = if active {
                palette.text
            } else {
                palette.disabled_text
            };
            canvas.draw_text(
                tab_x + Self::PAD,
                self.y + (Self::HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                label,
                fg,
            );
            tab_x += tw;
        }
    }
}

/// One column of a [`ColumnListView`].
pub struct Column {
    pub title: String,
    pub width: u32,
    /// Sort this column by numeric value (leading integer/decimal)
    /// instead of lexicographically.
    pub numeric: bool,
}

impl Column {
    pub fn new(title: &str, width: u32) -> Self {
        Self {
            title: String::from(title),
            width,
            numeric: false,
        }
    }

    pub fn numeric(title: &str, width: u32) -> Self {
        Self {
            title: String::from(title),
            width,
            numeric: true,
        }
    }
}

/// One row of a [`ColumnListView`], identified by a caller-supplied
/// stable key (e.g. a PID) so refreshes and re-sorts never move the
/// user's selection to a different entity.
pub struct ColumnRow {
    pub key: u64,
    pub cells: Vec<String>,
    /// Render with muted text and refuse activation — view-only rows
    /// (e.g. kernel threads in a task list).
    pub dim: bool,
}

impl ColumnRow {
    pub fn new(key: u64, cells: Vec<String>) -> Self {
        Self {
            key,
            cells,
            dim: false,
        }
    }
}

/// Result of routing input into a [`ColumnListView`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColumnListEvent {
    None,
    /// Selection moved to the row with this key.
    Selected(u64),
    /// The row with this key was activated (Enter / second click).
    Activated(u64),
    /// A header click changed the sort column/direction.
    SortChanged,
}

/// Parse a numeric sort value from a cell: integer part scaled by
/// 1000 plus up to three fractional digits, ignoring one trailing
/// unit suffix ("12.5", "348 KB", "7"). Non-numeric cells sort as 0.
fn numeric_sort_value(s: &str) -> u64 {
    let s = s.trim_start();
    let mut int_part: u64 = 0;
    let mut it = s.chars().peekable();
    let mut saw_digit = false;
    while let Some(&c) = it.peek() {
        if let Some(d) = c.to_digit(10) {
            int_part = int_part.saturating_mul(10).saturating_add(d as u64);
            saw_digit = true;
            it.next();
        } else {
            break;
        }
    }
    if !saw_digit {
        return 0;
    }
    let mut frac: u64 = 0;
    if it.peek() == Some(&'.') {
        it.next();
        let mut scale = 100;
        while let Some(&c) = it.peek() {
            if let Some(d) = c.to_digit(10) {
                frac += d as u64 * scale;
                scale /= 10;
                it.next();
                if scale == 0 {
                    break;
                }
            } else {
                break;
            }
        }
    }
    int_part.saturating_mul(1000).saturating_add(frac)
}

/// A scrollable, sortable multi-column list. Same selection/scroll/
/// activation semantics as [`ListView`], plus a clickable header row
/// that sorts by column (toggling ascending/descending) and key-stable
/// selection across refreshes.
pub struct ColumnListView {
    pub columns: Vec<Column>,
    pub rows: Vec<ColumnRow>,
    pub first_row: usize,
    pub selected_key: Option<u64>,
    pub sort_col: usize,
    pub sort_desc: bool,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    scrollbar: Scrollbar,
}

impl ColumnListView {
    pub const ROW_HEIGHT: u32 = 16;
    pub const HEADER_HEIGHT: u32 = 18;

    pub fn new(x: i32, y: i32, w: u32, h: u32, columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
            first_row: 0,
            selected_key: None,
            sort_col: 0,
            sort_desc: false,
            x,
            y,
            w,
            h,
            scrollbar: Scrollbar::new(Axis::Vertical, Rect::default()),
        }
    }

    /// Replace the rows, re-apply the current sort, and keep the
    /// selection pinned to its key (dropping it if the key vanished).
    pub fn set_rows(&mut self, rows: Vec<ColumnRow>) {
        self.rows = rows;
        self.apply_sort();
        if let Some(key) = self.selected_key {
            if !self.rows.iter().any(|r| r.key == key) {
                self.selected_key = None;
            }
        }
        let max_first = self.rows.len().saturating_sub(self.visible_rows());
        self.first_row = self.first_row.min(max_first);
    }

    fn apply_sort(&mut self) {
        let col = self.sort_col;
        let numeric = self.columns.get(col).map(|c| c.numeric).unwrap_or(false);
        let desc = self.sort_desc;
        self.rows.sort_by(|a, b| {
            // Dim (view-only) rows always sink below regular rows.
            let group = a.dim.cmp(&b.dim);
            if group != core::cmp::Ordering::Equal {
                return group;
            }
            let av = a.cells.get(col).map(String::as_str).unwrap_or("");
            let bv = b.cells.get(col).map(String::as_str).unwrap_or("");
            let ord = if numeric {
                numeric_sort_value(av).cmp(&numeric_sort_value(bv))
            } else {
                av.cmp(bv)
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }

    /// Sort by `col`, toggling direction when it is already the sort
    /// column.
    pub fn sort_by_column(&mut self, col: usize) {
        if col >= self.columns.len() {
            return;
        }
        if self.sort_col == col {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_col = col;
            // Numeric columns (CPU, memory) are most useful descending.
            self.sort_desc = self.columns[col].numeric;
        }
        self.apply_sort();
        self.ensure_visible();
    }

    pub fn visible_rows(&self) -> usize {
        ((self.h.saturating_sub(Self::HEADER_HEIGHT)) / Self::ROW_HEIGHT).max(1) as usize
    }

    fn selected_index(&self) -> Option<usize> {
        let key = self.selected_key?;
        self.rows.iter().position(|r| r.key == key)
    }

    /// The selected row, if any.
    pub fn selected_row(&self) -> Option<&ColumnRow> {
        self.selected_index().map(|i| &self.rows[i])
    }

    fn ensure_visible(&mut self) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let visible = self.visible_rows();
        if index < self.first_row {
            self.first_row = index;
        } else if index >= self.first_row + visible {
            self.first_row = index + 1 - visible;
        }
    }

    pub fn scroll(&mut self, delta: i32) {
        if delta < 0 {
            self.first_row = self.first_row.saturating_sub((-delta) as usize);
        } else {
            let max_first = self.rows.len().saturating_sub(self.visible_rows());
            self.first_row = (self.first_row + delta as usize).min(max_first);
        }
        self.sync_scrollbar();
    }

    fn overflow(&self) -> bool {
        self.rows.len() > self.visible_rows()
    }

    fn scrollbar_bounds(&self) -> Rect {
        Rect::new(
            self.x + self.w as i32 - SCROLLBAR_THICKNESS as i32 - 2,
            self.y + Self::HEADER_HEIGHT as i32,
            SCROLLBAR_THICKNESS,
            self.h.saturating_sub(Self::HEADER_HEIGHT).saturating_sub(2),
        )
    }

    fn sync_scrollbar(&mut self) {
        self.scrollbar.set_bounds(self.scrollbar_bounds());
        self.scrollbar.set_line_step(Self::ROW_HEIGHT);
        self.scrollbar.set_extents(
            self.rows.len() as u32 * Self::ROW_HEIGHT,
            self.visible_rows() as u32 * Self::ROW_HEIGHT,
        );
        self.scrollbar
            .set_offset(self.first_row as u32 * Self::ROW_HEIGHT);
        self.scrollbar.set_enabled(self.overflow());
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ColumnListEvent> {
        self.sync_scrollbar();
        if self.overflow()
            && (self.scrollbar.is_captured() || self.scrollbar.bounds().contains(input.x, input.y))
        {
            let response = self.scrollbar.handle_pointer(input);
            self.first_row = (self.scrollbar.offset() / Self::ROW_HEIGHT) as usize;
            return ControlResponse {
                consumed: response.consumed,
                repaint: response.repaint || response.action.is_some(),
                action: None,
            };
        }
        match input.kind {
            PointerKind::Scroll { delta_y, .. } if self.hit(input.x, input.y) => {
                let before = self.first_row;
                self.scroll(delta_y);
                ControlResponse::consumed(before != self.first_row, None)
            }
            PointerKind::Down if self.hit(input.x, input.y) => {
                let event = self.click(input.x, input.y);
                ControlResponse {
                    consumed: event != ColumnListEvent::None,
                    repaint: event != ColumnListEvent::None,
                    action: (event != ColumnListEvent::None).then_some(event),
                }
            }
            _ => ControlResponse::ignored(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = match self.selected_index() {
            None => {
                if delta < 0 {
                    last
                } else {
                    0
                }
            }
            Some(current) => {
                if delta < 0 {
                    current.saturating_sub((-delta) as usize)
                } else {
                    (current + delta as usize).min(last)
                }
            }
        };
        self.selected_key = Some(self.rows[next].key);
        self.ensure_visible();
    }

    pub fn key(&mut self, key: u32) -> ColumnListEvent {
        let page = self.visible_rows() as isize;
        match key {
            runtime::KEY_UP => self.move_selection(-1),
            runtime::KEY_DOWN => self.move_selection(1),
            runtime::KEY_PAGE_UP => self.move_selection(-page),
            runtime::KEY_PAGE_DOWN => self.move_selection(page),
            runtime::KEY_HOME => self.move_selection(-(self.rows.len() as isize)),
            runtime::KEY_END => self.move_selection(self.rows.len() as isize),
            runtime::KEY_ENTER => {
                if let Some(row) = self.selected_row() {
                    if !row.dim {
                        return ColumnListEvent::Activated(row.key);
                    }
                }
                return ColumnListEvent::None;
            }
            _ => return ColumnListEvent::None,
        }
        self.selected_key
            .map(ColumnListEvent::Selected)
            .unwrap_or(ColumnListEvent::None)
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w as i32 && y >= self.y && y < self.y + self.h as i32
    }

    /// Route a click: header row sorts, a body click selects, a second
    /// click on the selected (non-dim) row activates.
    pub fn click(&mut self, x: i32, y: i32) -> ColumnListEvent {
        if !self.hit(x, y) {
            return ColumnListEvent::None;
        }
        if self.overflow() && self.scrollbar_bounds().contains(x, y) {
            return ColumnListEvent::None;
        }
        if y < self.y + Self::HEADER_HEIGHT as i32 {
            let mut col_x = self.x;
            for (i, column) in self.columns.iter().enumerate() {
                let next_x = col_x + column.width as i32;
                if x >= col_x && x < next_x {
                    self.sort_by_column(i);
                    return ColumnListEvent::SortChanged;
                }
                col_x = next_x;
            }
            return ColumnListEvent::None;
        }
        let body_y = y - self.y - Self::HEADER_HEIGHT as i32;
        let row = self.first_row + (body_y / Self::ROW_HEIGHT as i32) as usize;
        if row >= self.rows.len() {
            return ColumnListEvent::None;
        }
        let key = self.rows[row].key;
        if self.selected_key == Some(key) {
            if self.rows[row].dim {
                ColumnListEvent::None
            } else {
                ColumnListEvent::Activated(key)
            }
        } else {
            self.selected_key = Some(key);
            ColumnListEvent::Selected(key)
        }
    }

    pub fn draw(&mut self, canvas: &mut Canvas) {
        self.sync_scrollbar();
        let palette = theme::palette();
        canvas.fill_rect(self.x, self.y, self.w, self.h, palette.field_bg);
        // Header.
        canvas.fill_rect(
            self.x,
            self.y,
            self.w,
            Self::HEADER_HEIGHT,
            palette.content_bg,
        );
        let visible = self.visible_rows();
        let overflow = self.rows.len() > visible;
        let gutter: i32 = if overflow {
            SCROLLBAR_THICKNESS as i32 + 2
        } else {
            0
        };
        let mut col_x = self.x;
        for (i, column) in self.columns.iter().enumerate() {
            let max_chars = ((column.width as i32 - 8) / FONT_CELL_WIDTH).max(1) as usize;
            let mut title: String = column.title.chars().take(max_chars).collect();
            if i == self.sort_col && !title.is_empty() {
                // Replace the last visible char with a sort arrow when
                // the title fills the column; append otherwise.
                if title.chars().count() >= max_chars {
                    title.pop();
                }
                title.push(if self.sort_desc { 'v' } else { '^' });
            }
            canvas.draw_text(
                col_x + 4,
                self.y + (Self::HEADER_HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                &title,
                palette.text,
            );
            col_x += column.width as i32;
            if col_x >= self.x + self.w as i32 {
                break;
            }
            canvas.vertical_line(col_x - 1, self.y, self.h, palette.border);
        }
        canvas.horizontal_line(
            self.x,
            self.y + Self::HEADER_HEIGHT as i32 - 1,
            self.w,
            palette.border,
        );
        // Body.
        for slot in 0..visible {
            let row_index = self.first_row + slot;
            let Some(row) = self.rows.get(row_index) else {
                break;
            };
            let row_y = self.y + Self::HEADER_HEIGHT as i32 + slot as i32 * Self::ROW_HEIGHT as i32;
            let selected = self.selected_key == Some(row.key);
            let (bg, fg) = if selected {
                (palette.selection_bg, palette.selection_text)
            } else if row.dim {
                (palette.field_bg, palette.disabled_text)
            } else {
                (palette.field_bg, palette.field_text)
            };
            if bg != palette.field_bg {
                canvas.fill_rect(self.x, row_y, self.w - gutter as u32, Self::ROW_HEIGHT, bg);
            }
            let mut cell_x = self.x;
            for (i, column) in self.columns.iter().enumerate() {
                let Some(cell) = row.cells.get(i) else {
                    break;
                };
                let max_chars = ((column.width as i32 - 8) / FONT_CELL_WIDTH).max(1) as usize;
                let clipped: String = cell.chars().take(max_chars).collect();
                canvas.draw_text(
                    cell_x + 4,
                    row_y + (Self::ROW_HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                    &clipped,
                    fg,
                );
                cell_x += column.width as i32;
                if cell_x >= self.x + self.w as i32 {
                    break;
                }
            }
        }
        theme::draw_field_border(canvas, self.x, self.y, self.w, self.h, false);
        // Scrollbar gutter.
        if overflow {
            self.scrollbar.draw(canvas);
        }
    }
}

/// A fixed-capacity time-series area chart with up to two series.
///
/// Samples push in from the right; the ring holds the last
/// `capacity` samples. Series A draws as a filled area with a 1 px
/// top line in the accent color; series B (when pushed) draws as a
/// line in green. Y scale is either fixed (percent graphs) or
/// autoscaling to the observed maximum (throughput graphs).
pub struct TimeSeriesGraph {
    pub capacity: usize,
    a: Vec<f32>,
    b: Vec<f32>,
    /// `Some(max)` pins the y-axis (e.g. 100.0 for percent);
    /// `None` autoscales to the observed maximum.
    pub fixed_max: Option<f32>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl TimeSeriesGraph {
    pub fn new(x: i32, y: i32, w: u32, h: u32, capacity: usize, fixed_max: Option<f32>) -> Self {
        Self {
            capacity: capacity.max(2),
            a: Vec::new(),
            b: Vec::new(),
            fixed_max,
            x,
            y,
            w,
            h,
        }
    }

    /// Push one sample per series. Passing `None` for `b` keeps the
    /// graph single-series.
    pub fn push(&mut self, a: f32, b: Option<f32>) {
        self.a.push(a.max(0.0));
        if self.a.len() > self.capacity {
            self.a.remove(0);
        }
        if let Some(b) = b {
            self.b.push(b.max(0.0));
            if self.b.len() > self.capacity {
                self.b.remove(0);
            }
        }
    }

    /// Latest sample of series A, if any.
    pub fn latest(&self) -> Option<f32> {
        self.a.last().copied()
    }

    fn y_max(&self) -> f32 {
        if let Some(max) = self.fixed_max {
            return max.max(1.0);
        }
        let mut max = 1.0f32;
        for &v in self.a.iter().chain(self.b.iter()) {
            if v > max {
                max = v;
            }
        }
        max * 1.1
    }

    pub fn draw(&self, canvas: &mut Canvas, title: &str, value_label: &str) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, COLOR_WHITE);
        let plot_x = self.x + 1;
        let plot_y = self.y + 1;
        let plot_w = self.w.saturating_sub(2) as i32;
        let plot_h = self.h.saturating_sub(2) as i32;
        // Gridlines at 25/50/75 %.
        for q in 1..4 {
            let gy = plot_y + plot_h * q / 4;
            canvas.horizontal_line(plot_x, gy, plot_w as u32, 0xEAEAEA);
        }
        let max = self.y_max();
        let scale = |v: f32| -> i32 {
            let clamped = if v > max { max } else { v };
            let px = (clamped / max * (plot_h - 1) as f32) as i32;
            plot_y + (plot_h - 1) - px
        };
        // Sample index for a pixel column: rightmost column = newest
        // sample; a partially-filled ring occupies the right edge.
        let sample_at = |series: &[f32], col: i32| -> Option<f32> {
            if series.is_empty() {
                return None;
            }
            let slot =
                (col as i64 * (self.capacity as i64 - 1) / (plot_w as i64 - 1).max(1)) as isize;
            let offset = self.capacity as isize - series.len() as isize;
            let index = slot - offset;
            if index < 0 {
                None
            } else {
                series.get(index as usize).copied()
            }
        };
        // Series A: filled area + top line.
        for col in 0..plot_w {
            if let Some(v) = sample_at(&self.a, col) {
                let top = scale(v);
                let bottom = plot_y + plot_h - 1;
                if bottom > top {
                    canvas.vertical_line(
                        plot_x + col,
                        top + 1,
                        (bottom - top) as u32,
                        COLOR_ACCENT_FILL,
                    );
                }
                canvas.pixel(plot_x + col, top, COLOR_HIGHLIGHT);
                if let Some(prev) = sample_at(&self.a, col - 1) {
                    // Join vertical gaps between adjacent columns so
                    // steep changes stay a connected line.
                    let prev_top = scale(prev);
                    let (lo, hi) = if prev_top < top {
                        (prev_top, top)
                    } else {
                        (top, prev_top)
                    };
                    if hi > lo + 1 {
                        canvas.vertical_line(
                            plot_x + col,
                            lo + 1,
                            (hi - lo - 1) as u32,
                            COLOR_HIGHLIGHT,
                        );
                    }
                }
            }
        }
        // Series B: line only.
        for col in 0..plot_w {
            if let Some(v) = sample_at(&self.b, col) {
                let top = scale(v);
                canvas.pixel(plot_x + col, top, COLOR_ACCENT2);
                if let Some(prev) = sample_at(&self.b, col - 1) {
                    let prev_top = scale(prev);
                    let (lo, hi) = if prev_top < top {
                        (prev_top, top)
                    } else {
                        (top, prev_top)
                    };
                    if hi > lo + 1 {
                        canvas.vertical_line(
                            plot_x + col,
                            lo + 1,
                            (hi - lo - 1) as u32,
                            COLOR_ACCENT2,
                        );
                    }
                }
            }
        }
        canvas.rect(self.x, self.y, self.w, self.h, COLOR_BORDER);
        canvas.draw_text(self.x + 6, self.y + 5, title, COLOR_TEXT);
        let label_w = value_label.chars().count() as i32 * FONT_CELL_WIDTH;
        canvas.draw_text(
            self.x + self.w as i32 - label_w - 6,
            self.y + 5,
            value_label,
            COLOR_TEXT,
        );
    }
}

#[derive(Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
    pub mode: u32,
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
                    let mut full_path = String::from(directory.trim_end_matches('/'));
                    if full_path.is_empty() {
                        full_path.push('/');
                    } else {
                        full_path.push('/');
                    }
                    full_path.push_str(&name);
                    let mut stat = runtime::LinuxStat::default();
                    let stat_path = c_path(&full_path);
                    let stat_result =
                        runtime::newfstatat(runtime::AT_FDCWD, &stat_path, &mut stat, 0);
                    output.push(DirEntry {
                        name,
                        is_dir,
                        size: if stat_result == 0 && stat.st_size > 0 {
                            stat.st_size as u64
                        } else {
                            0
                        },
                        modified: if stat_result == 0 { stat.st_mtime } else { 0 },
                        mode: if stat_result == 0 { stat.st_mode } else { 0 },
                    });
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

#[cfg(test)]
mod canvas_tests {
    use super::Canvas;

    #[test]
    fn borrowed_canvas_matches_owned_drawing() {
        let mut owned = Canvas::new(40, 24);
        owned.clear(0x112233);
        owned.fill_rect(3, 4, 17, 9, 0xabcdef);
        owned.draw_text(1, 1, "Native", 0x010203);

        let mut pixels = alloc::vec![0; 40 * 24];
        let mut borrowed = unsafe { Canvas::from_borrowed(40, 24, pixels.as_mut_ptr()).unwrap() };
        borrowed.clear(0x112233);
        borrowed.fill_rect(3, 4, 17, 9, 0xabcdef);
        borrowed.draw_text(1, 1, "Native", 0x010203);

        assert!(borrowed.is_borrowed());
        assert_eq!(owned.pixels(), borrowed.pixels());
    }

    #[test]
    fn borrowed_canvas_rejects_null_storage() {
        assert!(unsafe { Canvas::from_borrowed(1, 1, core::ptr::null_mut()) }.is_none());
    }
}
