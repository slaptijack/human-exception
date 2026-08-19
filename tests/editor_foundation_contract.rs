//! Retained evidence for issue #90's editor-foundation decision: proves
//! `ratatui-code-editor` against the repo's locked `ratatui`/`crossterm`
//! stack and the editor contract in `docs/TUI_DESIGN.md` ("The editor
//! contract").
//!
//! `ratatui-code-editor` was adopted over the previously-adopted
//! `ratatui-textarea` (proven against this same contract by an earlier
//! version of this file, but with no highlighting support at all) and
//! `tui-textarea-2` (confirmed to compile against this repo's locked
//! stack and to expose the caller-owned `custom_highlight`/
//! `selection_range`/`set_lines` API this decision needed, but not
//! carried through this file's full behavioral contract, since the
//! decision was made to proceed with `ratatui-code-editor` instead of
//! completing that comparison -- see the issue for the full record).
//! `ratatui-code-editor` has a tree-sitter-backed highlighting story with
//! real per-language grammars, not just a caller-owned range API without
//! any grammar behind it. It's consumed here as a git dependency on
//! `slaptijack/ratatui-code-editor`'s
//! `feature-gate-languages` branch rather than the crates.io release,
//! because the upstream crate (`vipmax/ratatui-code-editor`) bundles all
//! 15 of its Tree-sitter grammars as mandatory dependencies with no way to
//! opt out. The fork (proposed upstream as
//! <https://github.com/vipmax/ratatui-code-editor/issues/14>) makes each
//! grammar an optional `language-<name>` feature; this repo depends on it
//! with `default-features = false` and only the `crossterm` feature
//! enabled, so zero Tree-sitter grammars are pulled in. See the issue for
//! the full comparison. If upstream accepts the fork's changes this
//! dependency can move to a released crates.io version; if not, the fork
//! remains the source for now.
//!
//! Lua highlighting is not yet available: no `language-lua` feature exists
//! on either upstream or the fork (see `missing_lua_grammar_falls_back_to_plain_text`
//! below for what that means in practice today). Adding it is tracked as a
//! follow-up in the same upstream issue and is not required for this
//! decision, consistent with the epic's "highlighting is desirable but not
//! required" scope.
//!
//! This is not yet wired into the Controller (`src/console/editor.rs` is
//! still the live implementation) -- that integration is #91 onward.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui_code_editor::actions::{
    Delete, InsertNewline, InsertText, MoveRight, Redo, SelectAll, Undo,
};
use ratatui_code_editor::editor::Editor;

fn area() -> Rect {
    Rect::new(0, 0, 40, 10)
}

fn editor(text: &str) -> Editor {
    Editor::new("lua", text, vec![]).expect("editor construction must not fail")
}

#[test]
fn renders_via_test_backend() {
    let ed = editor("fn on_tick() end");
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| f.render_widget(&ed, f.area())).unwrap();
    let buf = terminal.backend().buffer().clone();
    let rendered: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(rendered.contains("fn on_tick"));
}

#[test]
fn selection_and_typed_replacement() {
    let mut ed = editor("hello world");
    // Shift-select "hello" from the start, then type a replacement.
    for _ in 0..5 {
        ed.apply(MoveRight { shift: true });
    }
    let sel = ed.get_selection().expect("selection must be active");
    assert!(!sel.is_empty());
    ed.apply(InsertText {
        text: "hi".to_string(),
    });
    assert_eq!(ed.get_content(), "hi world");
    assert!(
        ed.get_selection().is_none(),
        "typed replacement must clear the selection"
    );
}

#[test]
fn select_all_and_delete_clears_buffer() {
    let mut ed = editor("line one\nline two");
    ed.apply(SelectAll {});
    ed.apply(Delete {});
    assert_eq!(ed.get_content(), "");
}

#[test]
fn undo_redo_each_keystroke_is_its_own_step_but_paste_is_one_step() {
    // Each discrete keystroke insert is its own undo step here (undoing
    // "abc" typed as three key events takes three undos) -- unlike a single
    // programmatic multi-char insert (paste), which undoes in one step.
    // Contract-relevant: whichever direction #91/#93 want, this library's
    // granularity is keystroke-level for typed input, not run-coalesced.
    let mut ed = editor("");
    for c in "abc".chars() {
        ed.apply(InsertText {
            text: c.to_string(),
        });
    }
    assert_eq!(ed.get_content(), "abc");
    ed.apply(Undo {});
    assert_eq!(
        ed.get_content(),
        "ab",
        "typed keystrokes undo one char at a time"
    );
    ed.apply(Undo {});
    ed.apply(Undo {});
    assert_eq!(ed.get_content(), "");

    ed.apply(InsertText {
        text: "line one\nline two".to_string(),
    });
    assert_eq!(ed.get_content(), "line one\nline two");
    ed.apply(Undo {});
    assert_eq!(
        ed.get_content(),
        "",
        "multiline paste (a single InsertText) should undo as one step"
    );
    ed.apply(Redo {});
    assert_eq!(ed.get_content(), "line one\nline two");
}

