use gui_core::{ControlResponse, KeyInput, PointerInput, PointerKind, Rect};

use crate::{theme, Canvas};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderAction {
    Changed(u32),
}

pub struct Slider {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    min: u32,
    max: u32,
    value: u32,
    step: u32,
    enabled: bool,
    dragging: bool,
    hot: bool,
}

impl Slider {
    pub fn new(x: i32, y: i32, w: u32, h: u32, min: u32, max: u32, value: u32) -> Self {
        let max = max.max(min);
        Self {
            x,
            y,
            w,
            h,
            min,
            max,
            value: value.clamp(min, max),
            step: 1,
            enabled: true,
            dragging: false,
            hot: false,
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }

    pub fn value(&self) -> u32 {
        self.value
    }

    pub fn set_value(&mut self, value: u32) -> bool {
        let value = value.clamp(self.min, self.max);
        if value == self.value {
            false
        } else {
            self.value = value;
            true
        }
    }

    pub fn set_step(&mut self, step: u32) {
        self.step = step.max(1);
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.cancel();
        }
    }

    pub fn cancel(&mut self) {
        self.dragging = false;
        self.hot = false;
    }

    fn knob_width(&self) -> u32 {
        self.h.clamp(5, 10)
    }

    fn track(&self) -> Rect {
        let knob = self.knob_width();
        Rect::new(
            self.x + knob as i32 / 2,
            self.y + self.h as i32 / 2 - 2,
            self.w.saturating_sub(knob),
            4,
        )
    }

    fn knob(&self) -> Rect {
        let knob = self.knob_width();
        let track = self.track();
        let range = self.max.saturating_sub(self.min);
        let offset = if range == 0 {
            0
        } else {
            ((self.value - self.min) as u64 * track.w as u64 / range as u64) as i32
        };
        Rect::new(track.x + offset - knob as i32 / 2, self.y, knob, self.h)
    }

    fn value_at(&self, x: i32) -> u32 {
        let track = self.track();
        if track.w == 0 || self.max == self.min {
            return self.min;
        }
        let pixel = x.saturating_sub(track.x).clamp(0, track.w as i32) as u32;
        self.min + ((pixel as u64 * (self.max - self.min) as u64) / track.w as u64) as u32
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<SliderAction> {
        match input.kind {
            PointerKind::Cancel => {
                let repaint = self.dragging || self.hot;
                self.cancel();
                ControlResponse::consumed(repaint, None)
            }
            PointerKind::Move => {
                if self.dragging {
                    let changed = self.set_value(self.value_at(input.x));
                    return ControlResponse::consumed(
                        changed,
                        changed.then_some(SliderAction::Changed(self.value)),
                    );
                }
                let hot = self.bounds().contains(input.x, input.y);
                let repaint = hot != self.hot;
                self.hot = hot;
                ControlResponse {
                    consumed: hot,
                    repaint,
                    action: None,
                }
            }
            PointerKind::Down if self.enabled && self.bounds().contains(input.x, input.y) => {
                self.dragging = true;
                self.hot = true;
                let changed = self.set_value(self.value_at(input.x));
                ControlResponse::consumed(
                    true,
                    changed.then_some(SliderAction::Changed(self.value)),
                )
            }
            PointerKind::Up if self.dragging => {
                self.dragging = false;
                self.hot = self.bounds().contains(input.x, input.y);
                ControlResponse::consumed(true, None)
            }
            _ => ControlResponse::ignored(),
        }
    }

    pub fn handle_key(&mut self, input: KeyInput) -> ControlResponse<SliderAction> {
        if !self.enabled || !input.pressed {
            return ControlResponse::ignored();
        }
        let next = match input.key {
            runtime::KEY_LEFT | runtime::KEY_DOWN => self.value.saturating_sub(self.step),
            runtime::KEY_RIGHT | runtime::KEY_UP => self.value.saturating_add(self.step),
            runtime::KEY_HOME => self.min,
            runtime::KEY_END => self.max,
            _ => return ControlResponse::ignored(),
        };
        let changed = self.set_value(next);
        ControlResponse::consumed(
            changed,
            changed.then_some(SliderAction::Changed(self.value)),
        )
    }

    pub fn draw(&self, canvas: &mut Canvas, focused: bool) {
        let palette = theme::palette();
        let track = self.track();
        canvas.fill_rect(track.x, track.y, track.w, track.h, palette.border);
        let knob = self.knob();
        theme::draw_scrollbar_part(
            canvas,
            knob,
            self.enabled,
            self.hot || focused,
            self.dragging,
        );
    }
}
