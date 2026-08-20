//! The authoritative Controller working document.
//!
//! Wraps `ratatui_code_editor::editor::Editor` (the embedded editor
//! foundation selected in issue #90) behind a narrow, application-owned
//! seam: callers see `EditOp`/`String`/`(usize, usize)`, never `Code`,
//! `Selection`, or any other library-internal type. This is the sole
//! authoritative Lua working buffer for the Controller view — no
//! synchronized `String` mirror exists alongside it.
//!
//! Rendering (`super::ui::draw_controller_source`) still computes its own
//! rows/gutter/viewport from [`ControllerDocument::source`] and
//! [`ControllerDocument::cursor_line_col`], as it did against the previous
//! bespoke editor; migrating rendering to the library's own widget is #92.
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

use ratatui_code_editor::actions::{Delete, InsertText, MoveLeft, MoveRight};
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
    editor: Editor,
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
        let editor = Editor::new("lua", source, vec![]).expect("editor construction must not fail");
        let mut doc = ControllerDocument {
            editor,
            preferred_col: 0,
        };
        doc.editor.set_cursor(source.chars().count());
        doc.sync_preferred_col();
        doc
    }

    /// The exact current working source, including any trailing newline.
    pub(crate) fn source(&self) -> String {
        self.editor.get_content()
    }

    /// 0-based `(line, column)` of the cursor, both counted in `char`s.
    pub(crate) fn cursor_line_col(&self) -> (usize, usize) {
        self.editor.code_ref().point(self.editor.get_cursor())
    }

    /// Applies `op`, returning whether it actually changed the source (as
    /// opposed to only moving the cursor, or being a boundary edit that had
    /// nothing to do — `Backspace` at the start of the document,
    /// `DeleteForward` at its end). Callers use this to decide whether a
    /// prior validation result is still trustworthy.
    pub(crate) fn apply(&mut self, op: EditOp) -> bool {
        let changed = match op {
            EditOp::Insert(c) => {
                self.editor.apply(InsertText {
                    text: c.to_string(),
                });
                true
            }
            // A bare newline, not the library's `InsertNewline` action:
            // that action auto-indents from the current line, a new
            // editing feature out of scope for this issue.
            EditOp::Newline => {
                self.editor.apply(InsertText {
                    text: "\n".to_string(),
                });
                true
            }
            EditOp::Backspace => {
                if self.editor.get_cursor() == 0 {
                    false
                } else {
                    self.editor.apply(Delete {});
                    true
                }
            }
            EditOp::DeleteForward => {
                let cursor = self.editor.get_cursor();
                if cursor == self.editor.code_ref().len_chars() {
                    false
                } else {
                    let next = self.editor.code_ref().next_grapheme_boundary(cursor);
                    self.editor.set_cursor(next);
                    self.editor.apply(Delete {});
                    true
                }
            }
            EditOp::MoveLeft => {
                self.editor.apply(MoveLeft { shift: false });
                false
            }
            EditOp::MoveRight => {
                self.editor.apply(MoveRight { shift: false });
                false
            }
            EditOp::MoveUp => {
                self.move_vertical(Vertical::Up);
                return false;
            }
            EditOp::MoveDown => {
                self.move_vertical(Vertical::Down);
                return false;
            }
            EditOp::MoveLineStart => {
                self.move_line_start();
                false
            }
            EditOp::MoveLineEnd => {
                self.move_line_end();
                false
            }
            EditOp::PageUp => {
                self.move_page(Vertical::Up);
                return false;
            }
            EditOp::PageDown => {
                self.move_page(Vertical::Down);
                return false;
            }
        };
        self.sync_preferred_col();
        changed
    }

    /// Inserts `text` verbatim at the cursor as a single operation, used for
    /// pasted content. Returns whether the source actually changed (`false`
    /// for empty `text`).
    pub(crate) fn insert_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.editor.apply(InsertText {
            text: text.to_string(),
        });
        self.sync_preferred_col();
        true
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

    fn move_line_start(&mut self) {
        let code = self.editor.code_ref();
        let line = code.char_to_line(self.editor.get_cursor());
        let start = code.line_to_char(line);
        self.editor.set_cursor(start);
    }

    fn move_line_end(&mut self) {
        let code = self.editor.code_ref();
        let line = code.char_to_line(self.editor.get_cursor());
        let end = code.line_to_char(line) + code.line_len(line);
        self.editor.set_cursor(end);
    }

    /// Moves the cursor one line up/down, landing as close as possible to
    /// `preferred_col` without exceeding the target line's length, then
    /// leaves `preferred_col` untouched (skips the caller's usual
    /// `sync_preferred_col` via an early `return` in `apply`) so repeated
    /// vertical moves through shorter lines still remember how far right
    /// the cursor started.
    fn move_vertical(&mut self, direction: Vertical) {
        let code = self.editor.code_ref();
        let line = code.char_to_line(self.editor.get_cursor());
        let target_line = match direction {
            Vertical::Up if line == 0 => return,
            Vertical::Up => line - 1,
            Vertical::Down if line + 1 >= code.len_lines() => return,
            Vertical::Down => line + 1,
        };
        self.move_to(target_line, self.preferred_col);
    }

    fn move_page(&mut self, direction: Vertical) {
        let code = self.editor.code_ref();
        let line = code.char_to_line(self.editor.get_cursor());
        let last_line = code.len_lines().saturating_sub(1);
        let target_line = match direction {
            Vertical::Up => line.saturating_sub(PAGE_LINES),
            Vertical::Down => (line + PAGE_LINES).min(last_line),
        };
        self.move_to(target_line, self.preferred_col);
    }

    fn move_to(&mut self, target_line: usize, target_col: usize) {
        let code = self.editor.code_ref();
        let target_line_start = code.line_to_char(target_line);
        let target_line_len = code.line_len(target_line);
        let clamped_col = target_col.min(target_line_len);
        self.editor.set_cursor(target_line_start + clamped_col);
    }

    fn sync_preferred_col(&mut self) {
        let (_, col) = self.cursor_line_col();
        self.preferred_col = col;
    }
}

