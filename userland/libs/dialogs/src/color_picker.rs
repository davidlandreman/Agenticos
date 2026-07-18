//! Modal RGB color picker with a swatch grid and draggable channel sliders.

use alloc::format;

use gui::{Button, Window, COLOR_BORDER, COLOR_PANEL, COLOR_TEXT, COLOR_WHITE};

use crate::DialogStatus;

const MARGIN: i32 = 14;
const SWATCH_COLS: usize = 8;
const SWATCH_ROWS: usize = 5;
const SWATCH_SIZE: i32 = 26;
const SWATCH_GAP: i32 = 4;
const SLIDER_H: i32 = 18;

/// A curated 8×5 palette. Mirrors common desktop colors for continuity.
const PALETTE: [u32; SWATCH_COLS * SWATCH_ROWS] = [
    0x000000, 0x404040, 0x808080, 0xA0A0A0, 0xC0C0C0, 0xE0E0E0, 0xF0F0F0, 0xFFFFFF, //
    0x400000, 0x800000, 0xA03020, 0xFF0000, 0xFF4040, 0xFF8080, 0xFFC0C0, 0xFFE0E0, //
    0x804000, 0xA0522D, 0xFF8000, 0xFFA030, 0xFFC000, 0xFFE000, 0xFFFF00, 0xFFFFC0, //
    0x004000, 0x008000, 0x20A020, 0x00C000, 0x00FF00, 0x80FF80, 0x008080, 0x00FFFF, //
    0x000040, 0x000080, 0x0000FF, 0x0078D7, 0x203050, 0x4040FF, 0x8000FF, 0xFF00FF, //
];

/// A modal color picker returning an XRGB8888 value (`0x00RRGGBB`).
///
/// A swatch grid loads presets into the R/G/B sliders; sliders (click or drag)
/// allow arbitrary colors. A live preview swatch shows the hex value. This is
/// the reference consumer of mouse-move-with-button-held drag.
pub struct ColorPicker {
    window: Window,
    r: u8,
    g: u8,
    b: u8,
    active_slider: Option<usize>,
    ok: Button,
    cancel: Button,
}

impl ColorPicker {
    pub fn new(initial: u32) -> Result<Self, i64> {
        let window = Window::new(360, 320, "Choose Color")?;
        let mut dialog = Self {
            window,
            r: (initial >> 16) as u8,
            g: (initial >> 8) as u8,
            b: initial as u8,
            active_slider: None,
            ok: Button::new("OK", 0, 0, 88, 26),
            cancel: Button::new("Cancel", 0, 0, 88, 26),
        };
        dialog.relayout();
        dialog.render();
        Ok(dialog)
    }

    pub fn window_handle(&self) -> u32 {
        self.window.handle()
    }

