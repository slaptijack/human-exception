//! Maps terminal key events onto the console's player intents.
//!
//! Kept separate from [`super::state`] so the transition logic stays free of
//! any terminal-library types, and separate from rendering so key bindings
//! can be unit tested by constructing `KeyEvent`s directly.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::editor::EditOp;
use super::navigation;
use super::state::{Msg, PaneId, View};

/// Maps a key event to a player intent, given the view currently showing
/// (several keys are context-sensitive: `F1`/`Esc` need to know whether
/// Help is open, `Esc`/arrows/`Enter` behave differently in Signals, Target,
/// and Help), whether Network Bootstrap is currently owning the interface,
/// whether any confirmation dialog is currently open, whether the
/// first-launch bootstrap introduction is currently showing, which pane
/// is currently focused — pane-local input (Signals selection, Controller
/// editing, After Action scrolling) is only routed to the pane that owns
/// it, regardless of what layout width currently has it on screen — and
/// whether the currently rendered surface presents a real multi-pane focus
/// choice (`AppState::focus_movement_available`), which gates `F8`.
// One parameter over clippy's default threshold — each is an independent,
// differently-typed fact `AppState` tracks (not a natural group to bundle
// into a struct just to satisfy the lint), and this function is still a
// pure, exhaustively-tested `(inputs) -> Option<Msg>` mapping.
#[allow(clippy::too_many_arguments)]
pub fn map(
    key: KeyEvent,
    current_view: View,
    network_bootstrap_pending: bool,
    bootstrap_intro_visible: bool,
    reset_confirmation_pending: bool,
    quit_confirmation_pending: bool,
    redeploy_confirmation_pending: bool,
    focused_pane: PaneId,
    focus_movement_available: bool,
) -> Option<Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if key.code == KeyCode::Char('q') && ctrl {
        return Some(Msg::RequestQuit);
    }

    let unmodified = key.modifiers.is_empty();

    // Network Bootstrap advances entirely on its own timer
    // (`AppState::advance_network_bootstrap`, driven by `event_loop`) and
    // has no player-facing dismissal at all — `docs/TUI_DESIGN.md`'s
    // "Network Bootstrap" requires that "ordinary gameplay/navigation/
    // editing input is suppressed while it owns the interface; the only
    // input honored is the existing quit-safety behavior," already handled
    // by the always-global `Ctrl+Q` check above. So every other key is
    // simply swallowed, with no dismissal arm at all.
    if network_bootstrap_pending {
        return None;
    }

    // The bootstrap introduction is a must-acknowledge gate shown before
    // the Player has interacted with the console at all: every key but its
    // own unmodified `Enter` (and the always-global `Ctrl+Q` above) is
    // swallowed, the same "block everything but dismissal" rule the
    // confirmation dialogs below apply once the console is otherwise
    // usable.
    if bootstrap_intro_visible {
        return match key.code {
            KeyCode::Enter if unmodified => Some(Msg::AcknowledgeBootstrapIntro),
            _ => None,
        };
    }

    // A confirmation dialog swallows every key except its own unmodified
    // yes/no so the player can't accidentally act past it (e.g. keep
    // typing Lua while a reset prompt is showing) or trigger it by
    // accident via some other binding's modified form — `Ctrl+Enter`
    // (validate) and `Ctrl+Y` both have unmodified forms that must not
    // silently confirm a destructive action instead.
    if quit_confirmation_pending {
        return match key.code {
            KeyCode::Enter | KeyCode::Char('y') if unmodified => Some(Msg::ConfirmQuit),
            KeyCode::Esc | KeyCode::Char('n') if unmodified => Some(Msg::CancelQuit),
            _ => None,
        };
    }
    if reset_confirmation_pending {
        return match key.code {
            KeyCode::Enter | KeyCode::Char('y') if unmodified => Some(Msg::ConfirmResetController),
            KeyCode::Esc | KeyCode::Char('n') if unmodified => Some(Msg::CancelResetController),
            _ => None,
        };
    }
    if redeploy_confirmation_pending {
        return match key.code {
            KeyCode::Enter | KeyCode::Char('y') if unmodified => Some(Msg::ConfirmDeploy),
            KeyCode::Esc | KeyCode::Char('n') if unmodified => Some(Msg::CancelDeploy),
            _ => None,
        };
    }

    let help_is_open = current_view == View::Help;
    let controller_is_open = current_view == View::Controller;
    let operation_is_open = current_view == View::Operation;

    match key.code {
        KeyCode::F(1) => Some(if help_is_open {
            Msg::DismissHelp
        } else {
            Msg::OpenHelp
        }),
        KeyCode::Esc if help_is_open => Some(Msg::DismissHelp),
        KeyCode::Esc if current_view == View::Target => Some(Msg::Navigate(View::Signals)),
        KeyCode::F(2) => Some(Msg::Navigate(View::Signals)),
        KeyCode::F(3) => Some(Msg::Navigate(View::Target)),
        KeyCode::F(4) => Some(Msg::Navigate(View::Controller)),
        KeyCode::F(5) => Some(Msg::Navigate(View::Operation)),
        // Global, like the rest of the F-keys (`docs/TUI_DESIGN.md`'s
        // "Overall navigation" list) — reachable from Controller (the
        // common case) as well as from Operation itself (redeploy) or
        // anywhere else once a working set exists. `Msg::RequestDeploy`
        // itself is a no-op without a loaded controller source.
        KeyCode::F(6) => Some(Msg::RequestDeploy),
        KeyCode::F(7) if controller_is_open => Some(Msg::RequestResetController),
        // `F8` moves focus to the next pane in every current two-pane view
        // (`docs/TUI_DESIGN.md`, "F8 -- next pane"). `focus_movement_available`
        // is `false` for Help, for the
        // Operation/After Action placeholders before any deployment
        // exists, and while a confirmation dialog is pending (though those
        // are already filtered out above) — in each case `F8` stays inert
        // rather than mapping to a message that would be a no-op, or worse,
        // silently move hidden focus.
        KeyCode::F(8) if focus_movement_available => Some(Msg::FocusNextPane),
        // `Ctrl+V` is the advertised binding — an ordinary control character
        // every terminal sends correctly, so it works everywhere. `Ctrl+Enter`
        // is still accepted here too, on terminals capable of reporting it
        // distinctly from plain `Enter` via the Kitty keyboard protocol (see
        // `console::run`'s best-effort attempt to enable it), but it is not
        // required or advertised: without the Kitty protocol, most terminals
        // report it identically to plain `Enter`, so relying on it would
        // leave those players with no way to validate at all.
        KeyCode::Enter | KeyCode::Char('v') if ctrl && controller_is_open => {
            Some(Msg::ValidateController)
        }
        KeyCode::Enter
            if !ctrl
                && (current_view == View::Target
                    || (current_view == View::Signals && focused_pane == PaneId::SignalsList)) =>
        {
            Some(Msg::Activate)
        }
        // `Enter`/`Space` while Operation is showing: pacing controls, not
        // navigation (`docs/TUI_DESIGN.md`, "Pacing controls").
        // `Msg::StepOperationTick` is itself a no-op unless the run is
        // paused, so `Enter` here doesn't fast-forward a running operation.
        KeyCode::Enter if !ctrl && operation_is_open => Some(Msg::StepOperationTick),
        KeyCode::Char(' ') if !ctrl && operation_is_open => Some(Msg::TogglePauseOperation),
        // The shared, focus-aware navigation vocabulary
        // (`docs/TUI_DESIGN.md`, "Console-wide navigation") for every
        // non-editor surface: which surface, if any, owns these keys is
        // decided once by `navigation::focused_nav_surface` rather than by
        // a separate ad hoc `(View, PaneId)` gate per view, and each
        // surface interprets the resulting intent as the same `Msg` it
        // always has. The page-move intents carry a placeholder `0`;
        // `console::mod`'s dispatch loop rewrites it from real frame
        // geometry before calling `apply` (see `Msg`'s doc comment) since
        // this function has no access to rendered geometry. A surface with
        // no meaning for a given intent (e.g. Signals has no PageUp/Home
        // binding yet) falls out as `None` from `navigation::route` itself,
        // not from this arm's guard.
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End
            if navigation::focused_nav_surface(current_view, focused_pane).is_some() =>
        {
            let surface = navigation::focused_nav_surface(current_view, focused_pane)
                .expect("guard just checked this is Some");
            let intent = navigation::intent_for_key(key.code)
                .expect("key.code is one of the arm's patterns");
            navigation::route(surface, intent)
        }
        // Toggles the Run Inspector between TIMELINE and SOURCE
        // (`docs/TUI_DESIGN.md`, "Review Run"). Always produces the same
        // `Msg` regardless of which mode is currently active — the mode
        // itself, and whether the run has actually finished, are decided in
        // `AppState::apply`, matching the chronology keys above. Which
        // family of `Msg` the *other* Run-Inspector keys above produce while
        // SOURCE is active is decided in `console::mod`'s dispatch loop
        // (it already has real `AppState` access to check the mode; this
        // function deliberately doesn't take on that dependency), not here.
        KeyCode::Tab
            if navigation::focused_nav_surface(current_view, focused_pane)
                == Some(navigation::NavSurface::ReviewRun) =>
        {
            Some(Msg::ToggleRunInspectorMode)
        }
        // Ordinary editing keys must not silently mutate the source unless
        // it's the pane actually focused — i.e. the reference pane, not the
        // source, is focused (which `F8` can move to).
        _ if controller_is_open && focused_pane == PaneId::ControllerSource => {
            // AltGr (used on many non-US keyboard layouts to type
            // punctuation like `{`, `}`, `[`, `]`, `\`, `@`) is reported by
            // some terminals — notably on Windows — as `CONTROL | ALT`
            // rather than a distinct modifier, indistinguishable at this
            // level from an actual Ctrl+Alt chord. None of this console's
            // own bindings use Alt at all, so a `Char` arriving with ALT
            // held alongside CONTROL is always AltGr-produced printable
            // input here, never a real control binding, and must still be
            // inserted rather than silently dropped.
            let altgr = ctrl && key.modifiers.contains(KeyModifiers::ALT);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            map_controller_edit(key.code, ctrl, shift, altgr)
        }
        _ => None,
    }
}

