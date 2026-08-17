//! Maps terminal key events onto the console's player intents.
//!
//! Kept separate from [`super::state`] so the transition logic stays free of
//! any terminal-library types, and separate from rendering so key bindings
//! can be unit tested by constructing `KeyEvent`s directly.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::editor::EditOp;
use super::state::{Msg, View};

/// Views whose content pane can scroll via `Msg::ScrollUp`/`Msg::ScrollDown`.
fn view_is_scrollable(view: View) -> bool {
    matches!(view, View::Help | View::AfterAction)
}

/// Maps a key event to a player intent, given the view currently showing
/// (several keys are context-sensitive: `F1`/`Esc` need to know whether
/// Help is open, `Esc`/arrows/`Enter` behave differently in Signals, Target,
/// and Help), whether any confirmation dialog is currently open, and
/// whether Controller's source pane (rather than the Lua reference pane,
/// which `F8` can swap in at 80-99 columns) is what's actually on screen.
pub fn map(
    key: KeyEvent,
    current_view: View,
    reset_confirmation_pending: bool,
    quit_confirmation_pending: bool,
    redeploy_confirmation_pending: bool,
    controller_source_visible: bool,
) -> Option<Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if key.code == KeyCode::Char('q') && ctrl {
        return Some(Msg::RequestQuit);
    }

    // A confirmation dialog swallows every key except its own unmodified
    // yes/no so the player can't accidentally act past it (e.g. keep
    // typing Lua while a reset prompt is showing) or trigger it by
    // accident via some other binding's modified form — `Ctrl+Enter`
    // (validate) and `Ctrl+Y` both have unmodified forms that must not
    // silently confirm a destructive action instead.
    let unmodified = key.modifiers.is_empty();
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
    let after_action_is_open = current_view == View::AfterAction;

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
        // F8's narrow-layout pane toggle applies to Signals/Target/
        // Controller/Operation/AfterAction (`docs/TUI_DESIGN.md`,
        // "Responsive behavior"); mapping it elsewhere — e.g. while Help is
        // open — would flip the hidden toggle without a resize or
        // navigation to ever reset it.
        KeyCode::F(8)
            if current_view == View::Signals
                || current_view == View::Target
                || controller_is_open
                || operation_is_open
                || after_action_is_open =>
        {
            Some(Msg::ToggleSecondaryPane)
        }
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
            if !ctrl && (current_view == View::Signals || current_view == View::Target) =>
        {
            Some(Msg::Activate)
        }
        // `Enter`/`Space` while Operation is showing: pacing controls, not
        // navigation (`docs/TUI_DESIGN.md`, "Pacing controls").
        // `Msg::StepOperationTick` is itself a no-op unless the run is
        // paused, so `Enter` here doesn't fast-forward a running operation.
        KeyCode::Enter if !ctrl && operation_is_open => Some(Msg::StepOperationTick),
        KeyCode::Char(' ') if !ctrl && operation_is_open => Some(Msg::TogglePauseOperation),
        KeyCode::Up if current_view == View::Signals => Some(Msg::SelectPreviousSignal),
        KeyCode::Down if current_view == View::Signals => Some(Msg::SelectNextSignal),
        KeyCode::Up if view_is_scrollable(current_view) => Some(Msg::ScrollUp),
        KeyCode::Down if view_is_scrollable(current_view) => Some(Msg::ScrollDown),
        // While the reference pane is swapped in at 80-99 columns, the
        // source isn't on screen at all, so ordinary editing keys must not
        // silently mutate it — only F8 (handled above) can bring it back.
        _ if controller_is_open && controller_source_visible => {
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
            map_controller_edit(key.code, ctrl, altgr)
        }
        _ => None,
    }
}