#[test]
fn multiline_paste_via_insert_text() {
    let mut ed = editor("");
    ed.apply(InsertText {
        text: "alpha\nbeta\ngamma".to_string(),
    });
    assert_eq!(ed.get_content(), "alpha\nbeta\ngamma");
}

#[test]
fn unicode_combining_marks_and_wide_glyphs_round_trip() {
    let src = "e\u{0301}\u{4e16}\u{754c}"; // "é" (combining) + "世界"
    let ed = editor(src);
    assert_eq!(ed.get_content(), src);
}

#[test]
fn exact_source_round_trip_including_empty_and_trailing_newline() {
    for src in ["", "a", "a\nb", "a\nb\n", "\n", "a\n\n"] {
        let ed = editor(src);
        assert_eq!(ed.get_content(), src, "round trip failed for {src:?}");
    }
}

#[test]
fn long_line_scrolling_follows_the_cursor() {
    let long_line = "x".repeat(200);
    let mut ed = editor(&long_line);
    ed.set_cursor(long_line.chars().count());
    ed.focus(&area());
    assert!(
        ed.get_offset_x() > 0,
        "horizontal viewport must scroll to keep a far-right cursor visible"
    );
}

#[test]
fn vertical_scrolling_follows_the_cursor_down_a_tall_buffer() {
    let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    let mut ed = editor(&lines.join("\n"));
    ed.set_cursor(ed.get_content().chars().count());
    ed.focus(&area());
    assert!(
        ed.get_offset_y() > 0,
        "vertical viewport must scroll to keep the last line visible"
    );
}

#[test]
fn missing_lua_grammar_falls_back_to_plain_text() {
    // No `language-lua` feature exists yet on the fork this repo depends
    // on (see the file-level doc comment). `Editor::new` falls back to the
    // "text" language rather than failing, so the editor is fully usable
    // -- just without Lua syntax highlighting -- until that follow-up
    // lands upstream.
    let ed = Editor::new("lua", "function f() end", vec![])
        .expect("missing grammar must fall back, not error");
    assert_eq!(ed.get_content(), "function f() end");
}

// `Editor::input`'s convenience keymap hardwires Ctrl+V to its `Paste`
// action (src/editor_crossterm.rs), matching the finding recorded for
// `ratatui-textarea` 0.9.2 in the previous version of this file (which
// turned out to be true there too: Ctrl+V mapped to PageDown, not
// "unclaimed"). Unlike `ratatui-textarea`'s Ctrl+V (a harmless no-op
// PageDown against an in-memory buffer), this library's `Paste` action
// reads the real OS clipboard via `arboard::Clipboard` first and only
// falls back to an internal buffer if that fails (`Editor::get_clipboard`,
// `src/editor.rs`) -- confirmed empirically while writing this test, which
// is why there is no runtime test here exercising `input()` with Ctrl+V:
// doing so reads (and `Copy`/`Cut` would write) the *host's actual system
// clipboard*, which is both nondeterministic across environments (CI runs
// headless on ubuntu-latest, where `arboard::Clipboard::new()` fails and
// the internal fallback kicks in; a local run does not) and not something
// a test should be touching regardless.
//
// None of that matters for this repo's actual guarantee, which is
// unchanged from the previous library and lives structurally in
// `src/console/event.rs`'s `map` function: the Ctrl+V arm
// (`event.rs:117-119`) is matched before the pane-local dispatch wildcard
// arm (`event.rs:145`), so no Ctrl+V key event is ever passed to
// `Editor::input` at all, regardless of what its keymap does with it or
// what clipboard backend it reaches for. That guarantee is exercised
// end-to-end by `event.rs`'s own tests
// (`ctrl_v_also_validates_as_a_fallback_for_terminals_without_ctrl_enter`
// and friends), not here. Likewise, #94's bracketed-paste integration
// should insert pasted text via `InsertText` directly (as
// `multiline_paste_via_insert_text` above does), not via this library's
// `Paste` action -- bracketed paste delivers text the terminal already
// captured, and has no business reading the OS clipboard a second time.

#[test]
fn insert_newline_auto_indents_from_the_current_line() {
    let mut ed = editor("  a");
    ed.set_cursor(ed.get_content().chars().count());
    ed.apply(InsertNewline {});
    assert_eq!(ed.get_content(), "  a\n  ");
}

// `Editor` is neither `Send` nor `Sync`: its `Code` caches a per-language
// `Rc<RefCell<tree_sitter::Parser>>` and holds a boxed `dyn Fn` change
// callback, neither of which is `Send`. (Confirmed empirically: a
// `fn assert_send<T: Send>() { assert_send::<Editor>(); }` here fails to
// compile with exactly those two non-Send types named in the error.) This
// is a stronger constraint than `ratatui-textarea` 0.9.2, which was `Send`
// (just not `Sync`). Fine for this single-threaded TUI app -- no
// `thread::spawn`/`Send`/`Sync` requirement found in `src/console/*.rs` --
// but worth recording accurately rather than assuming parity with the
// previously-evaluated library. There's no first-party way to assert a
// *negative* trait bound as a passing `#[test]` without a compile-fail
// testing crate, so this is recorded as a comment rather than a test.
