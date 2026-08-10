//! A small, game-specific Lua source document with a single cursor.
//!
//! Deliberately not a general-purpose text editor: just enough to let the
//! player make ordinary multi-line edits to a controller script. Kept free
//! of any terminal/rendering types so it can be unit tested directly; the
//! Controller view in `super::ui` is responsible for turning this into
//! visible rows, a gutter, and an auto-scrolled viewport.

/// One editing or cursor-movement operation the player can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    Insert(char),
    Newline,
    Backspace,
    DeleteForward,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
    PageUp,
    PageDown,
}

impl EditOp {
    /// Whether this operation changes the source text, as opposed to only
    /// moving the cursor. Callers use this to decide whether a prior
    /// validation result is still trustworthy: moving the cursor after a
    /// successful or failed check shouldn't silently clear the READY/error
    /// banner, since the source it describes hasn't changed.
    pub fn is_mutating(self) -> bool {
        matches!(
            self,
            EditOp::Insert(_) | EditOp::Newline | EditOp::Backspace | EditOp::DeleteForward
        )
    }
}

/// How many lines a `PageUp`/`PageDown` moves the cursor by. Not tied to any
/// real viewport height (the document model doesn't know one); it's simply
/// a fixed jump big enough to be useful for scrolling through a short
/// controller script.
const PAGE_LINES: usize = 10;

/// A Lua source buffer plus a cursor into it.
///
/// The cursor is always a byte offset into `source` on a `char` boundary
/// (never mid-UTF-8-sequence), so slicing around it is always valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    source: String,
    cursor: usize,
    /// The column (in chars) the last `MoveUp`/`MoveDown` tried to reach,
    /// so moving through a short line and back doesn't forget how far right
    /// the cursor used to be, matching ordinary editor behavior.
    preferred_col: usize,
}

impl Editor {
    pub fn new(source: impl Into<String>) -> Self {
        let source = source.into();
        let cursor = source.len();
        let mut editor = Editor {
            source,
            cursor,
            preferred_col: 0,
        };
        editor.sync_preferred_col();
        editor
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// 0-based `(line, column)` of the cursor, both counted in `char`s.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.source[..self.cursor];
        let line = before.matches('\n').count();
        let col = match before.rfind('\n') {
            Some(idx) => before[idx + 1..].chars().count(),
            None => before.chars().count(),
        };
        (line, col)
    }

    pub fn apply(&mut self, op: EditOp) {
        match op {
            EditOp::Insert(c) => self.insert_char(c),
            EditOp::Newline => self.insert_newline(),
            EditOp::Backspace => self.backspace(),
            EditOp::DeleteForward => self.delete_forward(),
            EditOp::MoveLeft => self.move_left(),
            EditOp::MoveRight => self.move_right(),
            EditOp::MoveUp => self.move_up(),
            EditOp::MoveDown => self.move_down(),
            EditOp::MoveLineStart => self.move_line_start(),
            EditOp::MoveLineEnd => self.move_line_end(),
            EditOp::PageUp => self.move_page_up(),
            EditOp::PageDown => self.move_page_down(),
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.source.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.sync_preferred_col();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.prev_char_boundary(self.cursor);
        self.source.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        self.sync_preferred_col();
    }

    pub fn delete_forward(&mut self) {
        if self.cursor == self.source.len() {
            return;
        }
        let next = self.next_char_boundary(self.cursor);
        self.source.replace_range(self.cursor..next, "");
        self.sync_preferred_col();
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.prev_char_boundary(self.cursor);
        }
        self.sync_preferred_col();
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.source.len() {
            self.cursor = self.next_char_boundary(self.cursor);
        }
        self.sync_preferred_col();
    }

    pub fn move_up(&mut self) {
        let (line, _) = self.cursor_line_col();
        if line == 0 {
            return;
        }
        self.move_to(line - 1, self.preferred_col);
    }