enum Vertical {
    Up,
    Down,
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
        doc.apply(EditOp::MoveLeft);
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
        doc.apply(EditOp::MoveLineStart);
        assert!(!doc.apply(EditOp::Backspace));
        assert_eq!(doc.source(), "abc");
    }

    #[test]
    fn backspace_joins_the_previous_line_and_reports_a_change() {
        let mut doc = ControllerDocument::new("a\nb");
        doc.apply(EditOp::MoveLineStart);
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
        doc.apply(EditOp::MoveLineStart);
        assert!(doc.apply(EditOp::DeleteForward));
        assert_eq!(doc.source(), "bc");
        assert_eq!(doc.cursor_line_col(), (0, 0));
    }

    #[test]
    fn move_left_and_right_stay_within_bounds() {
        let mut doc = ControllerDocument::new("ab");
        doc.apply(EditOp::MoveLineStart);
        doc.apply(EditOp::MoveLeft);
        assert_eq!(doc.cursor_line_col(), (0, 0));

        doc.apply(EditOp::MoveRight);
        doc.apply(EditOp::MoveRight);
        doc.apply(EditOp::MoveRight);
        assert_eq!(doc.cursor_line_col(), (0, 2));
    }

    #[test]
    fn move_up_and_down_track_a_preferred_column_through_shorter_lines() {
        let mut doc = ControllerDocument::new("abcdef\nxy\nghijkl");
        // cursor starts at end of the last line, column 6
        doc.apply(EditOp::MoveUp); // onto "xy" (len 2), clamped to column 2
        assert_eq!(doc.cursor_line_col(), (1, 2));

        doc.apply(EditOp::MoveUp); // back onto "abcdef", remembers column 6
        assert_eq!(doc.cursor_line_col(), (0, 6));

        doc.apply(EditOp::MoveDown); // onto "xy" again, clamped to 2
        assert_eq!(doc.cursor_line_col(), (1, 2));

        doc.apply(EditOp::MoveDown); // back onto "ghijkl", remembers column 6
        assert_eq!(doc.cursor_line_col(), (2, 6));
    }

    #[test]
    fn move_up_at_first_line_is_a_no_op() {
        let mut doc = ControllerDocument::new("abc");
        doc.apply(EditOp::MoveUp);
        assert_eq!(doc.cursor_line_col(), (0, 3));
    }

    #[test]
    fn move_down_at_last_line_is_a_no_op() {
        let mut doc = ControllerDocument::new("abc\ndef");
        doc.apply(EditOp::MoveDown);
        assert_eq!(doc.cursor_line_col(), (1, 3));
    }

    #[test]
    fn move_line_start_and_end() {
        let mut doc = ControllerDocument::new("abc\ndef");
        doc.apply(EditOp::MoveLineStart);
        assert_eq!(doc.cursor_line_col(), (1, 0));

        doc.apply(EditOp::MoveLineEnd);
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
            doc.apply(EditOp::MoveUp);
        }
        doc.apply(EditOp::MoveLineStart);
        assert_eq!(doc.cursor_line_col(), (0, 0));

        doc.apply(EditOp::PageDown);
        assert_eq!(doc.cursor_line_col().0, 10);

        doc.apply(EditOp::PageDown);
        assert_eq!(doc.cursor_line_col().0, 19);

        doc.apply(EditOp::PageUp);
        assert_eq!(doc.cursor_line_col().0, 9);

        doc.apply(EditOp::PageUp);
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
        doc.apply(EditOp::MoveLineStart);
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
        doc.apply(EditOp::MoveLineStart); // column 0, not the end of the document
        doc.reset("first\nsecond\nthird");

        doc.apply(EditOp::MoveUp);
        assert_eq!(
            doc.cursor_line_col(),
            (1, "third".chars().count()),
            "Up should land at the reset cursor's actual column (end of \
             the last line, \"third\"), not column 0; \"second\" is long \
             enough that this column wouldn't be reached by clamping alone"
        );
    }
}
