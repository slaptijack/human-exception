//! The authoritative Controller working document.
//!
//! Wraps `ratatui_code_editor::editor::Editor` (the embedded editor
//! foundation selected in issue #90) behind a narrow, application-owned
//! seam: callers see `EditOp`/`String`/`(usize, usize)`, never `Code`,
//! `Selection`, or any other library-internal type. This is the sole
//! authoritative Lua working buffer for the Controller view — no
//! synchronized `String` mirror exists alongside it.
//!
//! Rendering (`super::ui::draw_controller_source`) draws the wrapped
//! `Editor` directly, reached through [`ControllerDocument::sync_for_render`]
//! — the only accessor that exposes the library's widget itself, for
//! presentation purposes only. The `Editor` lives behind a `RefCell` so
//! `Editor::focus` (which derives scroll offsets purely from cursor position
//! and viewport area) can run from `ui.rs`'s otherwise-immutable `&AppState`
//! render pass; that offset is a cache of a pure function, not new
//! authoritative state.
//!
//! Vertical movement (`MoveUp`/`MoveDown`) is implemented directly against
//! `code_ref()` rather than delegating to the library's own `MoveUp`/
//! `MoveDown` actions: those actions derive the target column from the
//! cursor's *current* line each time, so moving up through a short line and
//! back forgets how far right the cursor used to be. `docs/TUI_DESIGN.md`'s
//! editor contract requires exactly that memory ("moving through a shorter
//! line and back doesn't forget how far right the cursor was"), which the
//! previous bespoke editor already provided via a remembered
//! `preferred_col` — preserved here the same way, since #91 must not
//! regress it.

use std::cell::{Ref, RefCell};

use ratatui::layout::Rect;
use ratatui_code_editor::actions::{
    Delete, Indent, InsertText, MoveLeft, MoveRight, Redo, SelectAll, UnIndent, Undo,
};
use ratatui_code_editor::editor::Editor;

use super::editor::EditOp;

/// How many lines a `PageUp`/`PageDown` moves the cursor by. Not tied to any
/// real viewport height (the document model doesn't know one, matching the
/// previous bespoke editor); it's simply a fixed jump big enough to be
/// useful for scrolling through a short controller script.
const PAGE_LINES: usize = 10;

/// The player's current Lua source and cursor for a working set.
///
/// `ratatui_code_editor::editor::Editor` doesn't implement `Debug`, so this
/// derives it manually, describing only the application-level state
/// `AppState`'s own `Debug` derive needs, not the wrapped library internals.
pub(crate) struct ControllerDocument {
    editor: RefCell<Editor>,
    /// The column (in chars) the last `MoveUp`/`MoveDown` tried to reach,
    /// so moving through a short line and back doesn't forget how far right
    /// the cursor used to be, matching ordinary editor behavior.
    preferred_col: usize,
}

impl ControllerDocument {
    /// Constructs a fresh document from `source`, with the cursor placed at
    /// the end (the library defaults a new `Editor`'s cursor to `0`; the
    /// previous bespoke editor placed it at the end, and this preserves
    /// that).
    pub(crate) fn new(source: &str) -> Self {
        let mut editor =
            Editor::new("lua", source, vec![]).expect("editor construction must not fail");
        editor.set_cursor(source.chars().count());
        // Neither feature is part of #92's contract (line numbers + cursor
        // only) nor present in the editor it replaces; both default on in
        // the library, so disable them explicitly rather than silently
        // gaining unrequested visual behavior.
        editor.set_code_folding_enabled(false);
        editor.set_word_highlight_enabled(false);
        let mut doc = ControllerDocument {
            editor: RefCell::new(editor),
            preferred_col: 0,
        };
        doc.sync_preferred_col();
        doc
    }

    /// The exact current working source, including any trailing newline.
    pub(crate) fn source(&self) -> String {
        self.editor.borrow().get_content()
    }

    /// 0-based `(line, column)` of the cursor, both counted in `char`s.
    pub(crate) fn cursor_line_col(&self) -> (usize, usize) {
        let editor = self.editor.borrow();
        editor.code_ref().point(editor.get_cursor())
    }

