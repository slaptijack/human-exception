//! Retained evidence for issue #90's editor-foundation decision: proves
//! `ratatui-textarea` against the repo's locked `ratatui`/`crossterm` stack
//! and the editor contract in `docs/TUI_DESIGN.md` ("The editor contract").
//!
//! `ratatui-textarea` was adopted over `ratatui-code-editor` (heavy
//! mandatory tree-sitter/clipboard dependencies, no Lua grammar, `Ctrl+V`
//! hardwired to paste, 0.0.x maturity) and `edtui` (its only modeless
//! keymap has no selection at all; selection only exists behind the modal
//! Vim Normal/Visual flow the epic excludes) and over extending the
//! bespoke `src/console/editor.rs`, which has neither selection nor
//! undo/redo today. See the issue for the full comparison.
//!
//! This is not yet wired into the Controller (`src/console/editor.rs` is
//! still the live implementation) -- that integration is #91 onward.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_textarea::TextArea;

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn type_str(ta: &mut TextArea, s: &str) {
    for c in s.chars() {
        ta.input(key(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

#[test]
fn renders_via_test_backend() {
    let ta = TextArea::from(["fn on_tick() end"]);
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| f.render_widget(&ta, f.area())).unwrap();
    let buf = terminal.backend().buffer().clone();
    let rendered: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(rendered.contains("fn on_tick"));
}

#[test]
fn selection_and_typed_replacement() {
    let mut ta = TextArea::from(["hello world"]);
    // Move to start, shift-select "hello", type replacement.
    ta.input(key(KeyCode::Home, KeyModifiers::NONE));
    for _ in 0..5 {
        ta.input(key(KeyCode::Right, KeyModifiers::SHIFT));
    }
    assert!(ta.is_selecting());
    ta.input(key(KeyCode::Char('h'), KeyModifiers::NONE));
    ta.input(key(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(ta.lines(), ["hi world"]);
}

#[test]
fn select_all_and_backspace_clears_buffer() {
    let mut ta = TextArea::from(["line one", "line two"]);
    ta.select_all();
    ta.input(key(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(ta.lines(), [""]);
}

#[test]
fn undo_redo_each_keystroke_is_its_own_step_but_paste_is_one_step() {
    // Each discrete keystroke insert is its own undo step here (undoing
    // "abc" typed as three key events takes three undos) -- unlike a single
    // programmatic multi-char insert (paste), which undoes in one step.
    // Contract-relevant: whichever direction #91/#93 want, this library's
    // granularity is keystroke-level for typed input, not run-coalesced.
    let mut ta = TextArea::from([""]);
    type_str(&mut ta, "abc");
    assert_eq!(ta.lines(), ["abc"]);
    assert!(ta.undo());
    assert_eq!(
        ta.lines(),
        ["ab"],
        "typed keystrokes undo one char at a time"
    );
    assert!(ta.undo());
    assert!(ta.undo());
    assert_eq!(ta.lines(), [""]);

    ta.insert_str("line one\nline two");
    assert_eq!(ta.lines(), ["line one", "line two"]);
    assert!(ta.undo());
    assert_eq!(
        ta.lines(),
        [""],
        "multiline paste (insert_str) should undo as a single step"
    );
    assert!(ta.redo());
    assert_eq!(ta.lines(), ["line one", "line two"]);
}

#[test]
fn multiline_paste_via_insert_str() {
    let mut ta = TextArea::from([""]);
    ta.insert_str("alpha\nbeta\ngamma");
    assert_eq!(ta.lines(), ["alpha", "beta", "gamma"]);
}

#[test]
fn unicode_combining_marks_and_wide_glyphs_round_trip() {
    let src = "e\u{0301}\u{4e16}\u{754c}"; // "é" (combining) + "世界"
    let ta = TextArea::from([src]);
    assert_eq!(ta.lines(), [src]);
}

#[test]
fn exact_source_round_trip_including_empty_and_trailing_newline() {
    for src in ["", "a", "a\nb", "a\nb\n", "\n", "a\n\n"] {
        let ta = TextArea::from(src.split('\n'));
        let extracted = ta.lines().join("\n");
        assert_eq!(extracted, src, "round trip failed for {src:?}");
    }
}

#[test]
fn ctrl_v_is_not_claimed_by_default_keymap() {
    let mut ta = TextArea::from(["unchanged"]);
    ta.set_yank_text("PASTED");
    let changed = ta.input(key(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert!(
        !changed,
        "default keymap must not consume Ctrl+V (reserved for validate); got lines: {:?}",
        ta.lines()
    );
    assert_eq!(ta.lines(), ["unchanged"]);
}

#[test]
fn text_area_state_is_send_but_not_sync() {
    // TextArea caches an internal `Cell<Rect>` for its last-rendered layout,
    // which makes it Send but not Sync. Fine for this single-threaded TUI
    // app (no thread::spawn/Sync requirement found in src/console/*.rs),
    // but worth recording as an ownership constraint.
    fn assert_send<T: Send>() {}
    assert_send::<TextArea>();
}
