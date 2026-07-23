use alloc::string::String;
use alloc::vec::Vec;

use gui_core::{ControlInput, ControlResponse, PointerKind, Rect};

use crate::{theme, Canvas, FONT_CELL_WIDTH, FONT_LINE_HEIGHT};

/// Horizontal tab strip. The widget owns labels, geometry, selection, and
/// input; the active theme owns all surface construction.
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
            tabs: tabs.iter().map(|tab| String::from(*tab)).collect(),
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
        if x < self.x
            || x >= self.x + self.w as i32
            || y < self.y
            || y >= self.y + Self::HEIGHT as i32
        {
            return None;
        }
        let mut tab_x = self.x;
        for (index, label) in self.tabs.iter().enumerate() {
            let width = Self::tab_width(label);
            if x >= tab_x && x < tab_x + width {
                return Some(index);
            }
            tab_x += width;
        }
        None
    }

    /// Advance to the next tab and wrap.
    pub fn cycle(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    /// Move to the previous tab and wrap.
    pub fn cycle_reverse(&mut self) {
        if !self.tabs.is_empty() {
            self.active = if self.active == 0 {
                self.tabs.len() - 1
            } else {
                self.active - 1
            };
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
                runtime::KEY_TAB if key.modifiers.ctrl && key.modifiers.shift => {
                    self.cycle_reverse()
                }
                runtime::KEY_TAB if key.modifiers.ctrl => self.cycle(),
                runtime::KEY_LEFT => self.active = self.active.saturating_sub(1),
                runtime::KEY_RIGHT => {
                    self.active = (self.active + 1).min(self.tabs.len() - 1)
                }
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
        let strip = Rect::new(self.x, self.y, self.w, Self::HEIGHT);
        theme::draw_tab_strip(canvas, strip);
        let mut tab_x = self.x;
        for (index, label) in self.tabs.iter().enumerate() {
            let width = Self::tab_width(label).max(0) as u32;
            if tab_x >= self.x + self.w as i32 {
                break;
            }
            let visible_width =
                width.min((self.x + self.w as i32 - tab_x).max(0) as u32);
            if visible_width == 0 {
                break;
            }
            let selected = index == self.active;
            theme::draw_tab(
                canvas,
                Rect::new(tab_x, self.y, visible_width, Self::HEIGHT),
                selected,
            );
            canvas.draw_text(
                tab_x + Self::PAD,
                self.y + (Self::HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                label,
                theme::tab_text(selected),
            );
            tab_x += width as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TabBar, TabBarAction};
    use gui_core::{
        ControlInput, KeyInput, Modifiers, MouseButtons, PointerInput, PointerKind,
    };

    fn key(key: u32, modifiers: Modifiers) -> ControlInput {
        ControlInput::Key(KeyInput {
            key,
            character: '\0',
            modifiers,
            pressed: true,
        })
    }

    #[test]
    fn variable_width_hit_testing_respects_strip_bounds() {
        let tabs = TabBar::new(10, 5, 300, &["A", "Long"]);
        assert_eq!(tabs.hit(10, 5), Some(0));
        assert_eq!(tabs.hit(10 + 32, 5), Some(1));
        assert_eq!(tabs.hit(9, 5), None);
        assert_eq!(tabs.hit(311, 5), None);
        assert_eq!(tabs.hit(10, 31), None);
    }

    #[test]
    fn ctrl_tab_wraps_in_both_directions_and_plain_tab_is_ignored() {
        let mut tabs = TabBar::new(0, 0, 300, &["One", "Two", "Three"]);
        let plain = tabs.handle_input(key(runtime::KEY_TAB, Modifiers::default()));
        assert!(!plain.consumed);
        assert_eq!(tabs.active, 0);

        let forward = tabs.handle_input(key(
            runtime::KEY_TAB,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ));
        assert_eq!(forward.action, Some(TabBarAction::Changed(1)));
        tabs.active = 2;
        tabs.handle_input(key(
            runtime::KEY_TAB,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ));
        assert_eq!(tabs.active, 0);

        tabs.handle_input(key(
            runtime::KEY_TAB,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            },
        ));
        assert_eq!(tabs.active, 2);
    }

    #[test]
    fn pointer_selects_a_tab() {
        let mut tabs = TabBar::new(0, 0, 300, &["One", "Two"]);
        let response = tabs.handle_input(ControlInput::Pointer(PointerInput {
            x: 60,
            y: 8,
            buttons: MouseButtons {
                left: true,
                ..MouseButtons::default()
            },
            modifiers: Modifiers::default(),
            kind: PointerKind::Down,
            timestamp: 1,
        }));
        assert_eq!(response.action, Some(TabBarAction::Changed(1)));
    }
}
