use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    text: String,
    caret: usize,
    anchor: Option<usize>,
    line_starts: Vec<usize>,
    preferred_column: Option<usize>,
    modified: bool,
}

impl Default for TextEdit {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEdit {
    pub fn new() -> Self {
        let mut value = Self {
            text: String::new(),
            caret: 0,
            anchor: None,
            line_starts: Vec::new(),
            preferred_column: None,
            modified: false,
        };
        value.rebuild_lines();
        value
    }

    pub fn from_text(text: &str) -> Self {
        let mut value = Self::new();
        value.set_text(text);
        value
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.caret).then_some((anchor.min(self.caret), anchor.max(self.caret)))
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn set_modified(&mut self, modified: bool) {
        self.modified = modified;
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    pub fn line_range(&self, line: usize) -> Option<(usize, usize)> {
        let start = self.line_start(line)?;
        let mut end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        if end > start && self.text.as_bytes().get(end - 1) == Some(&b'\n') {
            end -= 1;
        }
        Some((start, end))
    }

    pub fn line(&self, line: usize) -> Option<&str> {
        let (start, end) = self.line_range(line)?;
        Some(&self.text[start..end])
    }

    pub fn max_line_columns(&self) -> usize {
        (0..self.line_count())
            .filter_map(|line| self.line(line))
            .map(str::chars)
            .map(Iterator::count)
            .max()
            .unwrap_or(0)
    }

    pub fn line_col(&self) -> (usize, usize) {
        self.line_col_at(self.caret)
    }

    pub fn line_col_at(&self, index: usize) -> (usize, usize) {
        let index = self.clamp_boundary(index);
        let line = self
            .line_starts
            .partition_point(|start| *start <= index)
            .saturating_sub(1);
        let start = self.line_starts[line];
        (line, self.text[start..index].chars().count())
    }

    pub fn index_for_line_col(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        let (start, end) = self.line_range(line).unwrap_or((0, 0));
        self.text[start..end]
            .char_indices()
            .nth(column)
            .map(|(offset, _)| start + offset)
            .unwrap_or(end)
    }

    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.caret = 0;
        self.anchor = None;
        self.preferred_column = None;
        self.modified = false;
        self.rebuild_lines();
    }

    pub fn set_caret(&mut self, index: usize, extend: bool) {
        self.begin_move(extend);
        self.caret = self.clamp_boundary(index);
        self.preferred_column = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
        self.preferred_column = None;
    }

    pub fn move_horizontal(&mut self, right: bool, extend: bool) {
        self.begin_move(extend);
        self.caret = if right {
            self.next_boundary(self.caret)
        } else {
            self.previous_boundary(self.caret)
        };
        self.preferred_column = None;
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let (line, column) = self.line_col();
        let preferred = self.preferred_column.unwrap_or(column);
        let target = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            line.saturating_add(delta as usize)
                .min(self.line_count().saturating_sub(1))
        };
        self.begin_move(extend);
        self.caret = self.index_for_line_col(target, preferred);
        self.preferred_column = Some(preferred);
    }

    pub fn move_home(&mut self, document: bool, extend: bool) {
        let (line, _) = self.line_col();
        self.begin_move(extend);
        self.caret = if document {
            0
        } else {
            self.line_start(line).unwrap_or(0)
        };
        self.preferred_column = None;
    }

    pub fn move_end(&mut self, document: bool, extend: bool) {
        let (line, _) = self.line_col();
        self.begin_move(extend);
        self.caret = if document {
            self.text.len()
        } else {
            self.line_range(line).map(|(_, end)| end).unwrap_or(0)
        };
        self.preferred_column = None;
    }

    pub fn insert_char(&mut self, character: char) -> bool {
        self.delete_selection_inner();
        self.text.insert(self.caret, character);
        self.caret += character.len_utf8();
        self.after_edit();
        true
    }

    pub fn insert_str(&mut self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        self.delete_selection_inner();
        self.text.insert_str(self.caret, value);
        self.caret += value.len();
        self.after_edit();
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selection_inner() {
            self.after_edit();
            return true;
        }
        if self.caret == 0 {
            return false;
        }
        let previous = self.previous_boundary(self.caret);
        self.text.replace_range(previous..self.caret, "");
        self.caret = previous;
        self.after_edit();
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.delete_selection_inner() {
            self.after_edit();
            return true;
        }
        if self.caret == self.text.len() {
            return false;
        }
        let next = self.next_boundary(self.caret);
        self.text.replace_range(self.caret..next, "");
        self.after_edit();
        true
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

    fn delete_selection_inner(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.caret = start;
        self.anchor = None;
        true
    }

    fn after_edit(&mut self) {
        self.anchor = None;
        self.preferred_column = None;
        self.modified = true;
        self.rebuild_lines();
    }

    fn rebuild_lines(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (index, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                self.line_starts.push(index + 1);
            }
        }
    }

    fn clamp_boundary(&self, index: usize) -> usize {
        let mut index = index.min(self.text.len());
        while !self.text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn previous_boundary(&self, index: usize) -> usize {
        self.text[..self.clamp_boundary(index)]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self, index: usize) -> usize {
        let index = self.clamp_boundary(index);
        self.text[index..]
            .char_indices()
            .nth(1)
            .map(|(next, _)| index + next)
            .unwrap_or(self.text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_editing_stays_on_boundaries() {
        let mut edit = TextEdit::from_text("aé🙂z");
        edit.move_end(true, false);
        edit.move_horizontal(false, false);
        assert!(edit.text().is_char_boundary(edit.caret()));
        assert!(edit.backspace());
        assert_eq!(edit.text(), "aéz");
    }

    #[test]
    fn line_index_includes_trailing_empty_line() {
        let edit = TextEdit::from_text("one\ntwo\n");
        assert_eq!(edit.line_count(), 3);
        assert_eq!(edit.line(2), Some(""));
        assert_eq!(edit.index_for_line_col(1, 99), 7);
    }

    #[test]
    fn preferred_column_survives_short_line() {
        let mut edit = TextEdit::from_text("abcdef\nx\nabcdef");
        edit.set_caret(edit.index_for_line_col(0, 5), false);
        edit.move_vertical(1, false);
        assert_eq!(edit.line_col(), (1, 1));
        edit.move_vertical(1, false);
        assert_eq!(edit.line_col(), (2, 5));
    }

    #[test]
    fn selection_delete_works_in_both_directions() {
        let mut edit = TextEdit::from_text("abcdef");
        edit.set_caret(5, false);
        edit.set_caret(2, true);
        assert_eq!(edit.selection(), Some((2, 5)));
        assert!(edit.delete());
        assert_eq!(edit.text(), "abf");
    }
}
