//! Maps terminal key events onto the console's player intents.
//!
//! Kept separate from [`super::state`] so the transition logic stays free of
//! any terminal-library types, and separate from rendering so key bindings
//! can be unit tested by constructing `KeyEvent`s directly.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::editor::EditOp;
use super::state::{Msg, View};

/// Maps a key event to a player intent, given the view currently showing
/// (several keys are context-sensitive: `F1`/`Esc` need to know whether
/// Help is open, `Esc`/arrows/`Enter` behave differently in Signals, Target,
/// and Help) and whether either confirmation dialog is currently open.
pub fn map(
    key: KeyEvent,
    current_view: View,
    reset_confirmation_pending: bool,
    quit_confirmation_pending: bool,
) -> Option<Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if key.code == KeyCode::Char('q') && ctrl {
        return Some(Msg::RequestQuit);
    }

    // A confirmation dialog swallows every key except its own yes/no so the
    // player can't accidentally act past it (e.g. keep typing Lua while a
    // reset prompt is showing).
    if quit_confirmation_pending {
        return match key.code {
            KeyCode::Enter | KeyCode::Char('y') => Some(Msg::ConfirmQuit),
            KeyCode::Esc | KeyCode::Char('n') => Some(Msg::CancelQuit),
            _ => None,
        };
    }
    if reset_confirmation_pending {
        return match key.code {
            KeyCode::Enter | KeyCode::Char('y') => Some(Msg::ConfirmResetController),
            KeyCode::Esc | KeyCode::Char('n') => Some(Msg::CancelResetController),
            _ => None,
        };
    }

    let help_is_open = current_view == View::Help;
    let controller_is_open = current_view == View::Controller;

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
        // F6 Deploy has no operation to deploy yet (see #45), so it stays
        // inert rather than claiming to run anything; ui::draw_footer
        // renders it visibly dimmed to match.
        KeyCode::F(6) => None,
        KeyCode::F(7) if controller_is_open => Some(Msg::RequestResetController),
        // F8's narrow-layout pane toggle applies to Signals/Target/Controller
        // (`docs/TUI_DESIGN.md`, "Responsive behavior"); mapping it
        // elsewhere — e.g. while Help is open — would flip the hidden
        // toggle without a resize or navigation to ever reset it.
        KeyCode::F(8)
            if current_view == View::Signals
                || current_view == View::Target
                || controller_is_open =>
        {
            Some(Msg::ToggleSecondaryPane)
        }
        KeyCode::Enter if ctrl && controller_is_open => Some(Msg::ValidateController),
        KeyCode::Enter
            if !ctrl && (current_view == View::Signals || current_view == View::Target) =>
        {
            Some(Msg::Activate)
        }
        KeyCode::Up if current_view == View::Signals => Some(Msg::SelectPreviousSignal),
        KeyCode::Down if current_view == View::Signals => Some(Msg::SelectNextSignal),
        KeyCode::Up if help_is_open => Some(Msg::ScrollHelpUp),
        KeyCode::Down if help_is_open => Some(Msg::ScrollHelpDown),
        _ if controller_is_open => map_controller_edit(key.code, ctrl),
        _ => None,
    }
}

/// Ordinary editing/cursor-movement keys, only reachable once Controller is
/// showing and neither confirmation dialog is open (see [`map`]).
fn map_controller_edit(code: KeyCode, ctrl: bool) -> Option<Msg> {
    let op = match code {
        KeyCode::Char(c) if !ctrl => EditOp::Insert(c),
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

    /// Shorthand for the common case: neither confirmation dialog is open.
    fn map_in(key: KeyEvent, view: View) -> Option<Msg> {
        map(key, view, false, false)
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
    fn f6_deploy_is_inert_until_an_operation_can_be_run() {
        assert_eq!(map_in(key(KeyCode::F(6)), View::Signals), None);
        assert_eq!(map_in(key(KeyCode::F(6)), View::Controller), None);
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
    fn f8_is_inert_outside_signals_target_and_controller() {
        for view in [View::Operation, View::AfterAction, View::Help] {
            assert_eq!(map_in(key(KeyCode::F(8)), view), None);
        }
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
        assert_eq!(
            map_in(key(KeyCode::Up), View::Help),
            Some(Msg::ScrollHelpUp)
        );
        assert_eq!(
            map_in(key(KeyCode::Down), View::Help),
            Some(Msg::ScrollHelpDown)
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
            map(key(KeyCode::Enter), View::Controller, false, true),
            Some(Msg::ConfirmQuit)
        );
        assert_eq!(
            map(key(KeyCode::Char('y')), View::Controller, false, true),
            Some(Msg::ConfirmQuit)
        );
        assert_eq!(
            map(key(KeyCode::Esc), View::Controller, false, true),
            Some(Msg::CancelQuit)
        );
        assert_eq!(
            map(key(KeyCode::Char('n')), View::Controller, false, true),
            Some(Msg::CancelQuit)
        );
        assert_eq!(
            map(key(KeyCode::Char('x')), View::Controller, false, true),
            None,
            "ordinary keys must not leak through while the quit dialog is open"
        );
    }

    #[test]
    fn reset_confirmation_pending_only_accepts_confirm_or_cancel() {
        assert_eq!(
            map(key(KeyCode::Enter), View::Controller, true, false),
            Some(Msg::ConfirmResetController)
        );
        assert_eq!(
            map(key(KeyCode::Esc), View::Controller, true, false),
            Some(Msg::CancelResetController)
        );
        assert_eq!(
            map(key(KeyCode::Char('x')), View::Controller, true, false),
            None,
            "ordinary keys must not leak through while the reset dialog is open"
        );
    }

    #[test]
    fn quit_confirmation_takes_priority_over_reset_confirmation() {
        assert_eq!(
            map(key(KeyCode::Enter), View::Controller, true, true),
            Some(Msg::ConfirmQuit)
        );
    }

    #[test]
    fn ctrl_q_quits_even_while_the_reset_dialog_is_open() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            map(quit, View::Controller, true, false),
            Some(Msg::RequestQuit)
        );
    }
}
