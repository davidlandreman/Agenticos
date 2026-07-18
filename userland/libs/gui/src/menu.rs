use alloc::string::String;
use alloc::vec::Vec;

use crate::{theme, Canvas, FONT_CELL_WIDTH, FONT_LINE_HEIGHT};
use gui_core::{ControlResponse, KeyInput, PointerInput, PointerKind, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuEntryFlags(pub u32);

impl MenuEntryFlags {
    pub const SEPARATOR: u32 = 1 << 0;
    pub const DISABLED: u32 = 1 << 1;
    pub const CHECKED: u32 = 1 << 2;
    pub const RADIO: u32 = 1 << 3;
    pub const SUBMENU: u32 = 1 << 4;

    pub const fn contains(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

#[derive(Clone, Debug)]
pub struct MenuEntry {
    pub id: u64,
    pub label: String,
    pub secondary: String,
    pub flags: MenuEntryFlags,
}

impl MenuEntry {
    pub fn new(id: u64, label: &str, secondary: &str, flags: MenuEntryFlags) -> Self {
        Self {
            id,
            label: String::from(label),
            secondary: String::from(secondary),
            flags,
        }
    }

    pub fn selectable(&self) -> bool {
        !self.flags.contains(MenuEntryFlags::SEPARATOR)
            && !self.flags.contains(MenuEntryFlags::DISABLED)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPopupAction {
    Activate(u64),
    Cancel,
}

pub struct MenuPopup {
    pub entries: Vec<MenuEntry>,
    pub bounds: Rect,
    selected: Option<usize>,
    pressed: Option<usize>,
}

impl MenuPopup {
    pub const ITEM_HEIGHT: u32 = 24;
    const MIN_WIDTH: u32 = 168;
    const HORIZONTAL_PADDING: u32 = 58;

    pub fn new(
        entries: Vec<MenuEntry>,
        selected: Option<usize>,
        anchor_x: i32,
        anchor_y: i32,
        surface_width: u32,
        surface_height: u32,
    ) -> Self {
        let text_columns = entries
            .iter()
            .map(|entry| entry.label.chars().count() + entry.secondary.chars().count())
            .max()
            .unwrap_or(0) as u32;
        let width = Self::MIN_WIDTH.max(
            text_columns
                .saturating_mul(FONT_CELL_WIDTH as u32)
                .saturating_add(Self::HORIZONTAL_PADDING),
        );
        let height = entries.len() as u32 * Self::ITEM_HEIGHT + 4;
        let max_x = surface_width.saturating_sub(width) as i32;
        let max_y = surface_height.saturating_sub(height) as i32;
        let bounds = Rect::new(
            anchor_x.clamp(0, max_x.max(0)),
            anchor_y.clamp(0, max_y.max(0)),
            width.min(surface_width),
            height.min(surface_height),
        );
        let selected = selected
            .filter(|index| {
                entries
                    .get(*index)
                    .map(MenuEntry::selectable)
                    .unwrap_or(false)
            })
            .or_else(|| entries.iter().position(MenuEntry::selectable));
        Self {
            entries,
            bounds,
            selected,
            pressed: None,
        }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let palette = theme::palette();
        theme::draw_menu_surface(
            canvas,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
        );
        for (index, entry) in self.entries.iter().enumerate() {
            let row_y = self.bounds.y + 2 + index as i32 * Self::ITEM_HEIGHT as i32;
            if entry.flags.contains(MenuEntryFlags::SEPARATOR) {
                canvas.horizontal_line(
                    self.bounds.x + 8,
                    row_y + Self::ITEM_HEIGHT as i32 / 2,
                    self.bounds.w.saturating_sub(16),
                    palette.border,
                );
                continue;
            }
            let highlighted = self.selected == Some(index);
            if highlighted {
                theme::draw_selection(
                    canvas,
                    self.bounds.x + 2,
                    row_y,
                    self.bounds.w.saturating_sub(4),
                    Self::ITEM_HEIGHT,
                );
            }
            let text_color = if entry.flags.contains(MenuEntryFlags::DISABLED) {
                palette.disabled_text
            } else if highlighted {
                palette.selection_text
            } else {
                palette.text
            };
            if entry.flags.contains(MenuEntryFlags::CHECKED) {
                canvas.draw_text(self.bounds.x + 9, row_y + 4, "✓", text_color);
            } else if entry.flags.contains(MenuEntryFlags::RADIO) {
                canvas.draw_text(self.bounds.x + 9, row_y + 4, "•", text_color);
            }
            canvas.draw_text(self.bounds.x + 28, row_y + 4, &entry.label, text_color);
            if !entry.secondary.is_empty() {
                let secondary_width = entry.secondary.chars().count() as i32 * FONT_CELL_WIDTH;
                canvas.draw_text(
                    self.bounds.right() - secondary_width - 24,
                    row_y + (Self::ITEM_HEIGHT as i32 - FONT_LINE_HEIGHT) / 2,
                    &entry.secondary,
                    text_color,
                );
            }
            if entry.flags.contains(MenuEntryFlags::SUBMENU) {
                canvas.draw_text(self.bounds.right() - 16, row_y + 4, "›", text_color);
            }
        }
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<MenuPopupAction> {
        let row = self.row_at(input.x, input.y);
        match input.kind {
            PointerKind::Move => {
                let next = row.filter(|index| self.entries[*index].selectable());
                let repaint = next != self.selected;
                self.selected = next;
                ControlResponse::consumed(repaint, None)
            }
            PointerKind::Down => {
                if !self.bounds.contains(input.x, input.y) {
                    return ControlResponse::consumed(true, Some(MenuPopupAction::Cancel));
                }
                self.pressed = row.filter(|index| self.entries[*index].selectable());
                ControlResponse::consumed(true, None)
            }
            PointerKind::Up => {
                let action = self
                    .pressed
                    .take()
                    .filter(|pressed| Some(*pressed) == row)
                    .map(|index| MenuPopupAction::Activate(self.entries[index].id));
                ControlResponse::consumed(true, action)
            }
            PointerKind::Cancel => {
                self.pressed = None;
                ControlResponse::consumed(true, Some(MenuPopupAction::Cancel))
            }
            _ => ControlResponse::consumed(false, None),
        }
    }

    pub fn handle_key(&mut self, input: KeyInput) -> ControlResponse<MenuPopupAction> {
        if !input.pressed {
            return ControlResponse::ignored();
        }
        if input.key == runtime::KEY_ESCAPE {
            return ControlResponse::consumed(true, Some(MenuPopupAction::Cancel));
        }
        if input.key == runtime::KEY_ENTER || input.character == ' ' {
            let action = self
                .selected
                .map(|index| MenuPopupAction::Activate(self.entries[index].id));
            return ControlResponse::consumed(true, action);
        }
        let direction = if input.key == runtime::KEY_UP {
            -1
        } else if input.key == runtime::KEY_DOWN {
            1
        } else {
            0
        };
        if direction != 0 {
            self.move_selection(direction);
            return ControlResponse::consumed(true, None);
        }
        if input.character > ' ' {
            let needle = input.character.to_ascii_lowercase();
            if let Some(index) = self.entries.iter().position(|entry| {
                entry.selectable()
                    && entry
                        .label
                        .chars()
                        .find(|character| character.is_alphanumeric())
                        .map(|character| character.to_ascii_lowercase() == needle)
                        .unwrap_or(false)
            }) {
                self.selected = Some(index);
                return ControlResponse::consumed(true, None);
            }
        }
        ControlResponse::consumed(false, None)
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.bounds.contains(x, y) || y < self.bounds.y + 2 {
            return None;
        }
        let index = ((y - self.bounds.y - 2) / Self::ITEM_HEIGHT as i32) as usize;
        (index < self.entries.len()).then_some(index)
    }

    fn move_selection(&mut self, direction: isize) {
        if self.entries.is_empty() {
            self.selected = None;
            return;
        }
        let mut index = self.selected.unwrap_or(0) as isize;
        for _ in 0..self.entries.len() {
            index = (index + direction).rem_euclid(self.entries.len() as isize);
            if self.entries[index as usize].selectable() {
                self.selected = Some(index as usize);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::{MenuEntry, MenuEntryFlags, MenuPopup};
    use gui_core::{KeyInput, Modifiers};

    #[test]
    fn keyboard_navigation_skips_disabled_items_and_separators() {
        let entries = vec![
            MenuEntry::new(1, "First", "", MenuEntryFlags::default()),
            MenuEntry::new(2, "Disabled", "", MenuEntryFlags(MenuEntryFlags::DISABLED)),
            MenuEntry::new(3, "", "", MenuEntryFlags(MenuEntryFlags::SEPARATOR)),
            MenuEntry::new(4, "Last", "", MenuEntryFlags::default()),
        ];
        let mut menu = MenuPopup::new(entries, Some(0), 0, 0, 400, 300);
        menu.handle_key(KeyInput {
            key: runtime::KEY_DOWN,
            character: '\0',
            modifiers: Modifiers::default(),
            pressed: true,
        });
        assert_eq!(menu.selected(), Some(3));
    }
}