    pub fn move_down(&mut self) {
        let (line, _) = self.cursor_line_col();
        let last_line = self.source.matches('\n').count();
        if line >= last_line {
            return;
        }
        self.move_to(line + 1, self.preferred_col);
    }

    pub fn move_line_start(&mut self) {
        self.cursor = self.line_start_byte(self.cursor);
        self.sync_preferred_col();
    }

    pub fn move_line_end(&mut self) {
        self.cursor = self.line_end_byte(self.cursor);
        self.sync_preferred_col();
    }

    pub fn move_page_up(&mut self) {
        let (line, _) = self.cursor_line_col();
        let target = line.saturating_sub(PAGE_LINES);
        self.move_to(target, self.preferred_col);
    }

    pub fn move_page_down(&mut self) {
        let (line, _) = self.cursor_line_col();
        let last_line = self.source.matches('\n').count();
        let target = (line + PAGE_LINES).min(last_line);
        self.move_to(target, self.preferred_col);
    }

    pub fn reset(&mut self, source: impl Into<String>) {
        let source = source.into();
        self.cursor = source.len();
        self.source = source;
        self.preferred_col = 0;
    }

    /// Moves the cursor to `target_line`, landing as close as possible to
    /// `target_col` without exceeding that line's length, then leaves
    /// `preferred_col` untouched so repeated vertical moves through shorter
    /// lines still remember how far right the cursor started.
    fn move_to(&mut self, target_line: usize, target_col: usize) {
        let line_start = self.nth_line_start_byte(target_line);
        let line_end = self.line_end_byte(line_start);
        let line = &self.source[line_start..line_end];
        let clamped_col = target_col.min(line.chars().count());
        let byte_offset: usize = line.chars().take(clamped_col).map(char::len_utf8).sum();
        self.cursor = line_start + byte_offset;
    }

    fn sync_preferred_col(&mut self) {
        let (_, col) = self.cursor_line_col();
        self.preferred_col = col;
    }

    fn prev_char_boundary(&self, from: usize) -> usize {
        let mut idx = from - 1;
        while !self.source.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn next_char_boundary(&self, from: usize) -> usize {
        let mut idx = from + 1;
        while idx < self.source.len() && !self.source.is_char_boundary(idx) {
            idx += 1;
        }
        idx
    }

    fn line_start_byte(&self, from: usize) -> usize {
        self.source[..from]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0)
    }

    fn line_end_byte(&self, from: usize) -> usize {
        self.source[from..]
            .find('\n')
            .map(|idx| from + idx)
            .unwrap_or(self.source.len())
    }

