use gui_core::{
    layout_scrollbars, Axis, ControlInput, ControlResponse, KeyInput, PointerInput, PointerKind,
    Rect, ScrollbarPolicy, TextEdit,
};

use crate::{theme, Canvas, Scrollbar, FONT_CELL_WIDTH, FONT_LINE_HEIGHT, SCROLLBAR_THICKNESS};

const PAD: i32 = 5;
const LINE_HEIGHT: u32 = FONT_LINE_HEIGHT as u32 + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextAreaOptions {
    pub vertical_scrollbar: ScrollbarPolicy,
    pub horizontal_scrollbar: ScrollbarPolicy,
    pub read_only: bool,
    pub tab_spaces: u8,
    pub wrap: WrapMode,
}

impl Default for TextAreaOptions {
    fn default() -> Self {
        Self {
            vertical_scrollbar: ScrollbarPolicy::Auto,
            horizontal_scrollbar: ScrollbarPolicy::Auto,
            read_only: false,
            tab_spaces: 4,
            wrap: WrapMode::None,
        }
    }
}

impl TextAreaOptions {
    pub fn vertical_scrollbar(mut self, policy: ScrollbarPolicy) -> Self {
        self.vertical_scrollbar = policy;
        self
    }

    pub fn horizontal_scrollbar(mut self, policy: ScrollbarPolicy) -> Self {
        self.horizontal_scrollbar = policy;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn tab_spaces(mut self, spaces: u8) -> Self {
        self.tab_spaces = spaces.max(1);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAreaAction {
    Changed,
    SelectionChanged,
}

pub struct TextArea {
    bounds: Rect,
    options: TextAreaOptions,
    edit: TextEdit,
    horizontal: Scrollbar,
    vertical: Scrollbar,
    selecting: bool,
}

impl TextArea {
    pub fn new(bounds: Rect, options: TextAreaOptions) -> Self {
        Self {
            bounds,
            options,
            edit: TextEdit::new(),
            horizontal: Scrollbar::new(Axis::Horizontal, Rect::default()),
            vertical: Scrollbar::new(Axis::Vertical, Rect::default()),
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        self.edit.text()
    }

    pub fn set_text(&mut self, text: &str) {
        self.edit.set_text(text);
        self.horizontal.set_offset(0);
        self.vertical.set_offset(0);
        self.sync_layout();
    }

    pub fn caret(&self) -> usize {
        self.edit.caret()
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.edit.selection()
    }

    pub fn line_col(&self) -> (usize, usize) {
        self.edit.line_col()
    }

    pub fn is_modified(&self) -> bool {
        self.edit.is_modified()
    }

    pub fn set_modified(&mut self, modified: bool) {
        self.edit.set_modified(modified);
    }

    pub fn set_bounds(&mut self, bounds: Rect) {
        if self.bounds == bounds {
            return;
        }
        self.bounds = bounds;
        self.sync_layout();
        self.ensure_caret_visible();
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn set_scrollbar_policies(
        &mut self,
        horizontal: ScrollbarPolicy,
        vertical: ScrollbarPolicy,
    ) {
        self.options.horizontal_scrollbar = horizontal;
        self.options.vertical_scrollbar = vertical;
        self.sync_layout();
    }

    pub fn cancel_interaction(&mut self) {
        self.selecting = false;
        self.horizontal.cancel();
        self.vertical.cancel();
    }

    fn content_size(&self) -> (u32, u32) {
        let width = (self.edit.max_line_columns() as u32)
            .saturating_mul(FONT_CELL_WIDTH as u32)
            .saturating_add((PAD * 2) as u32);
        let height = (self.edit.line_count() as u32)
            .saturating_mul(LINE_HEIGHT)
            .saturating_add((PAD * 2) as u32);
        (width, height)
    }

    fn layout(&self) -> gui_core::ScrollbarsLayout {
        let (content_w, content_h) = self.content_size();
        layout_scrollbars(
            self.bounds.inset(2),
            content_w,
            content_h,
            self.options.horizontal_scrollbar,
            self.options.vertical_scrollbar,
            SCROLLBAR_THICKNESS,
        )
    }

    fn sync_layout(&mut self) -> gui_core::ScrollbarsLayout {
        let layout = self.layout();
        let (content_w, content_h) = self.content_size();
        self.horizontal
            .set_bounds(layout.horizontal.unwrap_or_default());
        self.horizontal.set_line_step(FONT_CELL_WIDTH as u32);
        self.horizontal.set_extents(content_w, layout.viewport.w);
        self.horizontal
            .set_enabled(self.horizontal.state().has_range());
        self.vertical
            .set_bounds(layout.vertical.unwrap_or_default());
        self.vertical.set_line_step(LINE_HEIGHT);
        self.vertical.set_extents(content_h, layout.viewport.h);
        self.vertical.set_enabled(self.vertical.state().has_range());
        layout
    }

    fn ensure_caret_visible(&mut self) {
        let layout = self.sync_layout();
        let (line, column) = self.edit.line_col();
        let x = (PAD as u32).saturating_add(column as u32 * FONT_CELL_WIDTH as u32);
        let y = (PAD as u32).saturating_add(line as u32 * LINE_HEIGHT);
        self.horizontal
            .state_mut()
            .ensure_visible(x, x.saturating_add(FONT_CELL_WIDTH as u32));
        self.vertical
            .state_mut()
            .ensure_visible(y, y.saturating_add(LINE_HEIGHT));
        self.horizontal
            .set_extents(self.content_size().0, layout.viewport.w);
        self.vertical
            .set_extents(self.content_size().1, layout.viewport.h);
    }

    fn index_at(&self, x: i32, y: i32, viewport: Rect) -> usize {
        let content_x = x
            .saturating_sub(viewport.x)
            .saturating_add(self.horizontal.offset() as i32)
            .saturating_sub(PAD)
            .max(0);
        let content_y = y
            .saturating_sub(viewport.y)
            .saturating_add(self.vertical.offset() as i32)
            .saturating_sub(PAD)
            .max(0);
        let line = (content_y / LINE_HEIGHT as i32) as usize;
        let column = (content_x / FONT_CELL_WIDTH) as usize;
        self.edit.index_for_line_col(line, column)
    }

    fn handle_key(&mut self, input: KeyInput) -> ControlResponse<TextAreaAction> {
        if !input.pressed {
            return ControlResponse::ignored();
        }
        let shift = input.modifiers.shift;
        let before_caret = self.edit.caret();
        let before_selection = self.edit.selection();
        let changed = match input.key {
            runtime::KEY_BACKSPACE if !self.options.read_only => self.edit.backspace(),
            runtime::KEY_DELETE if !self.options.read_only => self.edit.delete(),
            runtime::KEY_LEFT => {
                self.edit.move_horizontal(false, shift);
                false
            }
            runtime::KEY_RIGHT => {
                self.edit.move_horizontal(true, shift);
                false
            }
            runtime::KEY_UP => {
                self.edit.move_vertical(-1, shift);
                false
            }
            runtime::KEY_DOWN => {
                self.edit.move_vertical(1, shift);
                false
            }
            runtime::KEY_PAGE_UP => {
                let lines = (self.layout().viewport.h / LINE_HEIGHT).max(1) as isize;
                self.edit.move_vertical(-lines, shift);
                false
            }
            runtime::KEY_PAGE_DOWN => {
                let lines = (self.layout().viewport.h / LINE_HEIGHT).max(1) as isize;
                self.edit.move_vertical(lines, shift);
                false
            }
            runtime::KEY_HOME => {
                self.edit.move_home(input.modifiers.ctrl, shift);
                false
            }
            runtime::KEY_END => {
                self.edit.move_end(input.modifiers.ctrl, shift);
                false
            }
            runtime::KEY_ENTER if !self.options.read_only => self.edit.insert_char('\n'),
            runtime::KEY_TAB if !self.options.read_only => {
                let spaces = "        ";
                self.edit
                    .insert_str(&spaces[..self.options.tab_spaces.min(8) as usize])
            }
            _ if input.modifiers.ctrl && input.character.eq_ignore_ascii_case(&'a') => {
                self.edit.select_all();
                false
            }
            _ if !self.options.read_only
                && !input.modifiers.ctrl
                && input.character >= ' '
                && input.character != '\u{7f}' =>
            {
                self.edit.insert_char(input.character)
            }
            _ => return ControlResponse::ignored(),
        };
        self.ensure_caret_visible();
        let selection_changed =
            before_caret != self.edit.caret() || before_selection != self.edit.selection();
        ControlResponse::consumed(
            true,
            if changed {
                Some(TextAreaAction::Changed)
            } else if selection_changed {
                Some(TextAreaAction::SelectionChanged)
            } else {
                None
            },
        )
    }

    fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<TextAreaAction> {
        let layout = self.sync_layout();

        if layout.horizontal.is_some()
            && (self.horizontal.is_captured()
                || self.horizontal.bounds().contains(input.x, input.y))
        {
            let response = self.horizontal.handle_pointer(input);
            return ControlResponse {
                consumed: response.consumed,
                repaint: response.repaint,
                action: None,
            };
        }
        if layout.vertical.is_some()
            && (self.vertical.is_captured() || self.vertical.bounds().contains(input.x, input.y))
        {
            let response = self.vertical.handle_pointer(input);
            return ControlResponse {
                consumed: response.consumed,
                repaint: response.repaint,
                action: None,
            };
        }

        match input.kind {
            PointerKind::Scroll { delta_x, delta_y } if self.bounds.contains(input.x, input.y) => {
                let vertical = self.vertical.state_mut().scroll_lines(delta_y);
                let horizontal = self.horizontal.state_mut().scroll_lines(delta_x);
                ControlResponse::consumed(vertical || horizontal, None)
            }
            PointerKind::Down if layout.viewport.contains(input.x, input.y) => {
                let before = self.edit.selection();
                let index = self.index_at(input.x, input.y, layout.viewport);
                self.edit.set_caret(index, input.modifiers.shift);
                self.selecting = true;
                self.ensure_caret_visible();
                ControlResponse::consumed(
                    true,
                    (before != self.edit.selection()).then_some(TextAreaAction::SelectionChanged),
                )
            }
            PointerKind::Move if self.selecting => {
                if layout.viewport.w == 0 || layout.viewport.h == 0 {
                    return ControlResponse::consumed(false, None);
                }
                if input.y < layout.viewport.y {
                    self.vertical.state_mut().scroll_lines(-1);
                } else if input.y >= layout.viewport.bottom() {
                    self.vertical.state_mut().scroll_lines(1);
                }
                if input.x < layout.viewport.x {
                    self.horizontal.state_mut().scroll_lines(-1);
                } else if input.x >= layout.viewport.right() {
                    self.horizontal.state_mut().scroll_lines(1);
                }
                let clamped_x = input
                    .x
                    .clamp(layout.viewport.x, layout.viewport.right() - 1);
                let clamped_y = input
                    .y
                    .clamp(layout.viewport.y, layout.viewport.bottom() - 1);
                let index = self.index_at(clamped_x, clamped_y, layout.viewport);
                self.edit.set_caret(index, true);
                ControlResponse::consumed(true, Some(TextAreaAction::SelectionChanged))
            }
            PointerKind::Up if self.selecting => {
                self.selecting = false;
                ControlResponse::consumed(true, None)
            }
            PointerKind::Cancel => {
                self.cancel_interaction();
                ControlResponse::consumed(true, None)
            }
            _ => ControlResponse::ignored(),
        }
    }

    pub fn handle_input(
        &mut self,
        input: ControlInput,
        focused: bool,
    ) -> ControlResponse<TextAreaAction> {
        match input {
            ControlInput::Key(key) if focused => self.handle_key(key),
            ControlInput::Pointer(pointer) => self.handle_pointer(pointer),
            _ => ControlResponse::ignored(),
        }
    }

    pub fn draw(&mut self, canvas: &mut Canvas, focused: bool) {
        let layout = self.sync_layout();
        let palette = theme::palette();
        canvas.fill_rect(
            layout.viewport.x,
            layout.viewport.y,
            layout.viewport.w,
            layout.viewport.h,
            palette.field_bg,
        );

        let previous_clip = canvas.clip();
        let text_clip = previous_clip
            .map(|clip| {
                clip.intersection(layout.viewport)
                    .unwrap_or_else(|| Rect::new(0, 0, 0, 0))
            })
            .unwrap_or(layout.viewport);
        canvas.set_clip(Some(text_clip));
        let first_line = self.vertical.offset().saturating_sub(PAD as u32) / LINE_HEIGHT;
        let last_line = self
            .vertical
            .offset()
            .saturating_add(layout.viewport.h)
            .div_ceil(LINE_HEIGHT)
            .min(self.edit.line_count() as u32);
        let selection = self.edit.selection();
        for line in first_line as usize..last_line as usize {
            let Some(text) = self.edit.line(line) else {
                continue;
            };
            let line_start = self.edit.line_start(line).unwrap_or(0);
            let y = layout.viewport.y + PAD + line as i32 * LINE_HEIGHT as i32
                - self.vertical.offset() as i32;
            for (column, (offset, character)) in text.char_indices().enumerate() {
                let x = layout.viewport.x + PAD + column as i32 * FONT_CELL_WIDTH
                    - self.horizontal.offset() as i32;
                if x + FONT_CELL_WIDTH < layout.viewport.x || x >= layout.viewport.right() {
                    continue;
                }
                let index = line_start + offset;
                let selected = selection
                    .map(|(start, end)| index >= start && index < end)
                    .unwrap_or(false);
                if selected {
                    canvas.fill_rect(
                        x,
                        y - 1,
                        FONT_CELL_WIDTH as u32,
                        LINE_HEIGHT,
                        palette.selection_bg,
                    );
                }
                canvas.draw_char(
                    x,
                    y,
                    character,
                    if selected {
                        palette.selection_text
                    } else {
                        palette.field_text
                    },
                );
            }
        }
        if focused {
            let (line, column) = self.edit.line_col();
            let x = layout.viewport.x + PAD + column as i32 * FONT_CELL_WIDTH
                - self.horizontal.offset() as i32;
            let y = layout.viewport.y + PAD + line as i32 * LINE_HEIGHT as i32
                - self.vertical.offset() as i32;
            canvas.vertical_line(x, y, FONT_LINE_HEIGHT as u32, palette.field_text);
        }
        canvas.set_clip(previous_clip);

        if layout.horizontal.is_some() {
            self.horizontal.draw(canvas);
        }
        if layout.vertical.is_some() {
            self.vertical.draw(canvas);
        }
        if let Some(corner) = layout.corner {
            canvas.fill_rect(corner.x, corner.y, corner.w, corner.h, palette.content_bg);
        }
        theme::draw_field_border(
            canvas,
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            focused,
        );
    }
}