/// Ordinary editing/cursor-movement keys, only reachable once Controller is
/// showing and neither confirmation dialog is open (see [`map`]). `altgr`
/// is set when `ctrl` is only true because of an AltGr chord (`CONTROL |
/// ALT` together), which must still type its printable character.
fn map_controller_edit(code: KeyCode, ctrl: bool, altgr: bool) -> Option<Msg> {
    let op = match code {
        KeyCode::Char(c) if !ctrl || altgr => EditOp::Insert(c),
        // A single space keeps indentation as ordinary printable characters
        // (no literal tab byte in the source) without pretending to be a
        // real indent-width-aware editor.
        KeyCode::Tab => EditOp::Insert(' '),
        KeyCode::Enter => EditOp::Newline,
        KeyCode::Backspace => EditOp::Backspace,
        KeyCode::Delete => EditOp::DeleteForward,
        KeyCode::Left => EditOp::MoveLeft,
        KeyCode::Right => EditOp::MoveRight,
        KeyCode::Up => EditOp::MoveUp,
        KeyCode::Down => EditOp::MoveDown,
        KeyCode::Home => EditOp::MoveLineStart,
        KeyCode::End => EditOp::MoveLineEnd,
        KeyCode::PageUp => EditOp::PageUp,
        KeyCode::PageDown => EditOp::PageDown,
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

    /// Shorthand for the common case: no confirmation dialog is open and
    /// Controller's source pane (if relevant) is visible.
    fn map_in(key: KeyEvent, view: View) -> Option<Msg> {
        map(key, view, false, false, false, true)
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
    fn editing_keys_are_inert_while_the_reference_pane_is_shown_instead_of_source() {
        let source_hidden = false;
        assert_eq!(
            map(
                key(KeyCode::Char('x')),
                View::Controller,
                false,
                false,
                false,
                source_hidden
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
                source_hidden
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
                source_hidden
            ),
            None
        );
    }

    #[test]
    fn f7_and_validate_still_work_while_the_reference_pane_is_shown() {
        let source_hidden = false;
        assert_eq!(
            map(
                key(KeyCode::F(7)),
                View::Controller,
                false,
                false,
                false,
                source_hidden
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
                source_hidden
            ),
            Some(Msg::ValidateController)
        );
    }

    #[test]
    fn f8_toggles_the_secondary_pane() {
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Signals),
            Some(Msg::ToggleSecondaryPane)
        );
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Target),
            Some(Msg::ToggleSecondaryPane)
        );
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Controller),
            Some(Msg::ToggleSecondaryPane)
        );
    }

    #[test]
    fn f8_is_inert_outside_signals_target_controller_operation_and_after_action() {
        assert_eq!(map_in(key(KeyCode::F(8)), View::Help), None);
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
            (KeyCode::Left, EditOp::MoveLeft),
            (KeyCode::Right, EditOp::MoveRight),
            (KeyCode::Up, EditOp::MoveUp),
            (KeyCode::Down, EditOp::MoveDown),
            (KeyCode::Home, EditOp::MoveLineStart),
            (KeyCode::End, EditOp::MoveLineEnd),
            (KeyCode::PageUp, EditOp::PageUp),
            (KeyCode::PageDown, EditOp::PageDown),
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
    fn arrows_scroll_help_when_help_is_open() {
        assert_eq!(map_in(key(KeyCode::Up), View::Help), Some(Msg::ScrollUp));
        assert_eq!(
            map_in(key(KeyCode::Down), View::Help),
            Some(Msg::ScrollDown)
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
                true,
                false,
                true
            ),
            Some(Msg::ConfirmQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('y')),
                View::Controller,
                false,
                true,
                false,
                true
            ),
            Some(Msg::ConfirmQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Esc),
                View::Controller,
                false,
                true,
                false,
                true
            ),
            Some(Msg::CancelQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('n')),
                View::Controller,
                false,
                true,
                false,
                true
            ),
            Some(Msg::CancelQuit)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('x')),
                View::Controller,
                false,
                true,
                false,
                true
            ),
            None,
            "ordinary keys must not leak through while the quit dialog is open"
        );
    }

    #[test]
    fn reset_confirmation_pending_only_accepts_confirm_or_cancel() {
        assert_eq!(
            map(
                key(KeyCode::Enter),
                View::Controller,
                true,
                false,
                false,
                true
            ),
            Some(Msg::ConfirmResetController)
        );
        assert_eq!(
            map(
                key(KeyCode::Esc),
                View::Controller,
                true,
                false,
                false,
                true
            ),
            Some(Msg::CancelResetController)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('x')),
                View::Controller,
                true,
                false,
                false,
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
                true,
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
                true,
                true
            ),
            Some(Msg::ConfirmDeploy)
        );
        assert_eq!(
            map(key(KeyCode::Esc), View::Operation, false, false, true, true),
            Some(Msg::CancelDeploy)
        );
        assert_eq!(
            map(
                key(KeyCode::Char('n')),
                View::Operation,
                false,
                false,
                true,
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
                true,
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
            map(ctrl_enter, View::Controller, false, true, false, true),
            None
        );
        assert_eq!(
            map(ctrl_y, View::Controller, false, true, false, true),
            None
        );
        assert_eq!(
            map(ctrl_esc, View::Controller, false, true, false, true),
            None
        );
        assert_eq!(
            map(ctrl_n, View::Controller, false, true, false, true),
            None
        );

        // Reset dialog: same requirement.
        assert_eq!(
            map(ctrl_enter, View::Controller, true, false, false, true),
            None
        );
        assert_eq!(
            map(ctrl_y, View::Controller, true, false, false, true),
            None
        );
        assert_eq!(
            map(ctrl_esc, View::Controller, true, false, false, true),
            None
        );
        assert_eq!(
            map(ctrl_n, View::Controller, true, false, false, true),
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
                map(enter, View::Controller, false, true, false, true),
                None,
                "{modifiers:?}+Enter must not confirm quit"
            );
            assert_eq!(
                map(y, View::Controller, true, false, false, true),
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
                true,
                true,
                false,
                true
            ),
            Some(Msg::ConfirmQuit)
        );
    }

    #[test]
    fn ctrl_q_quits_even_while_the_reset_dialog_is_open() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            map(quit, View::Controller, true, false, false, true),
            Some(Msg::RequestQuit)
        );
    }

    #[test]
    fn ctrl_q_quits_even_while_the_redeploy_dialog_is_open() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            map(quit, View::Operation, false, false, true, true),
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
    fn f8_toggles_the_secondary_pane_in_operation_too() {
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::Operation),
            Some(Msg::ToggleSecondaryPane)
        );
    }

    #[test]
    fn f8_toggles_the_secondary_pane_in_after_action_too() {
        assert_eq!(
            map_in(key(KeyCode::F(8)), View::AfterAction),
            Some(Msg::ToggleSecondaryPane)
        );
    }
}