    /// Updates the wrapped editor's scroll offsets so the cursor stays
    /// visible in `area`, then returns a borrow of it for the caller to
    /// render (`Widget for &Editor`) and to query cursor position from via
    /// `Editor::get_visible_cursor`. Mutating through `&self` is safe here
    /// because the offsets are a pure function of (cursor, area), cached
    /// rather than authoritative — see the module doc comment.
    ///
    /// The offsets are reset to zero before `focus` runs so that function
    /// always recomputes them from scratch against the *current* `area`,
    /// rather than nudging over whatever offsets a previous, differently
    /// sized `area` left behind. `Editor::focus` only ever grows an offset
    /// far enough to bring the cursor back into view; it never shrinks one
    /// just because the viewport grew, so without this reset, a pane that's
    /// resized larger after being scrolled in a smaller one can leave a
    /// wide blank margin even though the whole document would now fit.
    pub(crate) fn sync_for_render(&self, area: Rect) -> Ref<'_, Editor> {
        let mut editor = self.editor.borrow_mut();
        editor.set_offset_x(0);
        editor.set_offset_y(0);
        editor.focus(&area);
        drop(editor);
        self.editor.borrow()
    }

    /// Applies `op`, returning whether it actually changed the source (as
    /// opposed to only moving the cursor or selection, or being a boundary
    /// edit that had nothing to do — `Backspace` at the start of the
    /// document, `DeleteForward` at its end, `UnIndent` on an unindented
    /// line). Callers use this to decide whether a prior validation result
    /// is still trustworthy.
    ///
    /// `InsertText` and `Delete` (the library actions backing
    /// `Insert`/`Newline`/`Backspace`/`DeleteForward`) already replace an
    /// active selection when applied, so those ops need no special-casing
    /// here beyond routing selection-aware deletion (see `has_selection`
    /// use in `Backspace`/`DeleteForward` below) — typing or deleting over
    /// a selection "just works" via the library's own behavior. `Insert`
    /// and `Newline` compare source before/after rather than assuming a
    /// change, because replacing an active selection with identical
    /// content (typing the same character back over it, `Enter` over an
    /// already-selected lone newline) is a real case now that selection
    /// exists, and must not spuriously invalidate an accurate validation.
    pub(crate) fn apply(&mut self, op: EditOp) -> bool {
        let changed = match op {
            EditOp::Insert(c) => {
                let before = self.source();
                self.editor.get_mut().apply(InsertText {
                    text: c.to_string(),
                });
                self.source() != before
            }
            // A bare newline, not the library's `InsertNewline` action:
            // that action auto-indents from the current line, a new
            // editing feature out of scope for this issue.
            EditOp::Newline => {
                let before = self.source();
                self.editor.get_mut().apply(InsertText {
                    text: "\n".to_string(),
                });
                self.source() != before
            }
            EditOp::Backspace => {
                let editor = self.editor.get_mut();
                if Self::has_active_selection(editor) || editor.get_cursor() != 0 {
                    editor.apply(Delete {});
                    true
                } else {
                    false
                }
            }
            EditOp::DeleteForward => {
                let editor = self.editor.get_mut();
                if Self::has_active_selection(editor) {
                    editor.apply(Delete {});
                    true
                } else {
                    let cursor = editor.get_cursor();
                    if cursor == editor.code_ref().len_chars() {
                        false
                    } else {
                        let next = editor.code_ref().next_grapheme_boundary(cursor);
                        editor.set_cursor(next);
                        editor.apply(Delete {});
                        true
                    }
                }
            }
            EditOp::MoveLeft(shift) => {
                self.editor.get_mut().apply(MoveLeft { shift });
                false
            }
            EditOp::MoveRight(shift) => {
                self.editor.get_mut().apply(MoveRight { shift });
                false
            }
            EditOp::MoveUp(shift) => {
                self.move_vertical(Direction::Backward, shift);
                return false;
            }
            EditOp::MoveDown(shift) => {
                self.move_vertical(Direction::Forward, shift);
                return false;
            }
            EditOp::MoveLineStart(shift) => {
                self.move_line_start(shift);
                false
            }
            EditOp::MoveLineEnd(shift) => {
                self.move_line_end(shift);
                false
            }
            EditOp::PageUp(shift) => {
                self.move_page(Direction::Backward, shift);
                return false;
            }
            EditOp::PageDown(shift) => {
                self.move_page(Direction::Forward, shift);
                return false;
            }
            EditOp::MoveWordLeft(shift) => {
                self.move_word(Direction::Backward, shift);
                false
            }
            EditOp::MoveWordRight(shift) => {
                self.move_word(Direction::Forward, shift);
                false
            }
            EditOp::SelectAll => {
                self.editor.get_mut().apply(SelectAll {});
                false
            }
            EditOp::Undo => {
                let before = self.source();
                self.editor.get_mut().apply(Undo {});
                self.source() != before
            }
            EditOp::Redo => {
                let before = self.source();
                self.editor.get_mut().apply(Redo {});
                self.source() != before
            }
            EditOp::Indent => {
                let before = self.source();
                self.editor.get_mut().apply(Indent {});
                self.source() != before
            }
            EditOp::UnIndent => {
                let before = self.source();
                self.editor.get_mut().apply(UnIndent {});
                self.source() != before
            }
        };
        self.sync_preferred_col();
        changed
    }

    /// Whether the editor currently has a non-empty selection.
    fn has_active_selection(editor: &mut Editor) -> bool {
        editor.get_selection().is_some_and(|s| !s.is_empty())
    }

    /// The current selection's text, or `None` if there is no active
    /// selection. Exposed for tests and future rendering needs; never
    /// leaks the library's `Selection` type itself.
    #[cfg(test)]
    pub(crate) fn selected_text(&mut self) -> Option<String> {
        self.editor.get_mut().get_selection_text()
    }

    /// Inserts `text` verbatim at the cursor as a single operation, used for
    /// pasted content. Returns whether the source actually changed (`false`
    /// for empty `text`, or for a paste that replaces an active selection
    /// with identical content).
    pub(crate) fn insert_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let before = self.source();
        self.editor.get_mut().apply(InsertText {
            text: text.to_string(),
        });
        self.sync_preferred_col();
        self.source() != before
    }

    /// Replaces the working source with `source`, discarding cursor
    /// position, selection, and undo history, and placing the cursor at the
    /// end. The library has no in-place "replace and reset everything" API
    /// (`set_content` is a plain edit transaction that leaves history/
    /// cursor untouched), so this reconstructs a fresh `Editor`, matching
    /// the strategy proven in `tests/editor_foundation_contract.rs`.
    pub(crate) fn reset(&mut self, source: &str) {
        *self = ControllerDocument::new(source);
    }

    fn move_line_start(&mut self, shift: bool) {
        let editor = self.editor.get_mut();
        let code = editor.code_ref();
        let line = code.char_to_line(editor.get_cursor());
        let start = code.line_to_char(line);
        Self::set_cursor_with_selection(editor, start, shift);
    }

    fn move_line_end(&mut self, shift: bool) {
        let editor = self.editor.get_mut();
        let code = editor.code_ref();
        let line = code.char_to_line(editor.get_cursor());
        let end = code.line_to_char(line) + code.line_len(line);
        Self::set_cursor_with_selection(editor, end, shift);
    }

    /// Moves the cursor one line up/down, landing as close as possible to
    /// `preferred_col` without exceeding the target line's length, then
    /// leaves `preferred_col` untouched (skips the caller's usual
    /// `sync_preferred_col` via an early `return` in `apply`) so repeated
    /// vertical moves through shorter lines still remember how far right
    /// the cursor started.
    ///
    /// At the first/last line there is no line to move to, but an
    /// unshifted move must still clear any active selection — matching
    /// every other unshifted movement key — rather than silently leaving a
    /// stale selection behind for the next keystroke to replace.
    fn move_vertical(&mut self, direction: Direction, shift: bool) {
        let editor = self.editor.get_mut();
        let code = editor.code_ref();
        let line = code.char_to_line(editor.get_cursor());
        let last_line = code.len_lines();
        let target_line = match direction {
            Direction::Backward if line == 0 => None,
            Direction::Backward => Some(line - 1),
            Direction::Forward if line + 1 >= last_line => None,
            Direction::Forward => Some(line + 1),
        };
        match target_line {
            Some(target_line) => self.move_to(target_line, self.preferred_col, shift),
            None if !shift => self.editor.get_mut().clear_selection(),
            None => {}
        }
    }

    fn move_page(&mut self, direction: Direction, shift: bool) {
        let editor = self.editor.get_mut();
        let code = editor.code_ref();
        let line = code.char_to_line(editor.get_cursor());
        let last_line = code.len_lines().saturating_sub(1);
        let target_line = match direction {
            Direction::Backward => line.saturating_sub(PAGE_LINES),
            Direction::Forward => (line + PAGE_LINES).min(last_line),
        };
        self.move_to(target_line, self.preferred_col, shift);
    }

    /// Moves the cursor to `target_col` chars into `target_line`, clamped
    /// to the line's length and then snapped to the nearest grapheme
    /// boundary at or before that column. A raw char-count clamp can land
    /// inside a multi-codepoint grapheme (e.g. a base character plus a
    /// combining mark) on a *different* line than `target_col` was
    /// measured on, since line lengths and grapheme composition vary line
    /// to line; landing there would violate the grapheme-safe movement
    /// contract in `docs/TUI_DESIGN.md` ("Minimum editor experience") that
    /// `MoveLeft`/`MoveRight` already honor via the library's own
    /// grapheme-boundary movement.
    fn move_to(&mut self, target_line: usize, target_col: usize, shift: bool) {
        let editor = self.editor.get_mut();
        let code = editor.code_ref();
        let target_line_start = code.line_to_char(target_line);
        let target_line_len = code.line_len(target_line);
        let clamped_col = target_col.min(target_line_len);
        let raw_offset = target_line_start + clamped_col;

        let mut boundary = target_line_start;
        loop {
            let next = code.next_grapheme_boundary(boundary);
            if next > raw_offset || next == boundary {
                break;
            }
            boundary = next;
        }

        Self::set_cursor_with_selection(editor, boundary, shift);
    }

    /// Moves the cursor to the start of the previous word (`Backward`) or
    /// the end of the next word (`Forward`), matching an ordinary
    /// terminal/editor `Ctrl+Left`/`Ctrl+Right`. No library action exists
    /// for this (`ratatui_code_editor::actions` has no word-move variant),
    /// so it walks `code_ref()` by hand: skip any run of whitespace
    /// adjacent to the cursor in the direction of travel, then skip the
    /// following run of same-kind characters (word characters — alphanumeric
    /// or `_`, matching `Code::word_boundaries` — versus punctuation are
    /// each their own kind, so e.g. `foo.bar` stops at `.` as well as at
    /// each identifier). Steps by `next`/`prev_grapheme_boundary`, not raw
    /// char indices, so a multi-`char` grapheme (a base character plus a
    /// combining mark) is classified and crossed as one unit rather than
    /// leaving the cursor split partway through it — the same grapheme-safe
    /// contract `move_to` already honors for vertical movement.
    fn move_word(&mut self, direction: Direction, shift: bool) {
        let editor = self.editor.get_mut();
        let code = editor.code_ref();
        let cursor = editor.get_cursor();
        let len = code.len_chars();
        let target = match direction {
            Direction::Backward => Self::word_left(code, cursor),
            Direction::Forward => Self::word_right(code, cursor, len),
        };
        Self::set_cursor_with_selection(editor, target, shift);
    }

    fn char_kind(code: &ratatui_code_editor::code::Code, idx: usize) -> CharKind {
        let c =
            code.slice(idx, idx + 1).chars().next().expect(
                "idx is within bounds, so the one-char slice always yields exactly one char",
            );
        if c.is_whitespace() {
            CharKind::Whitespace
        } else if c.is_alphanumeric() || c == '_' {
            CharKind::Word
        } else {
            CharKind::Punctuation
        }
    }

    fn word_left(code: &ratatui_code_editor::code::Code, cursor: usize) -> usize {
        let mut pos = cursor;
        loop {
            if pos == 0 {
                return 0;
            }
            let prev = code.prev_grapheme_boundary(pos);
            if Self::char_kind(code, prev) != CharKind::Whitespace {
                break;
            }
            pos = prev;
        }
        let kind = Self::char_kind(code, code.prev_grapheme_boundary(pos));
        loop {
            if pos == 0 {
                break;
            }
            let prev = code.prev_grapheme_boundary(pos);
            if Self::char_kind(code, prev) != kind {
                break;
            }
            pos = prev;
        }
        pos
    }

    fn word_right(code: &ratatui_code_editor::code::Code, cursor: usize, len: usize) -> usize {
        let mut pos = cursor;
        while pos < len && Self::char_kind(code, pos) == CharKind::Whitespace {
            pos = code.next_grapheme_boundary(pos);
        }
        if pos >= len {
            return len;
        }
        let kind = Self::char_kind(code, pos);
        while pos < len && Self::char_kind(code, pos) == kind {
            pos = code.next_grapheme_boundary(pos);
        }
        pos
    }

    /// Extends the selection to `new_cursor` (preserving the existing
    /// anchor, or anchoring at the current cursor if there was no active
    /// selection) when `shift`, otherwise clears any selection — matching
    /// the library's own `MoveLeft`/`MoveRight { shift }` actions — then
    /// moves the cursor. Selection must be updated before the cursor so
    /// `extend_selection`'s "anchor at the current cursor" fallback reads
    /// the *old* position, not `new_cursor`.
    fn set_cursor_with_selection(editor: &mut Editor, new_cursor: usize, shift: bool) {
        if shift {
            editor.extend_selection(new_cursor);
        } else {
            editor.clear_selection();
        }
        editor.set_cursor(new_cursor);
    }

    fn sync_preferred_col(&mut self) {
        let (_, col) = self.cursor_line_col();
        self.preferred_col = col;
    }
}

