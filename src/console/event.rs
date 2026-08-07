//! Maps terminal key events onto the console's player intents.
//!
//! Kept separate from [`super::state`] so the transition logic stays free of
//! any terminal-library types, and separate from rendering so key bindings
//! can be unit tested by constructing `KeyEvent`s directly.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{Msg, View};

/// Maps a key event to a player intent, given whether Help is currently
/// showing (`F1` and `Esc` need to know whether they're opening or
/// dismissing it).
pub fn map(key: KeyEvent, help_is_open: bool) -> Option<Msg> {
    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Msg::Quit);
    }

    match key.code {
        KeyCode::F(1) => Some(if help_is_open {
            Msg::DismissHelp
        } else {
            Msg::OpenHelp
        }),
        KeyCode::Esc if help_is_open => Some(Msg::DismissHelp),
        KeyCode::F(2) => Some(Msg::Navigate(View::Signals)),
        KeyCode::F(3) => Some(Msg::Navigate(View::Target)),
        KeyCode::F(4) => Some(Msg::Navigate(View::Controller)),
        KeyCode::F(5) => Some(Msg::Navigate(View::Operation)),
        // F6 Deploy has no controller to deploy yet (see #44/#45), so it
        // stays inert rather than claiming to run anything; ui::draw_footer
        // renders it visibly dimmed to match.
        KeyCode::F(6) => None,
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

        assert_eq!(map(quit, false), Some(Msg::Quit));
        assert_eq!(map(quit, true), Some(Msg::Quit));
    }

    #[test]
    fn plain_q_does_not_quit() {
        assert_eq!(map(key(KeyCode::Char('q')), false), None);
    }

    #[test]
    fn f1_opens_help_when_closed_and_dismisses_when_open() {
        assert_eq!(map(key(KeyCode::F(1)), false), Some(Msg::OpenHelp));
        assert_eq!(map(key(KeyCode::F(1)), true), Some(Msg::DismissHelp));
    }

    #[test]
    fn esc_dismisses_help_only_when_open() {
        assert_eq!(map(key(KeyCode::Esc), true), Some(Msg::DismissHelp));
        assert_eq!(map(key(KeyCode::Esc), false), None);
    }

    #[test]
    fn function_keys_navigate_to_their_view() {
        assert_eq!(
            map(key(KeyCode::F(2)), false),
            Some(Msg::Navigate(View::Signals))
        );
        assert_eq!(
            map(key(KeyCode::F(3)), false),
            Some(Msg::Navigate(View::Target))
        );
        assert_eq!(
            map(key(KeyCode::F(4)), false),
            Some(Msg::Navigate(View::Controller))
        );
        assert_eq!(
            map(key(KeyCode::F(5)), false),
            Some(Msg::Navigate(View::Operation))
        );
    }

    #[test]
    fn f6_deploy_is_inert_until_a_controller_can_be_loaded() {
        assert_eq!(map(key(KeyCode::F(6)), false), None);
    }

    #[test]
    fn unbound_keys_map_to_nothing() {
        assert_eq!(map(key(KeyCode::Char('x')), false), None);
    }

    #[test]
    fn key_release_events_still_map_the_same_as_press() {
        let mut released = key(KeyCode::F(2));
        released.kind = KeyEventKind::Release;

        assert_eq!(map(released, false), Some(Msg::Navigate(View::Signals)));
    }
}