/// Ordinary editing/cursor-movement keys, only reachable once Controller is
/// showing and neither confirmation dialog is open (see [`map`]). `altgr`
/// is set when `ctrl` is only true because of an AltGr chord (`CONTROL |
/// ALT` together), which must still type its printable character. `shift`
/// extends the active selection for movement keys (`docs/TUI_DESIGN.md`,
/// "Minimum editor experience"); it is ignored by non-movement keys.
fn map_controller_edit(code: KeyCode, ctrl: bool, shift: bool, altgr: bool) -> Option<Msg> {
    let op = match code {
        KeyCode::Char(c) if !ctrl || altgr => EditOp::Insert(c),
        // Real Ctrl chords (never AltGr, which the arm above already
        // consumed) for select-all and undo/redo. `Ctrl+Y` is the redo
        // binding, not `Ctrl+Shift+Z`: it's an ordinary control character
        // every terminal reports correctly, whereas `Ctrl+Shift+Z` can
        // arrive indistinguishable from plain `Ctrl+Z` without an extended
        // keyboard protocol, which would silently undo instead of redo.
        KeyCode::Char('a') if ctrl => EditOp::SelectAll,
        KeyCode::Char('z') if ctrl => EditOp::Undo,
        KeyCode::Char('y') if ctrl => EditOp::Redo,
        // `Tab`/`Shift+Tab` indent/unindent the current line, or every
        // line an active selection touches, by one language-appropriate
        // unit (two spaces for Lua, never a literal tab byte) — see
        // `ControllerDocument::apply`'s `EditOp::Indent`/`UnIndent`.
        // Crossterm reports Shift+Tab as the distinct `BackTab` key code
        // rather than `Tab` with a Shift modifier bit.
        KeyCode::Tab => EditOp::Indent,
        KeyCode::BackTab => EditOp::UnIndent,
        KeyCode::Enter => EditOp::Newline,
        KeyCode::Backspace => EditOp::Backspace,
        KeyCode::Delete => EditOp::DeleteForward,
        // `Ctrl+Left`/`Ctrl+Right` jump by word; plain arrows move one
        // grapheme. Both forms carry `shift` through to extend or clear
        // the active selection, matching every other movement key here.
        KeyCode::Left if ctrl => EditOp::MoveWordLeft(shift),
        KeyCode::Right if ctrl => EditOp::MoveWordRight(shift),
        KeyCode::Left => EditOp::MoveLeft(shift),
        KeyCode::Right => EditOp::MoveRight(shift),
        KeyCode::Up => EditOp::MoveUp(shift),
        KeyCode::Down => EditOp::MoveDown(shift),
        KeyCode::Home => EditOp::MoveLineStart(shift),
        KeyCode::End => EditOp::MoveLineEnd(shift),
        KeyCode::PageUp => EditOp::PageUp(shift),
        KeyCode::PageDown => EditOp::PageDown(shift),
        _ => return None,
    };
    Some(Msg::EditController(op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// Shorthand for the common case: no confirmation dialog is open,
    /// `view`'s default pane is focused, and — matching every view except
    /// Help having a real multi-pane composition on screen once a
    /// deployment exists — focus movement is available everywhere but
    /// Help. Tests that need a single-content Operation/After Action
    /// placeholder call `map` directly instead.
    fn map_in(key: KeyEvent, view: View) -> Option<Msg> {
        map(
            key,
            view,
            false,
            false,
            false,
            false,
            false,
            view.default_pane(),
            view != View::Help,
        )
    }

    #[test]
    fn ctrl_q_requests_quit_regardless_of_help_state() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);

        assert_eq!(map_in(quit, View::Signals), Some(Msg::RequestQuit));
        assert_eq!(map_in(quit, View::Help), Some(Msg::RequestQuit));
    }

    #[test]
    fn plain_q_does_not_quit() {
        assert_eq!(map_in(key(KeyCode::Char('q')), View::Signals), None);
    }

    #[test]
    fn f1_opens_help_when_closed_and_dismisses_when_open() {
        assert_eq!(
            map_in(key(KeyCode::F(1)), View::Signals),
            Some(Msg::OpenHelp)
        );
        assert_eq!(
            map_in(key(KeyCode::F(1)), View::Help),
            Some(Msg::DismissHelp)
        );
    }

    #[test]
    fn esc_dismisses_help_when_open() {
        assert_eq!(
            map_in(key(KeyCode::Esc), View::Help),
            Some(Msg::DismissHelp)
        );
    }

    #[test]
    fn esc_returns_to_signals_from_target() {
        assert_eq!(
            map_in(key(KeyCode::Esc), View::Target),
            Some(Msg::Navigate(View::Signals))
        );
    }

    #[test]
    fn esc_does_nothing_in_signals_or_controller() {
        assert_eq!(map_in(key(KeyCode::Esc), View::Signals), None);
        assert_eq!(map_in(key(KeyCode::Esc), View::Controller), None);
    }

    #[test]
    fn function_keys_navigate_to_their_view() {
        assert_eq!(
            map_in(key(KeyCode::F(2)), View::Signals),
            Some(Msg::Navigate(View::Signals))
        );
        assert_eq!(
            map_in(key(KeyCode::F(3)), View::Signals),
            Some(Msg::Navigate(View::Target))
        );
        assert_eq!(
            map_in(key(KeyCode::F(4)), View::Signals),
            Some(Msg::Navigate(View::Controller))
        );
        assert_eq!(
            map_in(key(KeyCode::F(5)), View::Signals),
            Some(Msg::Navigate(View::Operation))
        );
    }

    #[test]
    fn f6_maps_to_request_deploy_regardless_of_view() {
        // `Msg::RequestDeploy` itself is a no-op in `AppState::apply` until
        // a controller source exists to deploy — `map` doesn't need to know
        // that, matching every other global F-key. See also
        // `f6_requests_deploy_from_any_view` for the full view sweep.
        assert_eq!(
            map_in(key(KeyCode::F(6)), View::Signals),
            Some(Msg::RequestDeploy)
        );
        assert_eq!(
            map_in(key(KeyCode::F(6)), View::Controller),
            Some(Msg::RequestDeploy)
        );
    }

    #[test]
    fn f7_resets_the_controller_only_while_controller_is_open() {
        assert_eq!(
            map_in(key(KeyCode::F(7)), View::Controller),
            Some(Msg::RequestResetController)
        );
        assert_eq!(map_in(key(KeyCode::F(7)), View::Signals), None);
    }

    #[test]
    fn ctrl_enter_validates_only_while_controller_is_open() {
        let ctrl_enter = key_with_modifiers(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(
            map_in(ctrl_enter, View::Controller),
            Some(Msg::ValidateController)
        );
        assert_eq!(map_in(ctrl_enter, View::Signals), None);
    }

    #[test]
    fn ctrl_v_also_validates_as_a_fallback_for_terminals_without_ctrl_enter() {
        let ctrl_v = key_with_modifiers(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(
            map_in(ctrl_v, View::Controller),
            Some(Msg::ValidateController)
        );
        assert_eq!(map_in(ctrl_v, View::Signals), None);
    }

    #[test]
    fn altgr_produced_characters_are_still_inserted_in_the_editor() {
        // Some terminals (notably on Windows) report AltGr as `CONTROL |
        // ALT` rather than a distinct modifier, indistinguishable at this
        // level from an actual Ctrl+Alt chord — but this console has no
        // Alt-based bindings at all, so any such `Char` must still type,
        // not be swallowed as if it were an unrecognized control shortcut.
        let altgr_at = key_with_modifiers(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(
            map_in(altgr_at, View::Controller),
            Some(Msg::EditController(EditOp::Insert('@')))
        );
    }

    #[test]
    fn plain_ctrl_characters_are_still_rejected_by_the_editor() {
        // Unlike the AltGr case above, an ordinary Ctrl+<letter> chord
        // (ALT not held) that isn't one of the console's own bindings must
        // still be swallowed rather than typed — it's not printable input
        // in that case, matching every terminal's real Ctrl+letter chords.
        let ctrl_x = key_with_modifiers(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(map_in(ctrl_x, View::Controller), None);
    }

    #[test]
    fn editing_keys_are_inert_while_the_reference_pane_is_focused_instead_of_source() {
        let reference_focused = PaneId::LuaFieldReference;
        assert_eq!(
            map(
                key(KeyCode::Char('x')),
                View::Controller,
                false,
                false,
                false,
                false,
                false,
                reference_focused,
                true
            ),
            None
        );
        assert_eq!(
            map(
                key(KeyCode::Backspace),
                View::Controller,
                false,
                false,
                false,
                false,
                false,
                reference_focused,
                true
            ),
            None
        );
        assert_eq!(
            map(
                key(KeyCode::Left),
                View::Controller,
                false,
                false,
                false,
                false,
                false,
                reference_focused,
                true
            ),
            None
        );
    }

    #[test]
    fn f7_and_validate_still_work_while_the_reference_pane_is_focused() {
        let reference_focused = PaneId::LuaFieldReference;
        assert_eq!(
            map(
                key(KeyCode::F(7)),
                View::Controller,
                false,
                false,
                false,
                false,
                false,
                reference_focused,
                true
            ),
            Some(Msg::RequestResetController)
        );
        let ctrl_enter = key_with_modifiers(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(
            map(
                ctrl_enter,
                View::Controller,
                false,
                false,
                false,
                false,
                false,
                reference_focused,
                true
            ),
            Some(Msg::ValidateController)
        );
    }

    #[test]
    fn f8_moves_focus_to_the_next_pane() {
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Signals),
            Some(Msg::FocusNextPane)
        );
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Target),
            Some(Msg::FocusNextPane)
        );
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Controller),
            Some(Msg::FocusNextPane)
        );
    }

    #[test]
    fn f8_is_inert_outside_signals_target_controller_operation_and_after_action() {
        assert_eq!(map_in(key(KeyCode::F(8)), View::Help), None);
    }

    #[test]
    fn f8_is_inert_on_the_operation_placeholder_before_any_deploy() {
        assert_eq!(
            map(
                key(KeyCode::F(8)),
                View::Operation,
                false,
                false,
                false,
                false,
                false,
                View::Operation.default_pane(),
                false
            ),
            None
        );
    }

    #[test]
    fn f8_is_inert_on_the_after_action_placeholder_before_any_conclusion() {
        assert_eq!(
            map(
                key(KeyCode::F(8)),
                View::AfterAction,
                false,
                false,
                false,
                false,
                false,
                View::AfterAction.default_pane(),
                false
            ),
            None
        );
    }

    #[test]
    fn enter_activates_in_signals_and_target() {
        assert_eq!(
            map_in(key(KeyCode::Enter), View::Signals),
            Some(Msg::Activate)
        );
        assert_eq!(
            map_in(key(KeyCode::Enter), View::Target),
            Some(Msg::Activate)
        );
        assert_eq!(map_in(key(KeyCode::Enter), View::Help), None);
    }

    #[test]
    fn enter_inserts_a_newline_in_controller() {
        assert_eq!(
            map_in(key(KeyCode::Enter), View::Controller),
            Some(Msg::EditController(EditOp::Newline))
        );
    }

    #[test]
    fn printable_characters_insert_in_controller_but_nowhere_else() {
        assert_eq!(
            map_in(key(KeyCode::Char('x')), View::Controller),
            Some(Msg::EditController(EditOp::Insert('x')))
        );
        assert_eq!(map_in(key(KeyCode::Char('x')), View::Signals), None);
    }

    #[test]
    fn editing_and_movement_keys_map_in_controller() {
        let cases = [
            (KeyCode::Backspace, EditOp::Backspace),
            (KeyCode::Delete, EditOp::DeleteForward),
            (KeyCode::Left, EditOp::MoveLeft(false)),
            (KeyCode::Right, EditOp::MoveRight(false)),
            (KeyCode::Up, EditOp::MoveUp(false)),
            (KeyCode::Down, EditOp::MoveDown(false)),
            (KeyCode::Home, EditOp::MoveLineStart(false)),
            (KeyCode::End, EditOp::MoveLineEnd(false)),
            (KeyCode::PageUp, EditOp::PageUp(false)),
            (KeyCode::PageDown, EditOp::PageDown(false)),
            (KeyCode::Tab, EditOp::Indent),
            (KeyCode::BackTab, EditOp::UnIndent),
        ];
        for (code, op) in cases {
            assert_eq!(
                map_in(key(code), View::Controller),
                Some(Msg::EditController(op)),
                "{code:?} should map to {op:?} in Controller"
            );
        }
    }

    #[test]
    fn shift_movement_keys_extend_selection_in_controller() {
        let cases = [
            (KeyCode::Left, EditOp::MoveLeft(true)),
            (KeyCode::Right, EditOp::MoveRight(true)),
            (KeyCode::Up, EditOp::MoveUp(true)),
            (KeyCode::Down, EditOp::MoveDown(true)),
            (KeyCode::Home, EditOp::MoveLineStart(true)),
            (KeyCode::End, EditOp::MoveLineEnd(true)),
            (KeyCode::PageUp, EditOp::PageUp(true)),
            (KeyCode::PageDown, EditOp::PageDown(true)),
        ];
        for (code, op) in cases {
            let shifted = key_with_modifiers(code, KeyModifiers::SHIFT);
            assert_eq!(
                map_in(shifted, View::Controller),
                Some(Msg::EditController(op)),
                "Shift+{code:?} should map to {op:?} in Controller"
            );
        }
    }

    #[test]
    fn ctrl_left_and_right_move_by_word_in_controller() {
        let ctrl_left = key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL);
        let ctrl_right = key_with_modifiers(KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(
            map_in(ctrl_left, View::Controller),
            Some(Msg::EditController(EditOp::MoveWordLeft(false)))
        );
        assert_eq!(
            map_in(ctrl_right, View::Controller),
            Some(Msg::EditController(EditOp::MoveWordRight(false)))
        );

        let ctrl_shift_left =
            key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(
            map_in(ctrl_shift_left, View::Controller),
            Some(Msg::EditController(EditOp::MoveWordLeft(true)))
        );
    }

    #[test]
    fn ctrl_a_selects_all_in_controller() {
        let ctrl_a = key_with_modifiers(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(
            map_in(ctrl_a, View::Controller),
            Some(Msg::EditController(EditOp::SelectAll))
        );
    }

    #[test]
    fn ctrl_z_undoes_and_ctrl_y_redoes_in_controller() {
        let ctrl_z = key_with_modifiers(KeyCode::Char('z'), KeyModifiers::CONTROL);
        let ctrl_y = key_with_modifiers(KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert_eq!(
            map_in(ctrl_z, View::Controller),
            Some(Msg::EditController(EditOp::Undo))
        );
        assert_eq!(
            map_in(ctrl_y, View::Controller),
            Some(Msg::EditController(EditOp::Redo))
        );
    }

    #[test]
    fn new_rich_editing_keys_are_inert_while_the_reference_pane_is_focused() {
        let reference_focused = PaneId::LuaFieldReference;
        let cases = [
            key_with_modifiers(KeyCode::Left, KeyModifiers::SHIFT),
            key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL),
            key_with_modifiers(KeyCode::Char('a'), KeyModifiers::CONTROL),
            key_with_modifiers(KeyCode::Char('z'), KeyModifiers::CONTROL),
            key_with_modifiers(KeyCode::Char('y'), KeyModifiers::CONTROL),
            key(KeyCode::Tab),
            key(KeyCode::BackTab),
        ];
        for k in cases {
            assert_eq!(
                map(
                    k,
                    View::Controller,
                    false,
                    false,
                    false,
                    false,
                    false,
                    reference_focused,
                    true
                ),
                None,
                "{k:?} should be inert while the reference pane is focused"
            );
        }
    }

    #[test]
    fn arrows_move_signal_selection_only_in_signals() {
        assert_eq!(
            map_in(key(KeyCode::Up), View::Signals),
            Some(Msg::SelectPreviousSignal)
        );
        assert_eq!(
            map_in(key(KeyCode::Down), View::Signals),
            Some(Msg::SelectNextSignal)
        );
        assert_eq!(map_in(key(KeyCode::Up), View::Target), None);
    }

    #[test]
    fn page_and_home_end_keys_move_signal_selection_only_in_signals() {
        assert_eq!(
            map_in(key(KeyCode::PageUp), View::Signals),
            Some(Msg::SelectSignalPageBackward(0))
        );
        assert_eq!(
            map_in(key(KeyCode::PageDown), View::Signals),
            Some(Msg::SelectSignalPageForward(0))
        );
        assert_eq!(
            map_in(key(KeyCode::Home), View::Signals),
            Some(Msg::SelectFirstSignal)
        );
        assert_eq!(
            map_in(key(KeyCode::End), View::Signals),
            Some(Msg::SelectLastSignal)
        );
        assert_eq!(map_in(key(KeyCode::PageUp), View::Target), None);
        assert_eq!(map_in(key(KeyCode::Home), View::Target), None);
    }

    #[test]
    fn arrows_scroll_help_when_help_is_open() {
        assert_eq!(map_in(key(KeyCode::Up), View::Help), Some(Msg::ScrollUp));
        assert_eq!(
            map_in(key(KeyCode::Down), View::Help),
            Some(Msg::ScrollDown)
        );
    }

    #[test]
    fn signals_selection_and_activation_are_inert_while_the_selected_signal_pane_is_focused() {
        let selected_signal_focused = PaneId::SelectedSignal;
        assert_eq!(
            map(
                key(KeyCode::Up),
                View::Signals,
                false,
                false,
                false,
                false,
                false,
                selected_signal_focused,
                true
            ),
            None
        );
        assert_eq!(
            map(
                key(KeyCode::Down),
                View::Signals,
                false,
                false,
                false,
                false,
                false,
                selected_signal_focused,
                true
            ),
            None
        );
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Signals,
                false,
                false,
                false,
                false,
                false,
                selected_signal_focused,
                true
            ),
            None
        );
        for k in [
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
        ] {
            assert_eq!(
                map(
                    key(k),
                    View::Signals,
                    false,
                    false,
                    false,
                    false,
                    false,
                    selected_signal_focused,
                    true
                ),
                None,
                "{k:?} should be inert while the Selected Signal pane is focused"
            );
        }
    }

    #[test]
    fn target_activation_is_not_gated_by_focus() {
        // Target has no pane-local input at all (`docs/TUI_DESIGN.md`'s
        // "Pane-local vs. view-level input today" table), so `Enter` must
        // fire regardless of which of Target's panes is focused.
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Target,
                false,
                false,
                false,
                false,
                false,
                PaneId::Provenance,
                true
            ),
            Some(Msg::Activate)
        );
    }

    #[test]
    fn unbound_keys_map_to_nothing() {
        assert_eq!(map_in(key(KeyCode::Char('x')), View::Signals), None);
    }

    #[test]
    fn key_release_events_still_map_the_same_as_press() {
        let mut released = key(KeyCode::F(2));
        released.kind = KeyEventKind::Release;

        assert_eq!(
            map_in(released, View::Signals),
            Some(Msg::Navigate(View::Signals))
        );
    }

    #[test]
    fn quit_confirmation_pending_only_accepts_confirm_or_cancel() {
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::ConfirmQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('y')),
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::ConfirmQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Esc),
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::CancelQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('n')),
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::CancelQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('x')),
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            None,
            "ordinary keys must not leak through while the quit dialog is open"
        );
    }

    #[test]
    fn network_bootstrap_pending_swallows_every_key_including_enter() {
        for code in [
            KeyCode::Enter,
            KeyCode::Char('y'),
            KeyCode::Char('n'),
            KeyCode::Esc,
            KeyCode::F(2),
            KeyCode::Char('x'),
        ] {
            assert_eq!(
                map(
                    key(code),
                    View::Signals,
                    true,
                    false,
                    false,
                    false,
                    false,
                    PaneId::SignalsList,
                    true
                ),
                None,
                "Network Bootstrap has no player-facing dismissal — every \
                 key but the always-global Ctrl+Q above must be swallowed"
            );
        }
    }

    #[test]
    fn ctrl_q_still_requests_quit_while_network_bootstrap_is_pending() {
        assert_eq!(
            map(
                key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL),
                View::Signals,
                true,
                false,
                false,
                false,
                false,
                PaneId::SignalsList,
                true
            ),
            Some(Msg::RequestQuit)
        );
    }

    #[test]
    fn bootstrap_intro_visible_only_accepts_unmodified_enter() {
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Signals,
                false,
                true,
                false,
                false,
                false,
                PaneId::SignalsList,
                true
            ),
            Some(Msg::AcknowledgeBootstrapIntro)
        );
        for code in [
            KeyCode::Char('y'),
            KeyCode::Char('n'),
            KeyCode::Esc,
            KeyCode::Char('q'),
            KeyCode::F(2),
        ] {
            assert_eq!(
                map(
                    key(code),
                    View::Signals,
                    false,
                    true,
                    false,
                    false,
                    false,
                    PaneId::SignalsList,
                    true
                ),
                None,
                "ordinary keys must not leak through while the bootstrap intro is open"
            );
        }
    }

    #[test]
    fn ctrl_enter_does_not_acknowledge_the_bootstrap_intro() {
        assert_eq!(
            map(
                key_with_modifiers(KeyCode::Enter, KeyModifiers::CONTROL),
                View::Signals,
                false,
                true,
                false,
                false,
                false,
                PaneId::SignalsList,
                true
            ),
            None
        );
    }

    #[test]
    fn ctrl_q_still_requests_quit_while_the_bootstrap_intro_is_open() {
        assert_eq!(
            map(
                key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL),
                View::Signals,
                false,
                true,
                false,
                false,
                false,
                PaneId::SignalsList,
                true
            ),
            Some(Msg::RequestQuit)
        );
    }

    #[test]
    fn reset_confirmation_pending_only_accepts_confirm_or_cancel() {
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::ConfirmResetController)
        );
        assert_eq!(
            map(
                key(KeyCode::Esc),
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::CancelResetController)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('x')),
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            None,
            "ordinary keys must not leak through while the reset dialog is open"
        );
    }

    #[test]
    fn redeploy_confirmation_pending_only_accepts_confirm_or_cancel() {
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Operation,
                false,
                false,
                false,
                false,
                true,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::ConfirmDeploy)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('y')),
                View::Operation,
                false,
                false,
                false,
                false,
                true,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::ConfirmDeploy)
        );
        assert_eq!(
            map(
                key(KeyCode::Esc),
                View::Operation,
                false,
                false,
                false,
                false,
                true,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::CancelDeploy)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('n')),
                View::Operation,
                false,
                false,
                false,
                false,
                true,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::CancelDeploy)
        );
        assert_eq!(
            map(
                key(KeyCode::Char(' ')),
                View::Operation,
                false,
                false,
                false,
                false,
                true,
                PaneId::ControllerSource,
                true
            ),
            None,
            "Space must not leak through to pause/resume while the redeploy dialog is open"
        );
    }

    #[test]
    fn ctrl_modified_confirm_keys_do_not_confirm_destructive_dialogs() {
        let ctrl_enter = key_with_modifiers(KeyCode::Enter, KeyModifiers::CONTROL);
        let ctrl_y = key_with_modifiers(KeyCode::Char('y'), KeyModifiers::CONTROL);
        let ctrl_esc = key_with_modifiers(KeyCode::Esc, KeyModifiers::CONTROL);
        let ctrl_n = key_with_modifiers(KeyCode::Char('n'), KeyModifiers::CONTROL);

        // Quit dialog: Ctrl+Enter must not silently discard the modified
        // controller just because it would otherwise mean "validate".
        assert_eq!(
            map(
                ctrl_enter,
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
        assert_eq!(
            map(
                ctrl_y,
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
        assert_eq!(
            map(
                ctrl_esc,
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
        assert_eq!(
            map(
                ctrl_n,
                View::Controller,
                false,
                false,
                false,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );

        // Reset dialog: same requirement.
        assert_eq!(
            map(
                ctrl_enter,
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
        assert_eq!(
            map(
                ctrl_y,
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
        assert_eq!(
            map(
                ctrl_esc,
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
        assert_eq!(
            map(
                ctrl_n,
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            None
        );
    }

    #[test]
    fn any_modifier_at_all_blocks_confirmation_dialog_keys() {
        for modifiers in [
            KeyModifiers::SHIFT,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL,
        ] {
            let enter = key_with_modifiers(KeyCode::Enter, modifiers);
            let y = key_with_modifiers(KeyCode::Char('y'), modifiers);
            assert_eq!(
                map(
                    enter,
                    View::Controller,
                    false,
                    false,
                    false,
                    true,
                    false,
                    PaneId::ControllerSource,
                    true
                ),
                None,
                "{modifiers:?}+Enter must not confirm quit"
            );
            assert_eq!(
                map(
                    y,
                    View::Controller,
                    false,
                    false,
                    true,
                    false,
                    false,
                    PaneId::ControllerSource,
                    true
                ),
                None,
                "{modifiers:?}+y must not confirm reset"
            );
        }
    }

    #[test]
    fn quit_confirmation_takes_priority_over_reset_confirmation() {
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Controller,
                false,
                false,
                true,
                true,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::ConfirmQuit)
        );
    }

    #[test]
    fn ctrl_q_quits_even_while_the_reset_dialog_is_open() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            map(
                quit,
                View::Controller,
                false,
                false,
                true,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::RequestQuit)
        );
    }

    #[test]
    fn ctrl_q_quits_even_while_the_redeploy_dialog_is_open() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            map(
                quit,
                View::Operation,
                false,
                false,
                false,
                false,
                true,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::RequestQuit)
        );
    }

    #[test]
    fn f6_requests_deploy_from_any_view() {
        for view in [
            View::Signals,
            View::Target,
            View::Controller,
            View::Operation,
            View::Help,
        ] {
            assert_eq!(
                map_in(key(KeyCode::F(6)), view),
                Some(Msg::RequestDeploy),
                "{view:?}"
            );
        }
    }

    #[test]
    fn space_toggles_pause_only_while_operation_is_open() {
        assert_eq!(
            map_in(key(KeyCode::Char(' ')), View::Operation),
            Some(Msg::TogglePauseOperation)
        );
        assert_eq!(map_in(key(KeyCode::Char(' ')), View::Signals), None);
    }

    #[test]
    fn space_still_types_an_ordinary_space_in_the_controller_editor() {
        // Space is a pacing control only in Operation; in Controller it must
        // keep being ordinary printable input, not silently swallowed.
        assert_eq!(
            map_in(key(KeyCode::Char(' ')), View::Controller),
            Some(Msg::EditController(EditOp::Insert(' ')))
        );
    }

    #[test]
    fn enter_steps_only_while_operation_is_open() {
        assert_eq!(
            map_in(key(KeyCode::Enter), View::Operation),
            Some(Msg::StepOperationTick)
        );
    }

    #[test]
    fn ctrl_enter_does_not_step_the_operation() {
        let ctrl_enter = key_with_modifiers(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(map_in(ctrl_enter, View::Operation), None);
    }

    #[test]
    fn f8_moves_focus_to_the_next_pane_in_operation_too() {
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Operation),
            Some(Msg::FocusNextPane)
        );
    }

    #[test]
    fn f8_moves_focus_to_the_next_pane_in_after_action_too() {
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::AfterAction),
            Some(Msg::FocusNextPane)
        );
    }

    #[test]
    fn review_run_chronology_keys_are_gated_on_the_telemetry_pane_being_focused() {
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
        ] {
            assert!(
                map(
                    key(code),
                    View::Operation,
                    false,
                    false,
                    false,
                    false,
                    false,
                    PaneId::OperationTelemetry,
                    true
                )
                .is_some(),
                "{code:?} should map while OperationTelemetry is focused"
            );
            assert_eq!(
                map(
                    key(code),
                    View::Operation,
                    false,
                    false,
                    false,
                    false,
                    false,
                    PaneId::Satellite,
                    true
                ),
                None,
                "{code:?} must not map while the Satellite pane is focused instead"
            );
        }
    }

    #[test]
    fn review_run_chronology_keys_map_to_the_expected_messages() {
        let map_telemetry = |code| {
            map(
                key(code),
                View::Operation,
                false,
                false,
                false,
                false,
                false,
                PaneId::OperationTelemetry,
                true,
            )
        };
        assert_eq!(
            map_telemetry(KeyCode::Up),
            Some(Msg::SelectPreviousReviewPoint)
        );
        assert_eq!(
            map_telemetry(KeyCode::Down),
            Some(Msg::SelectNextReviewPoint)
        );
        assert_eq!(
            map_telemetry(KeyCode::PageUp),
            Some(Msg::SelectReviewPointPageBackward(0))
        );
        assert_eq!(
            map_telemetry(KeyCode::PageDown),
            Some(Msg::SelectReviewPointPageForward(0))
        );
        assert_eq!(
            map_telemetry(KeyCode::Home),
            Some(Msg::SelectFirstReviewPoint)
        );
        assert_eq!(
            map_telemetry(KeyCode::End),
            Some(Msg::SelectLastReviewPoint)
        );
    }

    #[test]
    fn review_run_chronology_keys_do_not_leak_into_other_views() {
        // Signals keeps its own selection semantics even though it also
        // uses `PaneId::SignalsList`, distinct from
        // `PaneId::OperationTelemetry` — this guards against the gate ever
        // being loosened to match on the key alone and produce Review Run's
        // `Msg` variants for Signals instead of its own.
        assert_eq!(
            map_in(key(KeyCode::Up), View::Signals),
            Some(Msg::SelectPreviousSignal)
        );
        assert_eq!(
            map_in(key(KeyCode::PageUp), View::Signals),
            Some(Msg::SelectSignalPageBackward(0)),
            "Signals has its own PageUp binding, not Review Run's"
        );
        assert_eq!(
            map_in(key(KeyCode::Home), View::Signals),
            Some(Msg::SelectFirstSignal),
            "Signals has its own Home binding, not Review Run's"
        );
    }

    #[test]
    fn tab_toggles_run_inspector_mode_only_while_the_telemetry_pane_is_focused() {
        assert_eq!(
            map(
                key(KeyCode::Tab),
                View::Operation,
                false,
                false,
                false,
                false,
                false,
                PaneId::OperationTelemetry,
                true
            ),
            Some(Msg::ToggleRunInspectorMode)
        );
        assert_eq!(
            map(
                key(KeyCode::Tab),
                View::Operation,
                false,
                false,
                false,
                false,
                false,
                PaneId::Satellite,
                true
            ),
            None,
            "Tab must not map while the Satellite pane is focused instead"
        );
    }

    #[test]
    fn tab_still_indents_the_controller_source_and_is_unaffected_by_the_new_run_inspector_arm() {
        // The Run-Inspector `Tab` arm is gated on `View::Operation`, so it
        // must never shadow `Tab`'s existing indent binding in Controller
        // (`map_controller_edit`) — the two views can never both be current.
        assert_eq!(
            map(
                key(KeyCode::Tab),
                View::Controller,
                false,
                false,
                false,
                false,
                false,
                PaneId::ControllerSource,
                true
            ),
            Some(Msg::EditController(EditOp::Indent))
        );
    }

    #[test]
    fn after_action_scrolling_is_gated_on_the_report_pane_being_focused() {
        assert_eq!(
            map(
                key(KeyCode::Up),
                View::AfterAction,
                false,
                false,
                false,
                false,
                false,
                PaneId::Report,
                true
            ),
            Some(Msg::ScrollUp)
        );
        assert_eq!(
            map(
                key(KeyCode::Down),
                View::AfterAction,
                false,
                false,
                false,
                false,
                false,
                PaneId::Report,
                true
            ),
            Some(Msg::ScrollDown)
        );
        assert_eq!(
            map(
                key(KeyCode::Up),
                View::AfterAction,
                false,
                false,
                false,
                false,
                false,
                PaneId::FinalFrame,
                true
            ),
            None
        );
        assert_eq!(
            map(
                key(KeyCode::Down),
                View::AfterAction,
                false,
                false,
                false,
                false,
                false,
                PaneId::FinalFrame,
                true
            ),
            None
        );
    }

    #[test]
    fn help_supports_the_full_page_and_home_end_vocabulary() {
        assert_eq!(
            map_in(key(KeyCode::PageUp), View::Help),
            Some(Msg::ScrollPageBackward(0))
        );
        assert_eq!(
            map_in(key(KeyCode::PageDown), View::Help),
            Some(Msg::ScrollPageForward(0))
        );
        assert_eq!(
            map_in(key(KeyCode::Home), View::Help),
            Some(Msg::JumpScrollStart)
        );
        assert_eq!(
            map_in(key(KeyCode::End), View::Help),
            Some(Msg::JumpScrollEnd)
        );
    }

    #[test]
    fn after_action_report_supports_the_full_page_and_home_end_vocabulary() {
        assert_eq!(
            map_in(key(KeyCode::PageUp), View::AfterAction),
            Some(Msg::ScrollPageBackward(0))
        );
        assert_eq!(
            map_in(key(KeyCode::PageDown), View::AfterAction),
            Some(Msg::ScrollPageForward(0))
        );
        assert_eq!(
            map_in(key(KeyCode::Home), View::AfterAction),
            Some(Msg::JumpScrollStart)
        );
        assert_eq!(
            map_in(key(KeyCode::End), View::AfterAction),
            Some(Msg::JumpScrollEnd)
        );
    }

    #[test]
    fn after_action_page_and_home_end_keys_are_inert_while_the_final_frame_pane_is_focused() {
        for code in [
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
        ] {
            assert_eq!(
                map(
                    key(code),
                    View::AfterAction,
                    false,
                    false,
                    false,
                    false,
                    false,
                    PaneId::FinalFrame,
                    true
                ),
                None
            );
        }
    }
}