#[derive(PartialEq, Eq)]
enum CharKind {
    Whitespace,
    Word,
    Punctuation,
}

enum Direction {
    Backward,
    Forward,
}

impl std::fmt::Debug for ControllerDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerDocument")
            .field("source", &self.source())
            .field("cursor", &self.cursor_line_col())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_places_the_cursor_at_the_end_of_the_source() {
        let doc = ControllerDocument::new("abc");
        assert_eq!(doc.source(), "abc");
        assert_eq!(doc.cursor_line_col(), (0, 3));
    }

    #[test]
    fn exact_source_round_trip_including_empty_and_trailing_newline() {
        for src in ["", "a", "a\nb", "a\nb\n", "\n", "a\n\n"] {
            let doc = ControllerDocument::new(src);
            assert_eq!(doc.source(), src, "round trip failed for {src:?}");
        }
    }

    #[test]
    fn insert_inserts_at_the_cursor_and_reports_a_change() {
        let mut doc = ControllerDocument::new("ac");
        doc.apply(EditOp::MoveLeft(false));
        assert!(doc.apply(EditOp::Insert('b')));
        assert_eq!(doc.source(), "abc");
        assert_eq!(doc.cursor_line_col(), (0, 2));
    }

    #[test]
    fn newline_inserts_a_bare_newline_with_no_auto_indent() {
        let mut doc = ControllerDocument::new("  a");
        assert!(doc.apply(EditOp::Newline));
        assert_eq!(doc.source(), "  a\n");
    }

    #[test]
    fn backspace_at_start_of_document_is_a_no_op_and_reports_no_change() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveLineStart(false));
        assert!(!doc.apply(EditOp::Backspace));
        assert_eq!(doc.source(), "abc");
    }

    #[test]
    fn backspace_joins_the_previous_line_and_reports_a_change() {
        let mut doc = ControllerDocument::new("a\nb");
        doc.apply(EditOp::MoveLineStart(false));
        assert!(doc.apply(EditOp::Backspace));
        assert_eq!(doc.source(), "ab");
        assert_eq!(doc.cursor_line_col(), (0, 1));
    }

    #[test]
    fn delete_forward_at_end_of_document_is_a_no_op_and_reports_no_change() {
        let mut doc = ControllerDocument::new("abc");
        assert!(!doc.apply(EditOp::DeleteForward));
        assert_eq!(doc.source(), "abc");
    }

    #[test]
    fn delete_forward_removes_the_next_character_and_reports_a_change() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveLineStart(false));
        assert!(doc.apply(EditOp::DeleteForward));
        assert_eq!(doc.source(), "bc");
        assert_eq!(doc.cursor_line_col(), (0, 0));
    }

    #[test]
    fn move_left_and_right_stay_within_bounds() {
        let mut doc = ControllerDocument::new("ab");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveLeft(false));
        assert_eq!(doc.cursor_line_col(), (0, 0));

        doc.apply(EditOp::MoveRight(false));
        doc.apply(EditOp::MoveRight(false));
        doc.apply(EditOp::MoveRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 2));
    }

    #[test]
    fn move_up_and_down_track_a_preferred_column_through_shorter_lines() {
        let mut doc = ControllerDocument::new("abcdef\nxy\nghijkl");
        // cursor starts at end of the last line, column 6
        doc.apply(EditOp::MoveUp(false)); // onto "xy" (len 2), clamped to column 2
        assert_eq!(doc.cursor_line_col(), (1, 2));

        doc.apply(EditOp::MoveUp(false)); // back onto "abcdef", remembers column 6
        assert_eq!(doc.cursor_line_col(), (0, 6));

        doc.apply(EditOp::MoveDown(false)); // onto "xy" again, clamped to 2
        assert_eq!(doc.cursor_line_col(), (1, 2));

        doc.apply(EditOp::MoveDown(false)); // back onto "ghijkl", remembers column 6
        assert_eq!(doc.cursor_line_col(), (2, 6));
    }

    #[test]
    fn vertical_movement_snaps_the_target_column_to_a_grapheme_boundary() {
        // "é" here is the base character `e` plus a combining acute accent
        // — a two-`char` grapheme cluster. Column 1 sits between the two
        // `char`s that make it up; landing there would split the cluster.
        let mut doc = ControllerDocument::new("e\u{0301}y\nx");
        assert_eq!(doc.cursor_line_col(), (1, 1)); // end of "x"

        doc.apply(EditOp::MoveUp(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 0),
            "column 1 falls inside the e+combining-accent grapheme; the \
             cursor must snap back to its start (column 0), not split it"
        );
    }

    #[test]
    fn move_up_at_first_line_is_a_no_op() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveUp(false));
        assert_eq!(doc.cursor_line_col(), (0, 3));
    }

    #[test]
    fn move_down_at_last_line_is_a_no_op() {
        let mut doc = ControllerDocument::new("abc\ndef");
        doc.apply(EditOp::MoveDown(false));
        assert_eq!(doc.cursor_line_col(), (1, 3));
    }

    #[test]
    fn move_line_start_and_end() {
        let mut doc = ControllerDocument::new("abc\ndef");
        doc.apply(EditOp::MoveLineStart(false));
        assert_eq!(doc.cursor_line_col(), (1, 0));

        doc.apply(EditOp::MoveLineEnd(false));
        assert_eq!(doc.cursor_line_col(), (1, 3));
    }

    #[test]
    fn page_up_and_down_clamp_to_document_bounds() {
        let long = (0..20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut doc = ControllerDocument::new(&long);
        for _ in 0..20 {
            doc.apply(EditOp::MoveUp(false));
        }
        doc.apply(EditOp::MoveLineStart(false));
        assert_eq!(doc.cursor_line_col(), (0, 0));

        doc.apply(EditOp::PageDown(false));
        assert_eq!(doc.cursor_line_col().0, 10);

        doc.apply(EditOp::PageDown(false));
        assert_eq!(doc.cursor_line_col().0, 19);

        doc.apply(EditOp::PageUp(false));
        assert_eq!(doc.cursor_line_col().0, 9);

        doc.apply(EditOp::PageUp(false));
        assert_eq!(doc.cursor_line_col().0, 0);
    }

    #[test]
    fn insert_text_preserves_embedded_newlines_and_whitespace() {
        let mut doc = ControllerDocument::new("");
        assert!(doc.insert_text("line one\n  line two\n"));
        assert_eq!(doc.source(), "line one\n  line two\n");
    }

    #[test]
    fn insert_text_with_empty_string_is_a_noop_and_returns_false() {
        let mut doc = ControllerDocument::new("abc");
        assert!(!doc.insert_text(""));
        assert_eq!(doc.source(), "abc");
    }

    #[test]
    fn reset_replaces_source_and_places_cursor_at_the_end() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveLineStart(false));
        doc.reset("xyz");
        assert_eq!(doc.source(), "xyz");
        assert_eq!(doc.cursor_line_col(), (0, 3));
    }

    #[test]
    fn reset_is_exact_including_trailing_newline_and_discards_undo_history() {
        let mut doc = ControllerDocument::new("original");
        doc.apply(EditOp::Insert('!'));
        let new_source = "starter\ntext\n";
        doc.reset(new_source);
        assert_eq!(doc.source(), new_source);
        // Trailing newline means the cursor lands on the empty third line.
        assert_eq!(doc.cursor_line_col(), (2, 0));
    }

    #[test]
    fn reset_synchronizes_the_preferred_column_with_the_new_cursor_position() {
        // Without this, the next vertical move after a reset jumps to
        // whatever column an edit before the reset happened to leave
        // behind (or column 0, if none), rather than the cursor's actual
        // displayed column on the replacement source's final line.
        let mut doc = ControllerDocument::new("a\nbb");
        doc.apply(EditOp::MoveLineStart(false)); // column 0, not the end of the document
        doc.reset("first\nsecond\nthird");

        doc.apply(EditOp::MoveUp(false));
        assert_eq!(
            doc.cursor_line_col(),
            (1, "third".chars().count()),
            "Up should land at the reset cursor's actual column (end of \
             the last line, \"third\"), not column 0; \"second\" is long \
             enough that this column wouldn't be reached by clamping alone"
        );
    }

    #[test]
    fn shift_movement_creates_a_selection_and_typing_replaces_it() {
        let mut doc = ControllerDocument::new("abcdef");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveRight(true));
        doc.apply(EditOp::MoveRight(true));
        doc.apply(EditOp::MoveRight(true));
        assert_eq!(doc.selected_text().as_deref(), Some("abc"));

        assert!(doc.apply(EditOp::Insert('X')));
        assert_eq!(doc.source(), "Xdef");
        assert_eq!(doc.selected_text(), None, "typing clears the selection");
    }

    #[test]
    fn replacing_a_selection_with_identical_content_reports_no_change() {
        // Typing the same character back over a selected one, or pasting
        // identical text over an identical selection, leaves the final
        // source unchanged even though a mutation happened in between —
        // that must not spuriously invalidate an otherwise-accurate
        // validation result (`docs/TUI_DESIGN.md`, "Modified state and
        // validation invalidation").
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveRight(true));
        assert_eq!(doc.selected_text().as_deref(), Some("a"));
        assert!(!doc.apply(EditOp::Insert('a')));
        assert_eq!(doc.source(), "abc");

        let mut newline_doc = ControllerDocument::new("a\nb");
        newline_doc.apply(EditOp::MoveLineStart(false));
        newline_doc.apply(EditOp::MoveLeft(true));
        assert_eq!(newline_doc.selected_text().as_deref(), Some("\n"));
        assert!(!newline_doc.apply(EditOp::Newline));
        assert_eq!(newline_doc.source(), "a\nb");

        let mut paste_doc = ControllerDocument::new("abc");
        paste_doc.apply(EditOp::MoveLineStart(false));
        paste_doc.apply(EditOp::MoveRight(true));
        assert_eq!(paste_doc.selected_text().as_deref(), Some("a"));
        assert!(!paste_doc.insert_text("a"));
        assert_eq!(paste_doc.source(), "abc");
    }

    #[test]
    fn unshifted_vertical_movement_at_a_document_boundary_still_clears_a_selection() {
        // MoveUp/MoveDown return early at the first/last line without
        // moving the cursor, but an unshifted move must still clear any
        // active selection there, matching every other unshifted movement
        // key — otherwise the next typed character or deletion would still
        // replace a selection the player believes they've backed out of.
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveRight(true));
        assert_eq!(doc.selected_text().as_deref(), Some("a"));

        doc.apply(EditOp::MoveUp(false)); // already at the first line
        assert_eq!(
            doc.selected_text(),
            None,
            "MoveUp at line 0 must clear the selection"
        );

        doc.apply(EditOp::MoveLineEnd(false));
        doc.apply(EditOp::MoveLeft(true));
        assert!(doc.selected_text().is_some());
        doc.apply(EditOp::MoveDown(false)); // "abc" is a single line, so this is also the last line
        assert_eq!(
            doc.selected_text(),
            None,
            "MoveDown at the last line must clear the selection"
        );
    }

    #[test]
    fn backspace_and_delete_forward_remove_an_active_selection() {
        let mut doc = ControllerDocument::new("abcdef");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveRight(true));
        doc.apply(EditOp::MoveRight(true));
        assert!(doc.apply(EditOp::Backspace));
        assert_eq!(doc.source(), "cdef");

        doc.apply(EditOp::MoveRight(false));
        doc.apply(EditOp::MoveLineEnd(true));
        assert_eq!(doc.selected_text().as_deref(), Some("def"));
        assert!(doc.apply(EditOp::DeleteForward));
        assert_eq!(doc.source(), "c");
    }

    #[test]
    fn shift_left_from_column_zero_can_leave_the_cursor_at_zero_with_a_selection() {
        // Regression guard: Backspace must still delete a selection whose
        // cursor happens to be at document position 0 (e.g. after
        // shift-selecting backward to the start), not treat cursor == 0 as
        // "nothing to delete" the way it does with no selection at all.
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveLeft(true));
        doc.apply(EditOp::MoveLeft(true));
        doc.apply(EditOp::MoveLeft(true));
        assert_eq!(doc.cursor_line_col(), (0, 0));
        assert_eq!(doc.selected_text().as_deref(), Some("abc"));
        assert!(doc.apply(EditOp::Backspace));
        assert_eq!(doc.source(), "");
    }

    #[test]
    fn select_all_selects_the_whole_document_and_reports_no_content_change() {
        let mut doc = ControllerDocument::new("line one\nline two");
        assert!(!doc.apply(EditOp::SelectAll));
        assert_eq!(doc.selected_text().as_deref(), Some("line one\nline two"));
        assert_eq!(doc.source(), "line one\nline two");
    }

    #[test]
    fn select_all_then_backspace_clears_the_buffer() {
        let mut doc = ControllerDocument::new("line one\nline two");
        doc.apply(EditOp::SelectAll);
        assert!(doc.apply(EditOp::Backspace));
        assert_eq!(doc.source(), "");
    }

    #[test]
    fn cursor_only_movement_reports_no_change_even_with_a_selection_active() {
        let mut doc = ControllerDocument::new("abcdef");
        doc.apply(EditOp::MoveLineStart(false));
        assert!(!doc.apply(EditOp::MoveRight(true)));
        assert!(!doc.apply(EditOp::MoveLeft(false)));
        assert!(!doc.apply(EditOp::SelectAll));
    }

    #[test]
    fn undo_and_redo_round_trip_a_content_change() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::Insert('!'));
        assert_eq!(doc.source(), "abc!");

        assert!(doc.apply(EditOp::Undo));
        assert_eq!(doc.source(), "abc");

        assert!(doc.apply(EditOp::Redo));
        assert_eq!(doc.source(), "abc!");
    }

    #[test]
    fn undo_with_empty_history_is_a_no_op_and_reports_no_change() {
        let mut doc = ControllerDocument::new("abc");
        assert!(!doc.apply(EditOp::Undo));
        assert_eq!(doc.source(), "abc");
    }

    #[test]
    fn redo_with_no_undone_edit_is_a_no_op_and_reports_no_change() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::Insert('!'));
        assert!(!doc.apply(EditOp::Redo));
        assert_eq!(doc.source(), "abc!");
    }

    #[test]
    fn indent_inserts_two_spaces_for_lua_never_a_literal_tab() {
        let mut doc = ControllerDocument::new("x");
        doc.apply(EditOp::MoveLineStart(false));
        assert!(doc.apply(EditOp::Indent));
        assert_eq!(doc.source(), "  x");
        assert!(!doc.source().contains('\t'));
    }

    #[test]
    fn indent_with_a_multiline_selection_indents_every_selected_line_as_one_undo_step() {
        let mut doc = ControllerDocument::new("aaa\nbbb\nccc");
        // Cursor starts at the document's end, (2, 3). Move up to the end
        // of "bbb", then shift-move up again to the end of "aaa" so the
        // selection spans exactly the first two lines and does not touch
        // "ccc" at all.
        doc.apply(EditOp::MoveUp(false));
        doc.apply(EditOp::MoveUp(true));
        assert_eq!(doc.selected_text().as_deref(), Some("\nbbb"));

        assert!(doc.apply(EditOp::Indent));
        assert_eq!(doc.source(), "  aaa\n  bbb\nccc");

        assert!(doc.apply(EditOp::Undo));
        assert_eq!(
            doc.source(),
            "aaa\nbbb\nccc",
            "indenting a selection must undo as a single step, restoring every line at once"
        );
    }

    #[test]
    fn unindent_removes_one_indent_unit_and_is_a_no_op_on_an_unindented_line() {
        let mut doc = ControllerDocument::new("    x");
        doc.apply(EditOp::MoveLineStart(false));
        assert!(doc.apply(EditOp::UnIndent));
        assert_eq!(doc.source(), "  x");

        let mut unindented = ControllerDocument::new("x");
        unindented.apply(EditOp::MoveLineStart(false));
        assert!(!unindented.apply(EditOp::UnIndent));
        assert_eq!(unindented.source(), "x");
    }

    #[test]
    fn word_movement_stops_at_word_and_whitespace_boundaries() {
        let mut doc = ControllerDocument::new("foo.bar  baz");
        doc.apply(EditOp::MoveLineStart(false));

        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 3), "stops after \"foo\"");

        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 4), "stops after the \".\"");

        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 7), "stops after \"bar\"");

        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 12),
            "skips the whitespace run and stops after \"baz\""
        );

        doc.apply(EditOp::MoveWordLeft(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 9),
            "back to the start of \"baz\""
        );

        doc.apply(EditOp::MoveWordLeft(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 4),
            "skips the whitespace run and lands at the start of \"bar\""
        );
    }

    #[test]
    fn word_movement_clamps_at_document_boundaries() {
        let mut doc = ControllerDocument::new("word");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveWordLeft(false));
        assert_eq!(doc.cursor_line_col(), (0, 0), "already at the start");

        doc.apply(EditOp::MoveLineEnd(false));
        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 4), "already at the end");
    }

    #[test]
    fn word_movement_treats_a_combining_mark_as_part_of_its_base_characters_grapheme() {
        // "é" here is the base character `e` plus a combining acute accent
        // — a two-`char` grapheme cluster, the same construction used by
        // `vertical_movement_snaps_the_target_column_to_a_grapheme_boundary`
        // above. Classifying each `char` on its own (rather than stepping
        // by grapheme boundaries) would treat the combining mark as its
        // own non-word "kind" and stop word movement between the two
        // `char`s, splitting the grapheme in two; word movement must
        // instead cross "é" and the following "f" as one unbroken word.
        let mut doc = ControllerDocument::new("e\u{0301}f bar");
        doc.apply(EditOp::MoveLineStart(false));

        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 3),
            "lands right after \"f\", never between \"e\" and its combining accent"
        );

        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 7), "stops after \"bar\"");

        doc.apply(EditOp::MoveWordLeft(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 4),
            "back to the start of \"bar\""
        );

        doc.apply(EditOp::MoveWordLeft(false));
        assert_eq!(
            doc.cursor_line_col(),
            (0, 0),
            "back to the start of the grapheme cluster, not split partway through it"
        );
    }

    #[test]
    fn word_movement_on_an_empty_document_is_a_no_op() {
        let mut doc = ControllerDocument::new("");
        doc.apply(EditOp::MoveWordLeft(false));
        assert_eq!(doc.cursor_line_col(), (0, 0));
        doc.apply(EditOp::MoveWordRight(false));
        assert_eq!(doc.cursor_line_col(), (0, 0));
    }

    #[test]
    fn shift_word_movement_extends_a_selection() {
        let mut doc = ControllerDocument::new("foo bar");
        doc.apply(EditOp::MoveLineStart(false));
        doc.apply(EditOp::MoveWordRight(true));
        assert_eq!(doc.selected_text().as_deref(), Some("foo"));
    }
}
