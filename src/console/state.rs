//! Session/navigation state for the resistance console.
//!
//! This module is deliberately independent of any rendering or terminal
//! library so it can be tested without a real terminal and so later issues
//! can grow the state model without coupling it to widget code.

/// A major state the console can be showing.
///
/// Mirrors the `Signals -> Target -> Controller -> Operation -> After
/// Action` flow from `docs/TUI_DESIGN.md`, plus the contextual `Help`
/// overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Signals,
    Target,
    Controller,
    Operation,
    AfterAction,
    Help,
}

/// The opportunity the player has chosen to work.
///
/// No opportunity can be selected yet (that's #43), so this exists only as
/// the seam later issues will populate; it is always `None` in this issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(
    dead_code,
    reason = "constructed starting in #43 once selection exists"
)]
pub enum WorkingSet {
    FirstContact,
}

/// A player intent, decoupled from whatever key produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Navigate(View),
    OpenHelp,
    DismissHelp,
    Quit,
}

/// The console's full session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    current_view: View,
    help_return_view: Option<View>,
    working_set: Option<WorkingSet>,
    should_quit: bool,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            current_view: View::Signals,
            help_return_view: None,
            working_set: None,
            should_quit: false,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_view(&self) -> View {
        self.current_view
    }

    pub fn working_set(&self) -> Option<WorkingSet> {
        self.working_set
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Applies a single player intent, transitioning session state.
    pub fn apply(&mut self, msg: Msg) {
        match msg {
            // TODO(#43): gate Navigate on prerequisite state (a selected
            // signal/working set) once that state exists. Every view is
            // reachable for now because there is nothing yet to gate on.
            Msg::Navigate(view) => self.current_view = view,
            Msg::OpenHelp => {
                if self.current_view != View::Help {
                    self.help_return_view = Some(self.current_view);
                    self.current_view = View::Help;
                }
            }
            Msg::DismissHelp => {
                if let Some(view) = self.help_return_view.take() {
                    self.current_view = view;
                }
            }
            Msg::Quit => self.should_quit = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_on_signals_with_no_working_set() {
        let state = AppState::new();

        assert_eq!(state.current_view(), View::Signals);
        assert_eq!(state.working_set(), None);
        assert!(!state.should_quit());
    }

    #[test]
    fn navigate_switches_the_current_view() {
        for view in [
            View::Signals,
            View::Target,
            View::Controller,
            View::Operation,
            View::AfterAction,
        ] {
            let mut state = AppState::new();
            state.apply(Msg::Navigate(view));
            assert_eq!(state.current_view(), view);
        }
    }

    #[test]
    fn opening_help_remembers_the_prior_view() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::Controller));

        state.apply(Msg::OpenHelp);

        assert_eq!(state.current_view(), View::Help);
    }

    #[test]
    fn dismissing_help_restores_the_exact_prior_view() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::Operation));
        state.apply(Msg::OpenHelp);

        state.apply(Msg::DismissHelp);

        assert_eq!(state.current_view(), View::Operation);
    }

    #[test]
    fn dismissing_help_without_opening_it_is_a_no_op() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::Target));

        state.apply(Msg::DismissHelp);

        assert_eq!(state.current_view(), View::Target);
    }

    #[test]
    fn opening_help_twice_keeps_the_original_return_view() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::Target));

        state.apply(Msg::OpenHelp);
        state.apply(Msg::OpenHelp);
        state.apply(Msg::DismissHelp);

        assert_eq!(state.current_view(), View::Target);
    }

    #[test]
    fn quit_sets_the_quit_flag() {
        let mut state = AppState::new();

        state.apply(Msg::Quit);

        assert!(state.should_quit());
    }
}