    fn value(&self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    fn grid_origin() -> (i32, i32) {
        (MARGIN, MARGIN)
    }

    fn slider_track(&self, index: usize) -> (i32, i32, i32) {
        let (_, grid_y) = Self::grid_origin();
        let grid_bottom =
            grid_y + SWATCH_ROWS as i32 * (SWATCH_SIZE + SWATCH_GAP) + 12;
        let x = MARGIN + 24;
        let track_w = self.window.canvas().width() as i32 - x - MARGIN - 44;
        let y = grid_bottom + index as i32 * (SLIDER_H + 10);
        (x, y, track_w.max(16))
    }

    fn relayout(&mut self) {
        let width = self.window.canvas().width().max(320) as i32;
        let height = self.window.canvas().height().max(260) as i32;
        let button_y = height - MARGIN - 26;
        self.cancel.x = width - MARGIN - self.cancel.w as i32;
        self.cancel.y = button_y;
        self.ok.x = self.cancel.x - 12 - self.ok.w as i32;
        self.ok.y = button_y;
    }

    fn channel(&mut self, index: usize) -> &mut u8 {
        match index {
            0 => &mut self.r,
            1 => &mut self.g,
            _ => &mut self.b,
        }
    }

    fn set_channel_from_x(&mut self, index: usize, x: i32) {
        let (track_x, _, track_w) = self.slider_track(index);
        let value = (((x - track_x).clamp(0, track_w) * 255) / track_w) as u8;
        *self.channel(index) = value;
    }

    fn render(&mut self) {
        let r = self.r;
        let g = self.g;
        let b = self.b;
        let value = self.value();
        let hex = format!("#{r:02X}{g:02X}{b:02X}");
        let tracks = [
            self.slider_track(0),
            self.slider_track(1),
            self.slider_track(2),
        ];
        let (grid_x, grid_y) = Self::grid_origin();
        let width = self.window.canvas().width() as i32;
        let canvas = self.window.canvas_mut();
        canvas.clear(COLOR_PANEL);

        // Swatch grid.
        for (index, color) in PALETTE.iter().enumerate() {
            let col = (index % SWATCH_COLS) as i32;
            let row = (index / SWATCH_COLS) as i32;
            let x = grid_x + col * (SWATCH_SIZE + SWATCH_GAP);
            let y = grid_y + row * (SWATCH_SIZE + SWATCH_GAP);
            canvas.fill_rect(x, y, SWATCH_SIZE as u32, SWATCH_SIZE as u32, *color);
            canvas.rect(x, y, SWATCH_SIZE as u32, SWATCH_SIZE as u32, COLOR_BORDER);
        }

        // R/G/B sliders.
        let labels = ["R", "G", "B"];
        let values = [r, g, b];
        for index in 0..3 {
            let (track_x, track_y, track_w) = tracks[index];
            canvas.draw_text(MARGIN, track_y + (SLIDER_H - 8) / 2, labels[index], COLOR_TEXT);
            canvas.fill_rect(track_x, track_y, track_w as u32, SLIDER_H as u32, COLOR_WHITE);
            canvas.rect(track_x, track_y, track_w as u32, SLIDER_H as u32, COLOR_BORDER);
            let knob_x = track_x + (values[index] as i32 * track_w) / 255;
            canvas.fill_rect(knob_x - 2, track_y - 2, 5, SLIDER_H as u32 + 4, COLOR_TEXT);
            let text = format!("{}", values[index]);
            canvas.draw_text(track_x + track_w + 8, track_y + (SLIDER_H - 8) / 2, &text, COLOR_TEXT);
        }

        // Preview swatch + hex.
        let preview_y = tracks[2].1 + SLIDER_H + 14;
        canvas.fill_rect(MARGIN, preview_y, 64, 40, value);
        canvas.rect(MARGIN, preview_y, 64, 40, COLOR_BORDER);
        canvas.draw_text(MARGIN + 76, preview_y + 16, &hex, COLOR_TEXT);
        let _ = width;

        self.ok.draw(canvas, true);
        self.cancel.draw(canvas, false);
        let _ = self.window.present();
    }

    fn slider_hit(&self, x: i32, y: i32) -> Option<usize> {
        for index in 0..3 {
            let (track_x, track_y, track_w) = self.slider_track(index);
            if x >= track_x - 3
                && x <= track_x + track_w + 3
                && y >= track_y - 3
                && y <= track_y + SLIDER_H + 3
            {
                return Some(index);
            }
        }
        None
    }

    fn swatch_hit(&self, x: i32, y: i32) -> Option<usize> {
        let (grid_x, grid_y) = Self::grid_origin();
        for index in 0..PALETTE.len() {
            let col = (index % SWATCH_COLS) as i32;
            let row = (index / SWATCH_COLS) as i32;
            let sx = grid_x + col * (SWATCH_SIZE + SWATCH_GAP);
            let sy = grid_y + row * (SWATCH_SIZE + SWATCH_GAP);
            if x >= sx && x < sx + SWATCH_SIZE && y >= sy && y < sy + SWATCH_SIZE {
                return Some(index);
            }
        }
        None
    }

    pub fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<u32> {
        match event.kind {
            runtime::GUI_EVENT_CLOSE => return DialogStatus::Done(None),
            runtime::GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                self.relayout();
                self.render();
            }
            runtime::GUI_EVENT_KEY if event.payload[3] != 0 => match event.payload[0] {
                runtime::KEY_ENTER => return DialogStatus::Done(Some(self.value())),
                runtime::KEY_ESCAPE => return DialogStatus::Done(None),
                _ => {}
            },
            runtime::GUI_EVENT_MOUSE => {
                let x = event.payload[0] as i32;
                let y = event.payload[1] as i32;
                match event.payload[3] {
                    runtime::GUI_MOUSE_DOWN => {
                        if self.ok.hit(x, y) {
                            return DialogStatus::Done(Some(self.value()));
                        }
                        if self.cancel.hit(x, y) {
                            return DialogStatus::Done(None);
                        }
                        if let Some(swatch) = self.swatch_hit(x, y) {
                            let color = PALETTE[swatch];
                            self.r = (color >> 16) as u8;
                            self.g = (color >> 8) as u8;
                            self.b = color as u8;
                            self.render();
                        } else if let Some(slider) = self.slider_hit(x, y) {
                            self.active_slider = Some(slider);
                            self.set_channel_from_x(slider, x);
                            self.render();
                        }
                    }
                    runtime::GUI_MOUSE_MOVE => {
                        // Drag: update the active slider while the left button is held.
                        if event.payload[2] & 1 != 0 {
                            if let Some(slider) = self.active_slider {
                                self.set_channel_from_x(slider, x);
                                self.render();
                            }
                        } else {
                            self.active_slider = None;
                        }
                    }
                    runtime::GUI_MOUSE_UP => self.active_slider = None,
                    _ => {}
                }
            }
            _ => {}
        }
        DialogStatus::Pending
    }
}
