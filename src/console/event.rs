//! Maps terminal key events onto the console's player intents.
//!
//! Kept separate from [`super::state`] so the transition logic stays free of
//! any terminal-library types, and separate from rendering so key bindings
//! can be unit tested by constructing `KeyEvent`s directly.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{Msg, View};

/// Maps a key event to a player intent, given the view currently showing
/// (several keys are context-sensitive: `F1`/`Esc` need to know whether
/// Help is open, `Esc`/arrows/`Enter` behave differently in Signals, Target,
/// and Help).
pub fn map(key: KeyEvent, current_view: View) -> Option<Msg> {
    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Msg::Quit);
    }

    let help_is_open = current_view == View::Help;

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
        // F6 Deploy has no controller to deploy yet (see #44/#45), so it
        // stays inert rather than claiming to run anything; ui::draw_footer
        // renders it visibly dimmed to match.
        KeyCode::F(6) => None,
        // F8's narrow-layout pane toggle only applies to Signals/Target
        // (`docs/TUI_DESIGN.md`, "Responsive behavior"); mapping it
        // elsewhere — e.g. while Help is open — would flip the hidden
        // toggle without a resize or navigation to ever reset it.
        KeyCode::F(8) if current_view == View::Signals || current_view == View::Target => {
            Some(Msg::ToggleSecondaryPane)
        }
        KeyCode::Enter if current_view == View::Signals || current_view == View::Target => {
            Some(Msg::Activate)
        }
        KeyCode::Up if current_view == View::Signals => Some(Msg::SelectPreviousSignal),
        KeyCode::Down if current_view == View::Signals => Some(Msg::SelectNextSignal),
        KeyCode::Up if help_is_open => Some(Msg::ScrollHelpUp),
        KeyCode::Down if help_is_open => Some(Msg::ScrollHelpDown),
        _ => None,
    }
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

    #[test]
    fn ctrl_q_quits_regardless_of_help_state() {
        let quit = key_with_modifiers(KeyCode::Char('q'), KeyModifiers::CONTROL);

        assert_eq!(map(quit, View::Signals), Some(Msg::Quit));
        assert_eq!(map(quit, View::Help), Some(Msg::Quit));
    }

    #[test]
    fn plain_q_does_not_quit() {
        assert_eq!(map(key(KeyCode::Char('q')), View::Signals), None);
    }

    #[test]
    fn f1_opens_help_when_closed_and_dismisses_when_open() {
        assert_eq!(map(key(KeyCode::F(1)), View::Signals), Some(Msg::OpenHelp));
        assert_eq!(map(key(KeyCode::F(1)), View::Help), Some(Msg::DismissHelp));
    }

    #[test]
    fn esc_dismisses_help_when_open() {
        assert_eq!(map(key(KeyCode::Esc), View::Help), Some(Msg::DismissHelp));
    }

    #[test]
    fn esc_returns_to_signals_from_target() {
        assert_eq!(
            map(key(KeyCode::Esc), View::Target),
            Some(Msg::Navigate(View::Signals))
        );
    }

    #[test]
    fn esc_does_nothing_elsewhere() {
        assert_eq!(map(key(KeyCode::Esc), View::Signals), None);
        assert_eq!(map(key(KeyCode::Esc), View::Controller), None);
    }

    #[test]
    fn function_keys_navigate_to_their_view() {
        assert_eq!(
            map(key(KeyCode::F(2)), View::Signals),
            Some(Msg::Navigate(View::Signals))
        );
        assert_eq!(
            map(key(KeyCode::F(3)), View::Signals),
            Some(Msg::Navigate(View::Target))
        );
        assert_eq!(
            map(key(KeyCode::F(4)), View::Signals),
            Some(Msg::Navigate(View::Controller))
        );
        assert_eq!(
            map(key(KeyCode::F(5)), View::Signals),
            Some(Msg::Navigate(View::Operation))
        );
    }

    #[test]
    fn f6_deploy_is_inert_until_a_controller_can_be_loaded() {
        assert_eq!(map(key(KeyCode::F(6)), View::Signals), None);
    }

    #[test]
    fn f8_toggles_the_secondary_pane() {
        assert_eq!(
            map(key(KeyCode::F(8)), View::Signals),
            Some(Msg::ToggleSecondaryPane)
        );
        assert_eq!(
            map(key(KeyCode::F(8)), View::Target),
            Some(Msg::ToggleSecondaryPane)
        );
    }

    #[test]
    fn f8_is_inert_outside_signals_and_target() {
        for view in [
            View::Controller,
            View::Operation,
            View::AfterAction,
            View::Help,
        ] {
            assert_eq!(map(key(KeyCode::F(8)), view), None);
        }
    }

    #[test]
    fn enter_activates_in_signals_and_target_only() {
        assert_eq!(map(key(KeyCode::Enter), View::Signals), Some(Msg::Activate));
        assert_eq!(map(key(KeyCode::Enter), View::Target), Some(Msg::Activate));
        assert_eq!(map(key(KeyCode::Enter), View::Controller), None);
        assert_eq!(map(key(KeyCode::Enter), View::Help), None);
    }

    #[test]
    fn arrows_move_signal_selection_only_in_signals() {
        assert_eq!(
            map(key(KeyCode::Up), View::Signals),
            Some(Msg::SelectPreviousSignal)
        );
        assert_eq!(
            map(key(KeyCode::Down), View::Signals),
            Some(Msg::SelectNextSignal)
        );
        assert_eq!(map(key(KeyCode::Up), View::Target), None);
    }

    #[test]
    fn arrows_scroll_help_when_help_is_open() {
        assert_eq!(map(key(KeyCode::Up), View::Help), Some(Msg::ScrollHelpUp));
        assert_eq!(
            map(key(KeyCode::Down), View::Help),
            Some(Msg::ScrollHelpDown)
        );
    }

    #[test]
    fn unbound_keys_map_to_nothing() {
        assert_eq!(map(key(KeyCode::Char('x')), View::Signals), None);
    }

    #[test]
    fn key_release_events_still_map_the_same_as_press() {
        let mut released = key(KeyCode::F(2));
        released.kind = KeyEventKind::Release;

        assert_eq!(
            map(released, View::Signals),
            Some(Msg::Navigate(View::Signals))
        );
    }
}
