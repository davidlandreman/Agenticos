use alloc::string::String;

use crate::{theme, Canvas, FONT_LINE_HEIGHT};
use gui_core::{ControlResponse, KeyInput, PointerInput, PointerKind, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleAction {
    Changed(bool),
}

struct ToggleState {
    label: String,
    bounds: Rect,
    checked: bool,
    enabled: bool,
    hot: bool,
    pressed: bool,
}

impl ToggleState {
    fn new(label: &str, bounds: Rect, checked: bool) -> Self {
        Self {
            label: String::from(label),
            bounds,
            checked,
            enabled: true,
            hot: false,
            pressed: false,
        }
    }

    fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ToggleAction> {
        match input.kind {
            PointerKind::Move => {
                let hot = self.enabled && self.bounds.contains(input.x, input.y);
                let repaint = hot != self.hot;
                self.hot = hot;
                ControlResponse {
                    consumed: hot || self.pressed,
                    repaint,
                    action: None,
                }
            }
            PointerKind::Down if self.enabled && self.bounds.contains(input.x, input.y) => {
                self.hot = true;
                self.pressed = true;
                ControlResponse::consumed(true, None)
            }
            PointerKind::Up if self.pressed => {
                let activate = self.enabled && self.bounds.contains(input.x, input.y);
                self.pressed = false;
                if activate {
                    self.checked = !self.checked;
                }
                ControlResponse::consumed(
                    true,
                    activate.then_some(ToggleAction::Changed(self.checked)),
                )
            }
            PointerKind::Cancel => {
                let repaint = self.hot || self.pressed;
                self.hot = false;
                self.pressed = false;
                ControlResponse::consumed(repaint, None)
            }
            _ => ControlResponse::ignored(),
        }
    }

    fn handle_key(&mut self, input: KeyInput, focused: bool) -> ControlResponse<ToggleAction> {
        if self.enabled && focused && input.pressed && input.character == ' ' {
            self.checked = !self.checked;
            ControlResponse::consumed(true, Some(ToggleAction::Changed(self.checked)))
        } else {
            ControlResponse::ignored()
        }
    }
}

pub struct CheckBox(ToggleState);

impl CheckBox {
    pub fn new(label: &str, bounds: Rect, checked: bool) -> Self {
        Self(ToggleState::new(label, bounds, checked))
    }

    pub fn checked(&self) -> bool {
        self.0.checked
    }

    pub fn set_checked(&mut self, checked: bool) {
        self.0.checked = checked;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.0.enabled = enabled;
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ToggleAction> {
        self.0.handle_pointer(input)
    }

    pub fn handle_key(&mut self, input: KeyInput, focused: bool) -> ControlResponse<ToggleAction> {
        self.0.handle_key(input, focused)
    }

    pub fn draw(&self, canvas: &mut Canvas, focused: bool) {
        let palette = theme::palette();
        let box_size = 16u32;
        let box_y = self.0.bounds.y + (self.0.bounds.h as i32 - box_size as i32) / 2;
        theme::draw_field(canvas, self.0.bounds.x, box_y, box_size, box_size, focused);
        if self.0.checked {
            canvas.draw_text(self.0.bounds.x + 3, box_y - 1, "✓", palette.field_text);
        }
        canvas.draw_text(
            self.0.bounds.x + 22,
            self.0.bounds.y + (self.0.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
            &self.0.label,
            if self.0.enabled {
                palette.text
            } else {
                palette.disabled_text
            },
        );
    }
}

pub struct RadioButton {
    inner: ToggleState,
}

impl RadioButton {
    pub fn new(label: &str, bounds: Rect, checked: bool) -> Self {
        Self {
            inner: ToggleState::new(label, bounds, checked),
        }
    }

    pub fn checked(&self) -> bool {
        self.inner.checked
    }

    pub fn set_checked(&mut self, checked: bool) {
        self.inner.checked = checked;
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ToggleAction> {
        let was_checked = self.inner.checked;
        let mut response = self.inner.handle_pointer(input);
        if let Some(ToggleAction::Changed(_)) = response.action {
            self.inner.checked = true;
            response.action = (!was_checked).then_some(ToggleAction::Changed(true));
        }
        response
    }

    pub fn handle_key(&mut self, input: KeyInput, focused: bool) -> ControlResponse<ToggleAction> {
        let was_checked = self.inner.checked;
        let mut response = self.inner.handle_key(input, focused);
        if let Some(ToggleAction::Changed(_)) = response.action {
            self.inner.checked = true;
            response.action = (!was_checked).then_some(ToggleAction::Changed(true));
        }
        response
    }

    pub fn draw(&self, canvas: &mut Canvas, focused: bool) {
        let palette = theme::palette();
        let size = 16u32;
        let y = self.inner.bounds.y + (self.inner.bounds.h as i32 - size as i32) / 2;
        canvas.fill_rect(
            self.inner.bounds.x + 3,
            y,
            size - 6,
            size,
            palette.content_bg,
        );
        canvas.fill_rect(
            self.inner.bounds.x,
            y + 3,
            size,
            size - 6,
            palette.content_bg,
        );
        canvas.rect(
            self.inner.bounds.x + 2,
            y + 2,
            size - 4,
            size - 4,
            palette.border,
        );
        if self.inner.checked {
            canvas.fill_rect(self.inner.bounds.x + 6, y + 6, 4, 4, palette.field_text);
        }
        if focused {
            canvas.rect(self.inner.bounds.x, y, size, size, palette.selection_bg);
        }
        canvas.draw_text(
            self.inner.bounds.x + 22,
            self.inner.bounds.y + (self.inner.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
            &self.inner.label,
            if self.inner.enabled {
                palette.text
            } else {
                palette.disabled_text
            },
        );
    }
}

pub struct ProgressBar {
    pub bounds: Rect,
    value: u64,
    maximum: u64,
}

impl ProgressBar {
    pub fn new(bounds: Rect, maximum: u64) -> Self {
        Self {
            bounds,
            value: 0,
            maximum: maximum.max(1),
        }
    }

    pub fn set(&mut self, value: u64, maximum: u64) {
        self.maximum = maximum.max(1);
        self.value = value.min(self.maximum);
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let palette = theme::palette();
        theme::draw_field(
            canvas,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            false,
        );
        let inner = self.bounds.inset(2);
        let filled = ((inner.w as u64).saturating_mul(self.value) / self.maximum) as u32;
        canvas.fill_rect(inner.x, inner.y, filled, inner.h, palette.selection_bg);
    }
}

pub struct Toolbar {
    pub bounds: Rect,
}

impl Toolbar {
    pub fn draw(&self, canvas: &mut Canvas) {
        let palette = theme::palette();
        canvas.fill_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            palette.content_bg,
        );
        canvas.horizontal_line(
            self.bounds.x,
            self.bounds.bottom() - 1,
            self.bounds.w,
            palette.border,
        );
    }
}

pub struct StatusBar {
    pub bounds: Rect,
    text: String,
}

impl StatusBar {
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            text: String::new(),
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let palette = theme::palette();
        theme::draw_status_bar_surface(canvas, self.bounds);
        canvas.draw_text(
            self.bounds.x + 6,
            self.bounds.y + (self.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
            &self.text,
            palette.text,
        );
    }
}