    /// The byte offset where 0-based `target_line` begins. Line 0 always
    /// starts at byte 0; line `k` (k >= 1) starts right after the k-th
    /// newline counting from 1, i.e. the `(k - 1)`-th `\n` in 0-based
    /// `match_indices` order.
    fn nth_line_start_byte(&self, target_line: usize) -> usize {
        if target_line == 0 {
            return 0;
        }
        self.source
            .match_indices('\n')
            .nth(target_line - 1)
            .map(|(idx, _)| idx + 1)
            .unwrap_or(self.source.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_places_the_cursor_at_the_end_of_the_source() {
        let editor = Editor::new("abc");
        assert_eq!(editor.source(), "abc");
        assert_eq!(editor.cursor_line_col(), (0, 3));
    }

    #[test]
    fn insert_char_inserts_at_the_cursor_and_advances_it() {
        let mut editor = Editor::new("ac");
        editor.move_left();
        editor.insert_char('b');
        assert_eq!(editor.source(), "abc");
        assert_eq!(editor.cursor_line_col(), (0, 2));
    }

    #[test]
    fn insert_newline_splits_the_current_line() {
        let mut editor = Editor::new("ab");
        editor.move_left();
        editor.insert_newline();
        assert_eq!(editor.source(), "a\nb");
        assert_eq!(editor.cursor_line_col(), (1, 0));
    }

    #[test]
    fn backspace_at_start_of_document_is_a_no_op() {
        let mut editor = Editor::new("abc");
        editor.move_line_start();
        editor.backspace();
        assert_eq!(editor.source(), "abc");
    }

    #[test]
    fn backspace_joins_the_previous_line() {
        let mut editor = Editor::new("a\nb");
        editor.move_line_start();
        editor.backspace();
        assert_eq!(editor.source(), "ab");
        assert_eq!(editor.cursor_line_col(), (0, 1));
    }

    #[test]
    fn delete_forward_at_end_of_document_is_a_no_op() {
        let mut editor = Editor::new("abc");
        editor.delete_forward();
        assert_eq!(editor.source(), "abc");
    }

    #[test]
    fn delete_forward_removes_the_next_character() {
        let mut editor = Editor::new("abc");
        editor.move_line_start();
        editor.delete_forward();
        assert_eq!(editor.source(), "bc");
        assert_eq!(editor.cursor_line_col(), (0, 0));
    }

    #[test]
    fn move_left_and_right_stay_within_bounds() {
        let mut editor = Editor::new("ab");
        editor.move_line_start();
        editor.move_left();
        assert_eq!(editor.cursor_line_col(), (0, 0));

        editor.move_right();
        editor.move_right();
        editor.move_right();
        assert_eq!(editor.cursor_line_col(), (0, 2));
    }

    #[test]
    fn move_up_and_down_track_a_preferred_column_through_shorter_lines() {
        let mut editor = Editor::new("abcdef\nxy\nghijkl");
        // cursor starts at end of the last line, column 6
        editor.move_up(); // onto "xy" (len 2), clamped to column 2
        assert_eq!(editor.cursor_line_col(), (1, 2));

        editor.move_up(); // back onto "abcdef", remembers column 6
        assert_eq!(editor.cursor_line_col(), (0, 6));

        editor.move_down(); // onto "xy" again, clamped to 2
        assert_eq!(editor.cursor_line_col(), (1, 2));

        editor.move_down(); // back onto "ghijkl", remembers column 6
        assert_eq!(editor.cursor_line_col(), (2, 6));
    }

    #[test]
    fn move_up_at_first_line_is_a_no_op() {
        let mut editor = Editor::new("abc");
        editor.move_up();
        assert_eq!(editor.cursor_line_col(), (0, 3));
    }

    #[test]
    fn move_down_at_last_line_is_a_no_op() {
        let mut editor = Editor::new("abc\ndef");
        editor.move_down();
        assert_eq!(editor.cursor_line_col(), (1, 3));
    }

    #[test]
    fn move_line_start_and_end() {
        let mut editor = Editor::new("abc\ndef");
        editor.move_line_start();
        assert_eq!(editor.cursor_line_col(), (1, 0));

        editor.move_line_end();
        assert_eq!(editor.cursor_line_col(), (1, 3));
    }

    #[test]
    fn page_up_and_down_clamp_to_document_bounds() {
        let long = (0..20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = Editor::new(long);
        for _ in 0..20 {
            editor.move_up();
        }
        editor.move_line_start();
        assert_eq!(editor.cursor_line_col(), (0, 0));

        editor.move_page_down();
        assert_eq!(editor.cursor_line_col().0, 10);

        editor.move_page_down();
        assert_eq!(editor.cursor_line_col().0, 19);

        editor.move_page_up();
        assert_eq!(editor.cursor_line_col().0, 9);

        editor.move_page_up();
        assert_eq!(editor.cursor_line_col().0, 0);
    }

    #[test]
    fn reset_replaces_source_and_places_cursor_at_the_end() {
        let mut editor = Editor::new("abc");
        editor.move_line_start();
        editor.reset("xyz");
        assert_eq!(editor.source(), "xyz");
        assert_eq!(editor.cursor_line_col(), (0, 3));
    }
}
