//! Modal message / confirmation box.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use gui::{theme, Button, Window, FONT_CELL_WIDTH, FONT_LINE_HEIGHT};

use crate::DialogStatus;

/// Which button set a [`MessageBox`] presents.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    OkCancel,
    YesNo,
}

/// The user's answer to a [`MessageBox`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MessageChoice {
    Ok,
    Cancel,
    Yes,
    No,
}

const MARGIN: i32 = 14;
const LINE_HEIGHT: i32 = FONT_LINE_HEIGHT + 2;
const BUTTON_W: u32 = 88;
const BUTTON_H: u32 = 26;

/// A modal message box with an affirmative and (optionally) a negative button.
///
/// Text wraps greedily at the window width; `\n` is respected. Enter activates
/// the affirmative button, Esc the negative/cancel path. See the crate docs
/// for the host-integration pattern.
pub struct MessageBox {
    window: Window,
    lines: Vec<String>,
    buttons: Buttons,
    affirmative: Button,
    negative: Option<Button>,
}

impl MessageBox {
    pub fn new(title: &str, text: &str, buttons: Buttons) -> Result<Self, i64> {
        let width = 420u32;
        let lines = wrap(
            text,
            ((width as i32 - MARGIN * 2) / FONT_CELL_WIDTH).max(1) as usize,
        );
        let height = (MARGIN * 2 + lines.len() as i32 * LINE_HEIGHT + 16 + BUTTON_H as i32)
            .clamp(120, 360) as u32;
        let window = Window::new(width, height, title)?;
        let (affirmative_label, negative_label) = match buttons {
            Buttons::Ok => ("OK", None),
            Buttons::OkCancel => ("OK", Some("Cancel")),
            Buttons::YesNo => ("Yes", Some("No")),
        };
        let mut dialog = Self {
            window,
            lines,
            buttons,
            affirmative: Button::new(affirmative_label, 0, 0, BUTTON_W, BUTTON_H),
            negative: negative_label.map(|label| Button::new(label, 0, 0, BUTTON_W, BUTTON_H)),
        };
        dialog.relayout();
        dialog.render();
        Ok(dialog)
    }

    pub fn error(text: &str) -> Result<Self, i64> {
        Self::new("Error", text, Buttons::Ok)
    }

    pub fn info(text: &str) -> Result<Self, i64> {
        Self::new("Notice", text, Buttons::Ok)
    }

    pub fn confirm(title: &str, text: &str) -> Result<Self, i64> {
        Self::new(title, text, Buttons::YesNo)
    }

    pub fn window_handle(&self) -> u32 {
        self.window.handle()
    }

    fn affirmative_choice(&self) -> MessageChoice {
        match self.buttons {
            Buttons::YesNo => MessageChoice::Yes,
            _ => MessageChoice::Ok,
        }
    }

    fn negative_choice(&self) -> MessageChoice {
        match self.buttons {
            Buttons::YesNo => MessageChoice::No,
            _ => MessageChoice::Cancel,
        }
    }

    fn relayout(&mut self) {
        let canvas = self.window.canvas();
        let width = canvas.width() as i32;
        let height = canvas.height() as i32;
        let button_y = height - MARGIN - BUTTON_H as i32;
        match self.negative.as_mut() {
            None => {
                self.affirmative.x = (width - BUTTON_W as i32) / 2;
                self.affirmative.y = button_y;
            }
            Some(negative) => {
                let total = BUTTON_W as i32 * 2 + 12;
                let start = width - MARGIN - total;
                self.affirmative.x = start;
                self.affirmative.y = button_y;
                negative.x = start + BUTTON_W as i32 + 12;
                negative.y = button_y;
            }
        }
    }

    fn render(&mut self) {
        let lines = self.lines.clone();
        let palette = theme::palette();
        let canvas = self.window.canvas_mut();
        canvas.clear(palette.content_bg);
        for (index, line) in lines.iter().enumerate() {
            canvas.draw_text(
                MARGIN,
                MARGIN + index as i32 * LINE_HEIGHT,
                line,
                palette.text,
            );
        }
        self.affirmative.draw(canvas, true);
        if let Some(negative) = self.negative.as_ref() {
            negative.draw(canvas, false);
        }
        let _ = self.window.present();
    }

    pub fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<MessageChoice> {
        match event.kind {
            runtime::GUI_EVENT_CLOSE => return DialogStatus::Done(None),
            runtime::GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                self.relayout();
                self.render();
            }
            runtime::GUI_EVENT_KEY if event.payload[3] != 0 => match event.payload[0] {
                runtime::KEY_ENTER => return DialogStatus::Done(Some(self.affirmative_choice())),
                runtime::KEY_ESCAPE => {
                    return DialogStatus::Done(if self.negative.is_some() {
                        Some(self.negative_choice())
                    } else {
                        None
                    })
                }
                _ => {}
            },
            runtime::GUI_EVENT_MOUSE if event.payload[3] == runtime::GUI_MOUSE_DOWN => {
                let x = event.payload[0] as i32;
                let y = event.payload[1] as i32;
                if self.affirmative.hit(x, y) {
                    return DialogStatus::Done(Some(self.affirmative_choice()));
                }
                if let Some(negative) = self.negative.as_ref() {
                    if negative.hit(x, y) {
                        return DialogStatus::Done(Some(self.negative_choice()));
                    }
                }
            }
            _ => {}
        }
        DialogStatus::Pending
    }
}

/// Greedy word-wrap to `max_chars` columns, honoring explicit `\n`.
fn wrap(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split(' ') {
            if current.is_empty() {
                current = word.to_string();
            } else if current.chars().count() + 1 + word.chars().count() <= max_chars {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(core::mem::take(&mut current));
                current = word.to_string();
            }
            // Hard-break a single word longer than the line.
            while current.chars().count() > max_chars {
                let split = current
                    .char_indices()
                    .nth(max_chars)
                    .map(|(index, _)| index)
                    .unwrap_or(current.len());
                let rest = current.split_off(split);
                lines.push(core::mem::take(&mut current));
                current = rest;
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
