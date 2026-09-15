//! Unicode-aware multiline chat editor state and viewport layout.

use std::{cell::Cell, ops::Range};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCell {
    pub text: String,
    pub selected: bool,
    byte: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorRow {
    pub cells: Vec<EditorCell>,
    end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorViewport {
    pub rows: Vec<EditorRow>,
    pub cursor_row: usize,
    pub cursor_column: usize,
    pub total_rows: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ChatEditor {
    buffer: String,
    cursor: usize,
    anchor: Option<usize>,
    history_index: Option<usize>,
    history_draft: Option<String>,
    preferred_column: Option<usize>,
    viewport_start: Cell<usize>,
}

impl ChatEditor {
    pub fn text(&self) -> &str {
        &self.buffer
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    #[cfg(test)]
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.buffer = text.into();
        self.cursor = self.buffer.len();
        self.anchor = None;
        self.reset_navigation();
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.anchor = None;
        self.reset_navigation();
    }

    pub fn insert_text(&mut self, text: &str) {
        self.delete_selection();
        let filtered = text
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect::<String>();
        self.buffer.insert_str(self.cursor, &filtered);
        self.cursor += filtered.len();
        self.finish_edit();
    }

    pub fn insert_newline(&mut self) {
        self.insert_text("\n");
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.buffer[range])
    }

    pub fn select_all(&mut self) {
        if self.buffer.is_empty() {
            self.anchor = None;
            return;
        }
        self.anchor = Some(0);
        self.cursor = self.buffer.len();
        self.preferred_column = None;
    }

    pub fn clear_selection(&mut self) -> bool {
        let had_selection = self.selection().is_some();
        self.anchor = None;
        had_selection
    }

    pub fn backspace(&mut self, by_word: bool) {
        if self.delete_selection() {
            return;
        }
        let start = if by_word {
            self.previous_word_boundary()
        } else {
            self.previous_grapheme_boundary()
        };
        if start < self.cursor {
            self.buffer.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.finish_edit();
        }
    }

    pub fn delete(&mut self, by_word: bool) {
        if self.delete_selection() {
            return;
        }
        let end = if by_word {
            self.next_word_boundary()
        } else {
            self.next_grapheme_boundary()
        };
        if end > self.cursor {
            self.buffer.replace_range(self.cursor..end, "");
            self.finish_edit();
        }
    }

    pub fn move_left(&mut self, by_word: bool, extend: bool) {
        let target = if by_word {
            self.previous_word_boundary()
        } else {
            self.previous_grapheme_boundary()
        };
        self.move_cursor(target, extend);
    }

    pub fn move_right(&mut self, by_word: bool, extend: bool) {
        let target = if by_word {
            self.next_word_boundary()
        } else {
            self.next_grapheme_boundary()
        };
        self.move_cursor(target, extend);
    }

    pub fn move_home(&mut self, document: bool, extend: bool) {
        let target = if document {
            0
        } else {
            self.buffer[..self.cursor]
                .rfind('\n')
                .map_or(0, |index| index + 1)
        };
        self.move_cursor(target, extend);
    }

    pub fn move_end(&mut self, document: bool, extend: bool) {
        let target = if document {
            self.buffer.len()
        } else {
            self.buffer[self.cursor..]
                .find('\n')
                .map_or(self.buffer.len(), |index| self.cursor + index)
        };
        self.move_cursor(target, extend);
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let lines = logical_lines(&self.buffer);
        let current_line = lines
            .iter()
            .position(|(start, end)| self.cursor >= *start && self.cursor <= *end)
            .unwrap_or_else(|| lines.len().saturating_sub(1));
        let column = self.preferred_column.unwrap_or_else(|| {
            let (start, _) = lines[current_line];
            self.buffer[start..self.cursor].width()
        });
        self.preferred_column = Some(column);
        let target_line = current_line
            .saturating_add_signed(delta)
            .min(lines.len().saturating_sub(1));
        let (start, end) = lines[target_line];
        let target = byte_at_display_column(&self.buffer, start, end, column);
        self.move_cursor_preserving_column(target, extend);
    }

    pub fn navigate_history(&mut self, history: &[String], previous: bool) {
        if history.is_empty() {
            return;
        }
        let next = if previous {
            match self.history_index {
                Some(index) => index.saturating_sub(1),
                None => {
                    self.history_draft = Some(self.buffer.clone());
                    history.len() - 1
                }
            }
        } else {
            let Some(index) = self.history_index else {
                return;
            };
            if index + 1 >= history.len() {
                self.buffer = self.history_draft.take().unwrap_or_default();
                self.history_index = None;
                self.cursor = self.buffer.len();
                self.anchor = None;
                self.preferred_column = None;
                return;
            }
            index + 1
        };
        self.history_index = Some(next);
        self.buffer.clone_from(&history[next]);
        self.cursor = self.buffer.len();
        self.anchor = None;
        self.preferred_column = None;
    }

    /// Move to a terminal cell using the same grapheme layout as the rendered input.
    pub fn move_to_position(
        &mut self,
        width: usize,
        height: usize,
        row: usize,
        column: usize,
        extend: bool,
    ) {
        let viewport = self.viewport(width, height);
        let Some(row) = viewport.rows.get(row).or_else(|| viewport.rows.last()) else {
            return;
        };
        let mut target = row.end;
        let mut left = 0;
        for cell in &row.cells {
            left += cell.text.width().max(1);
            if column < left {
                target = cell.byte;
                break;
            }
        }
        self.move_cursor(target, extend);
    }

    pub fn viewport(&self, width: usize, height: usize) -> EditorViewport {
        if width == 0 || height == 0 {
            return EditorViewport::default();
        }
        let selection = self.selection();
        let mut rows = vec![EditorRow::default()];
        let mut row = 0usize;
        let mut column = 0usize;
        let mut cursor_position = None;
        let mut wrapped_at_line_start = false;
        let graphemes = self.buffer.grapheme_indices(true).collect::<Vec<_>>();

        for (position, (byte, grapheme)) in graphemes.iter().copied().enumerate() {
            let next_byte = graphemes
                .get(position + 1)
                .map_or(self.buffer.len(), |(byte, _)| *byte);
            if grapheme == "\n" {
                if byte == self.cursor {
                    cursor_position = Some((row, column));
                }
                if wrapped_at_line_start {
                    wrapped_at_line_start = false;
                } else {
                    rows[row].end = byte;
                    rows.push(EditorRow::default());
                    row += 1;
                }
                rows[row].end = next_byte;
                column = 0;
                continue;
            }

            let display = display_grapheme(grapheme, width);
            let grapheme_width = display.width().max(1);
            wrapped_at_line_start = false;
            if column > 0 && column.saturating_add(grapheme_width) > width {
                rows.push(EditorRow {
                    end: byte,
                    ..EditorRow::default()
                });
                row += 1;
                column = 0;
            }
            if byte == self.cursor {
                cursor_position = Some((row, column));
            }
            let selected = selection
                .as_ref()
                .is_some_and(|range| byte < range.end && next_byte > range.start);
            rows[row].cells.push(EditorCell {
                text: display,
                selected,
                byte,
            });
            rows[row].end = next_byte;
            column = column.saturating_add(grapheme_width);
            if column >= width {
                rows.push(EditorRow {
                    end: next_byte,
                    ..EditorRow::default()
                });
                row += 1;
                column = 0;
                wrapped_at_line_start = true;
            }
        }
        let (cursor_row, cursor_column) = cursor_position.unwrap_or((row, column));
        let total_rows = rows.len();
        let mut start = self
            .viewport_start
            .get()
            .min(total_rows.saturating_sub(height.min(total_rows)));
        if cursor_row < start {
            start = cursor_row;
        } else if cursor_row >= start + height {
            start = cursor_row + 1 - height;
        }
        self.viewport_start.set(start);
        let visible_rows = rows.into_iter().skip(start).take(height).collect();
        EditorViewport {
            rows: visible_rows,
            cursor_row: cursor_row.saturating_sub(start),
            cursor_column,
            total_rows,
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.buffer.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = None;
        self.finish_edit();
        true
    }

    fn move_cursor(&mut self, target: usize, extend: bool) {
        self.preferred_column = None;
        self.move_cursor_preserving_column(target, extend);
    }

    fn move_cursor_preserving_column(&mut self, target: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = target.min(self.buffer.len());
        if self.anchor == Some(self.cursor) {
            self.anchor = None;
        }
        self.history_index = None;
        self.history_draft = None;
    }

    fn previous_grapheme_boundary(&self) -> usize {
        self.buffer[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_grapheme_boundary(&self) -> usize {
        self.buffer[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.buffer.len(), |grapheme| self.cursor + grapheme.len())
    }

    fn previous_word_boundary(&self) -> usize {
        let graphemes = self.buffer[..self.cursor]
            .grapheme_indices(true)
            .collect::<Vec<_>>();
        let mut index = graphemes.len();
        while index > 0 && grapheme_class(graphemes[index - 1].1) == GraphemeClass::Whitespace {
            index -= 1;
        }
        if index == 0 {
            return 0;
        }
        let class = grapheme_class(graphemes[index - 1].1);
        while index > 0 && grapheme_class(graphemes[index - 1].1) == class {
            index -= 1;
        }
        graphemes.get(index).map_or(0, |(byte, _)| *byte)
    }

    fn next_word_boundary(&self) -> usize {
        let graphemes = self.buffer[self.cursor..]
            .grapheme_indices(true)
            .map(|(byte, grapheme)| (self.cursor + byte, grapheme))
            .collect::<Vec<_>>();
        let mut index = 0usize;
        while index < graphemes.len()
            && grapheme_class(graphemes[index].1) == GraphemeClass::Whitespace
        {
            index += 1;
        }
        if index >= graphemes.len() {
            return self.buffer.len();
        }
        let class = grapheme_class(graphemes[index].1);
        while index < graphemes.len() && grapheme_class(graphemes[index].1) == class {
            index += 1;
        }
        graphemes
            .get(index)
            .map_or(self.buffer.len(), |(byte, _)| *byte)
    }

    fn finish_edit(&mut self) {
        self.anchor = None;
        self.reset_navigation();
    }

    fn reset_navigation(&mut self) {
        self.history_index = None;
        self.history_draft = None;
        self.preferred_column = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphemeClass {
    Whitespace,
    Word,
    Punctuation,
}

fn grapheme_class(grapheme: &str) -> GraphemeClass {
    if grapheme.chars().all(char::is_whitespace) {
        GraphemeClass::Whitespace
    } else if grapheme
        .chars()
        .any(|character| character.is_alphanumeric() || character == '_')
    {
        GraphemeClass::Word
    } else {
        GraphemeClass::Punctuation
    }
}

fn logical_lines(buffer: &str) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, character) in buffer.char_indices() {
        if character == '\n' {
            lines.push((start, index));
            start = index + character.len_utf8();
        }
    }
    lines.push((start, buffer.len()));
    lines
}

fn byte_at_display_column(buffer: &str, start: usize, end: usize, target: usize) -> usize {
    let mut column = 0usize;
    for (offset, grapheme) in buffer[start..end].grapheme_indices(true) {
        let width = grapheme.width();
        if column.saturating_add(width) > target {
            return start + offset;
        }
        column = column.saturating_add(width);
    }
    end
}

fn display_grapheme(grapheme: &str, width: usize) -> String {
    if grapheme == "\t" {
        return "⇥".to_owned();
    }
    if grapheme.chars().any(char::is_control) {
        return "�".to_owned();
    }
    if grapheme.width() == 0 {
        return format!("◌{grapheme}");
    }
    if grapheme.width() > width {
        return "…".to_owned();
    }
    grapheme.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_is_grapheme_aware_and_replaces_selections() {
        let mut editor = ChatEditor::default();
        editor.insert_text("a你e\u{301} word");
        editor.move_left(true, true);
        assert_eq!(editor.selected_text(), Some("word"));
        editor.insert_text("世界");
        assert_eq!(editor.text(), "a你e\u{301} 世界");
        editor.backspace(false);
        assert_eq!(editor.text(), "a你e\u{301} 世");
    }

    #[test]
    fn vertical_navigation_and_history_restore_the_draft() {
        let mut editor = ChatEditor::default();
        editor.set_text("alpha\n你好");
        editor.move_home(false, false);
        editor.move_vertical(-1, false);
        assert!(editor.cursor() <= "alpha".len());

        editor.set_text("draft");
        let history = vec!["first".to_owned(), "second".to_owned()];
        editor.navigate_history(&history, true);
        assert_eq!(editor.text(), "second");
        editor.navigate_history(&history, true);
        assert_eq!(editor.text(), "first");
        editor.navigate_history(&history, false);
        editor.navigate_history(&history, false);
        assert_eq!(editor.text(), "draft");
    }

    #[test]
    fn viewport_tracks_cjk_combining_text_selection_and_wrapping() {
        let mut editor = ChatEditor::default();
        editor.set_text("你e\u{301}好\nline");
        editor.select_all();
        let viewport = editor.viewport(4, 2);
        assert_eq!(viewport.rows.len(), 2);
        assert!(viewport.total_rows >= 3);
        assert!(
            viewport
                .rows
                .iter()
                .flat_map(|row| &row.cells)
                .all(|cell| cell.selected)
        );
        assert!(viewport.cursor_row < 2);
        assert!(viewport.cursor_column <= 4);
    }

    #[test]
    fn cursor_moves_to_the_next_visual_row_before_a_wrapped_grapheme() {
        let mut editor = ChatEditor::default();
        editor.set_text("ab你");
        editor.move_left(false, false);

        let viewport = editor.viewport(3, 3);

        assert_eq!(viewport.cursor_row, 1);
        assert_eq!(viewport.cursor_column, 0);
    }

    #[test]
    fn mouse_position_handles_wrapping_tabs_and_empty_lines() {
        let mut editor = ChatEditor::default();
        editor.set_text("ab你c\n👩‍💻e\u{301}\n\n\tZ");
        editor.move_to_position(5, 8, 0, 3, false);
        assert_eq!(editor.cursor(), "ab".len());
        editor.move_to_position(5, 8, 1, 1, false);
        assert_eq!(editor.cursor(), "ab你c\n".len());
        editor.move_to_position(5, 8, 1, 2, true);
        assert_eq!(editor.selected_text(), Some("👩‍💻"));
        editor.move_to_position(5, 8, 2, 4, false);
        assert_eq!(editor.cursor(), "ab你c\n👩‍💻e\u{301}\n".len());
        editor.move_to_position(5, 8, 3, 0, false);
        assert_eq!(editor.cursor(), "ab你c\n👩‍💻e\u{301}\n\n".len());
        editor.move_to_position(5, 8, 7, 4, false);
        assert_eq!(editor.cursor(), editor.text().len());

        editor.set_text("ab你");
        editor.move_to_position(3, 4, 0, 2, false);
        assert_eq!(editor.cursor(), 2);
        editor.move_to_position(3, 4, 1, 1, false);
        assert_eq!(editor.cursor(), 2);
    }

    #[test]
    fn clicking_a_scrolled_input_keeps_the_visible_rows_stable() {
        let mut editor = ChatEditor::default();
        editor.set_text("zero\none\ntwo\nthree\nfour");
        let before = editor.viewport(10, 3);
        editor.move_to_position(10, 3, 0, 1, false);
        assert_eq!(editor.cursor(), "zero\none\nt".len());
        let after = editor.viewport(10, 3);
        assert_eq!(before.rows, after.rows);
        assert_eq!(after.cursor_row, 0);
    }
}
