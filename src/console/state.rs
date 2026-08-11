//! Session/navigation state for the resistance console.
//!
//! This module is deliberately independent of any rendering or terminal
//! library so it can be tested without a real terminal and so later issues
//! can grow the state model without coupling it to widget code.

use super::editor::{EditOp, Editor};
use super::intel::authored_signals;
use crate::lua_controller;

/// An upper bound on how far Help can scroll. This exists only so
/// `ScrollHelpDown` can never run away toward `u16::MAX` and leave the
/// player pressing `Up` an impractical number of times to recover — the
/// real, content- and frame-size-aware bound is `ui::help_max_scroll`,
/// which `console::should_redraw` re-clamps `help_scroll` against after
/// every scroll key, so this only needs to be comfortably above Help's
/// actual line count (not tuned to match it), not an accurate ceiling in
/// its own right. A ceiling tuned too close to that count silently caps
/// scrolling below the real content height the next time Help grows.
const MAX_HELP_SCROLL: u16 = 500;

/// The result of the most recent `Msg::ValidateController`, or the fact
/// that the current source hasn't been checked (or was edited since the
/// last check).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validation {
    Unchecked,
    Valid,
    Invalid(String),
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkingSet {
    FirstContact,
}

/// A player intent, decoupled from whatever key produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Navigate(View),
    OpenHelp,
    DismissHelp,
    SelectPreviousSignal,
    SelectNextSignal,
    /// Context-sensitive "Enter": inspects the selected signal in Signals,
    /// or commits to working the current dossier's opportunity in Target.
    Activate,
    ToggleSecondaryPane,
    ScrollHelpUp,
    ScrollHelpDown,
    /// An editing/cursor-movement key applied to the current controller
    /// source; a no-op if no controller is loaded.
    EditController(EditOp),
    /// Checks whether the current controller source is loadable Lua that
    /// defines `on_tick`, without running anything.
    ValidateController,
    /// `F7`: restores the starter controller, subject to confirmation if
    /// doing so would discard edits.
    RequestResetController,
    ConfirmResetController,
    CancelResetController,
    /// `Ctrl+Q`: quits, subject to confirmation if the controller is
    /// modified.
    RequestQuit,
    ConfirmQuit,
    CancelQuit,
}

/// The console's full session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    current_view: View,
    help_return_view: Option<View>,
    working_set: Option<WorkingSet>,
    /// Index into [`authored_signals`], moved by `SelectPreviousSignal` /
    /// `SelectNextSignal`.
    selected_signal: usize,
    /// True once the player has inspected the actionable signal, making
    /// Target reachable even before a working set is committed.
    target_known: bool,
    /// The player's current Lua source and cursor for the working set,
    /// seeded from the starter controller the first time an opportunity is
    /// committed to.
    controller: Option<Editor>,
    /// The result of the most recent validation, reset to `Unchecked` by
    /// every edit or reset.
    validation: Validation,
    /// `F7` was pressed with a modified controller; the player must confirm
    /// before the starter controller replaces it.
    reset_confirmation_pending: bool,
    /// `Ctrl+Q` was pressed with a modified controller; the player must
    /// confirm before the session exits.
    quit_confirmation_pending: bool,
    /// At 80-99 columns, whether the secondary (detail/provenance) pane is
    /// showing instead of the primary one.
    narrow_secondary_visible: bool,
    help_scroll: u16,
    should_quit: bool,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            current_view: View::Signals,
            help_return_view: None,
            working_set: None,
            selected_signal: 0,
            target_known: false,
            controller: None,
            validation: Validation::Unchecked,
            reset_confirmation_pending: false,
            quit_confirmation_pending: false,
            narrow_secondary_visible: false,
            help_scroll: 0,
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

    /// The view Help was opened from, if Help is currently showing.
    pub fn help_return_view(&self) -> Option<View> {
        self.help_return_view
    }

    pub fn working_set(&self) -> Option<WorkingSet> {
        self.working_set
    }

    pub fn selected_signal(&self) -> usize {
        self.selected_signal
    }

    pub fn target_known(&self) -> bool {
        self.target_known
    }

    pub fn controller_source(&self) -> Option<&str> {
        self.controller.as_ref().map(Editor::source)
    }

    /// 0-based `(line, column)` of the cursor in the current controller
    /// source, if a controller is loaded.
    pub fn controller_cursor(&self) -> Option<(usize, usize)> {
        self.controller.as_ref().map(Editor::cursor_line_col)
    }

    /// Whether the current controller source differs from the starter
    /// controller. `false` when no controller is loaded.
    pub fn controller_modified(&self) -> bool {
        self.controller_source()
            .is_some_and(|source| source != super::intel::STARTER_CONTROLLER)
    }

    pub fn validation(&self) -> &Validation {
        &self.validation
    }

    pub fn reset_confirmation_pending(&self) -> bool {
        self.reset_confirmation_pending
    }

    pub fn quit_confirmation_pending(&self) -> bool {
        self.quit_confirmation_pending
    }

    pub fn narrow_secondary_visible(&self) -> bool {
        self.narrow_secondary_visible
    }

    pub fn help_scroll(&self) -> u16 {
        self.help_scroll
    }

    /// Bounds the stored scroll offset itself against `max`, not just the
    /// value used for a single render. Without this, repeated `Down`
    /// presses can advance `help_scroll` toward `MAX_HELP_SCROLL` even once
    /// the content is fully visible, and `Up` then appears to do nothing
    /// until the stored offset drops back below the real maximum.
    pub fn clamp_help_scroll(&mut self, max: u16) {
        self.help_scroll = self.help_scroll.min(max);
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Resets narrow-layout state that only makes sense for the geometry it
    /// was toggled under: a `ToggleSecondaryPane` while wide has no visible
    /// effect, but without this it would still silently flip the flag, so a
    /// later resize into narrow layout could show the secondary pane
    /// without the player ever having pressed `F8` in that layout.
    pub fn handle_resize(&mut self) {
        self.narrow_secondary_visible = false;
    }

    /// Whether `view` is currently reachable via `Navigate`, given what the
    /// player has inspected or committed to so far.
    pub fn view_available(&self, view: View) -> bool {
        match view {
            View::Signals | View::AfterAction | View::Help => true,
            View::Target => self.target_known || self.working_set.is_some(),
            View::Controller | View::Operation => self.working_set.is_some(),
        }
    }

    /// Applies a single player intent, transitioning session state.
    pub fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Navigate(view) => {
                if self.view_available(view) {
                    self.current_view = view;
                    self.narrow_secondary_visible = false;
                }
            }
            Msg::OpenHelp => {
                if self.current_view != View::Help {
                    self.help_return_view = Some(self.current_view);
                    self.current_view = View::Help;
                    self.help_scroll = 0;
                }
            }
            Msg::DismissHelp => {
                if let Some(view) = self.help_return_view.take() {
                    self.current_view = view;
                }
            }
            Msg::SelectPreviousSignal => {
                self.selected_signal = self.selected_signal.saturating_sub(1);
            }
            Msg::SelectNextSignal => {
                let last = authored_signals().len().saturating_sub(1);
                self.selected_signal = (self.selected_signal + 1).min(last);
            }
            Msg::Activate => self.activate(),
            Msg::ToggleSecondaryPane => {
                self.narrow_secondary_visible = !self.narrow_secondary_visible;
            }
            Msg::ScrollHelpUp => self.help_scroll = self.help_scroll.saturating_sub(1),
            Msg::ScrollHelpDown => {
                self.help_scroll = self.help_scroll.saturating_add(1).min(MAX_HELP_SCROLL);
            }
            Msg::EditController(op) => {
                if let Some(controller) = self.controller.as_mut()
                    && controller.apply(op)
                {
                    self.validation = Validation::Unchecked;
                }
            }
            Msg::ValidateController => {
                if let Some(source) = self.controller_source() {
                    self.validation = match lua_controller::validate(source) {
                        Ok(()) => Validation::Valid,
                        Err(err) => Validation::Invalid(err.to_string()),
                    };
                }
            }
            Msg::RequestResetController => {
                if self.controller_modified() {
                    self.reset_confirmation_pending = true;
                } else {
                    self.reset_controller();
                }
            }
            Msg::ConfirmResetController => {
                self.reset_controller();
                self.reset_confirmation_pending = false;
            }
            Msg::CancelResetController => {
                self.reset_confirmation_pending = false;
            }
            Msg::RequestQuit => {
                if self.controller_modified() {
                    self.quit_confirmation_pending = true;
                } else {
                    self.should_quit = true;
                }
            }
            Msg::ConfirmQuit => {
                self.should_quit = true;
            }
            Msg::CancelQuit => {
                self.quit_confirmation_pending = false;
            }
        }
    }

    fn reset_controller(&mut self) {
        if let Some(controller) = self.controller.as_mut() {
            controller.reset(super::intel::STARTER_CONTROLLER);
            self.validation = Validation::Unchecked;
        }
    }

    fn activate(&mut self) {
        match self.current_view {
            View::Signals => {
                let selected = authored_signals().get(self.selected_signal);
                if selected.is_some_and(|signal| signal.is_actionable()) {
                    self.target_known = true;
                    self.current_view = View::Target;
                    self.narrow_secondary_visible = false;
                }
            }
            View::Target => {
                if self.working_set != Some(WorkingSet::FirstContact) {
                    self.working_set = Some(WorkingSet::FirstContact);
                    self.controller = Some(Editor::new(super::intel::STARTER_CONTROLLER));
                }
                self.current_view = View::Controller;
                self.narrow_secondary_visible = false;
            }
            _ => {}
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
    fn navigate_switches_the_current_view_for_always_available_views() {
        for view in [View::Signals, View::AfterAction] {
            let mut state = AppState::new();
            state.apply(Msg::Navigate(view));
            assert_eq!(state.current_view(), view);
        }
    }

    #[test]
    fn target_controller_and_operation_are_unreachable_before_their_prerequisites() {
        for view in [View::Target, View::Controller, View::Operation] {
            let mut state = AppState::new();
            state.apply(Msg::Navigate(view));
            assert_eq!(
                state.current_view(),
                View::Signals,
                "{view:?} should not be reachable from a fresh session"
            );
        }
    }

    #[test]
    fn target_becomes_reachable_once_the_actionable_signal_is_inspected() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);

        state.apply(Msg::Navigate(View::Signals));
        state.apply(Msg::Navigate(View::Target));

        assert_eq!(state.current_view(), View::Target);
    }

    #[test]
    fn controller_and_operation_become_reachable_once_a_working_set_exists() {
        let mut state = AppState::new();
        state.apply(Msg::Activate); // inspect First Contact
        state.apply(Msg::Activate); // commit to working it

        for view in [View::Controller, View::Operation] {
            state.apply(Msg::Navigate(view));
            assert_eq!(state.current_view(), view);
        }
    }

    #[test]
    fn opening_help_remembers_the_prior_view() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::AfterAction));

        state.apply(Msg::OpenHelp);

        assert_eq!(state.current_view(), View::Help);
    }

    #[test]
    fn dismissing_help_restores_the_exact_prior_view() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::AfterAction));
        state.apply(Msg::OpenHelp);

        state.apply(Msg::DismissHelp);

        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn dismissing_help_without_opening_it_is_a_no_op() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::AfterAction));

        state.apply(Msg::DismissHelp);

        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn opening_help_twice_keeps_the_original_return_view() {
        let mut state = AppState::new();
        state.apply(Msg::Navigate(View::AfterAction));

        state.apply(Msg::OpenHelp);
        state.apply(Msg::OpenHelp);
        state.apply(Msg::DismissHelp);

        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn signal_selection_does_not_move_past_the_ends_of_the_list() {
        let mut state = AppState::new();

        state.apply(Msg::SelectPreviousSignal);
        assert_eq!(state.selected_signal(), 0);

        for _ in 0..authored_signals().len() + 2 {
            state.apply(Msg::SelectNextSignal);
        }
        assert_eq!(state.selected_signal(), authored_signals().len() - 1);
    }

    #[test]
    fn activating_the_actionable_signal_marks_target_known_and_opens_it() {
        let mut state = AppState::new();
        let actionable = authored_signals()
            .iter()
            .position(|signal| signal.is_actionable())
            .expect("exactly one signal is actionable");
        for _ in 0..actionable {
            state.apply(Msg::SelectNextSignal);
        }

        state.apply(Msg::Activate);

        assert!(state.target_known());
        assert_eq!(state.current_view(), View::Target);
    }

    #[test]
    fn activating_a_non_actionable_signal_is_a_no_op() {
        let mut state = AppState::new();
        let non_actionable = authored_signals()
            .iter()
            .position(|signal| !signal.is_actionable())
            .expect("at least one signal is non-actionable");
        for _ in 0..non_actionable {
            state.apply(Msg::SelectNextSignal);
        }

        state.apply(Msg::Activate);

        assert!(!state.target_known());
        assert_eq!(state.current_view(), View::Signals);
    }

    #[test]
    fn activating_target_commits_the_working_set_and_seeds_the_starter_controller() {
        let mut state = AppState::new();
        state.apply(Msg::Activate); // inspect
        state.apply(Msg::Activate); // commit

        assert_eq!(state.working_set(), Some(WorkingSet::FirstContact));
        assert_eq!(
            state.controller_source(),
            Some(super::super::intel::STARTER_CONTROLLER)
        );
        assert_eq!(state.current_view(), View::Controller);
    }

    #[test]
    fn reactivating_target_preserves_edited_controller_source() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('!')));

        state.apply(Msg::Navigate(View::Target));
        state.apply(Msg::Activate);

        assert_eq!(
            state.controller_source(),
            Some(format!("{}!", super::super::intel::STARTER_CONTROLLER).as_str())
        );
    }

    #[test]
    fn editing_the_controller_marks_it_modified_and_resets_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        assert!(!state.controller_modified());

        state.apply(Msg::EditController(EditOp::Insert('x')));

        assert!(state.controller_modified());
        assert_eq!(state.validation(), &Validation::Unchecked);
    }

    #[test]
    fn moving_the_cursor_after_validating_preserves_the_result() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);

        state.apply(Msg::EditController(EditOp::MoveLeft));
        state.apply(Msg::EditController(EditOp::MoveDown));

        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "cursor movement doesn't change the source, so a prior validation is still accurate"
        );
    }

    #[test]
    fn a_no_op_boundary_delete_preserves_a_prior_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate); // cursor starts at the end of the source
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);

        // DeleteForward at the document's end and Backspace at its start
        // are both no-ops (Editor::apply reports no mutation), so neither
        // should invalidate a result that's still accurate.
        state.apply(Msg::EditController(EditOp::DeleteForward));
        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "DeleteForward at the end of the document didn't change anything"
        );

        state.apply(Msg::EditController(EditOp::PageUp));
        state.apply(Msg::EditController(EditOp::PageUp));
        state.apply(Msg::EditController(EditOp::MoveLineStart));
        state.apply(Msg::EditController(EditOp::Backspace));
        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "Backspace at the start of the document didn't change anything either"
        );
    }

    #[test]
    fn editing_the_controller_before_a_working_set_exists_is_a_no_op() {
        let mut state = AppState::new();
        state.apply(Msg::EditController(EditOp::Insert('x')));
        assert_eq!(state.controller_source(), None);
    }

    #[test]
    fn validating_a_valid_controller_reports_it_valid() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);

        state.apply(Msg::ValidateController);

        assert_eq!(state.validation(), &Validation::Valid);
    }

    #[test]
    fn validating_an_invalid_controller_reports_the_diagnostic() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));

        state.apply(Msg::ValidateController);

        assert!(matches!(state.validation(), Validation::Invalid(_)));
    }

    #[test]
    fn resetting_an_unmodified_controller_needs_no_confirmation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);

        state.apply(Msg::RequestResetController);

        assert!(!state.reset_confirmation_pending());
        assert_eq!(
            state.controller_source(),
            Some(super::super::intel::STARTER_CONTROLLER)
        );
    }

    #[test]
    fn resetting_a_modified_controller_requires_confirmation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));

        state.apply(Msg::RequestResetController);

        assert!(state.reset_confirmation_pending());
        assert!(state.controller_modified(), "the edit is not discarded yet");
    }

    #[test]
    fn confirming_reset_restores_the_starter_controller() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::RequestResetController);

        state.apply(Msg::ConfirmResetController);

        assert!(!state.reset_confirmation_pending());
        assert_eq!(
            state.controller_source(),
            Some(super::super::intel::STARTER_CONTROLLER)
        );
    }

    #[test]
    fn cancelling_reset_keeps_the_edit_and_clears_the_prompt() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::RequestResetController);

        state.apply(Msg::CancelResetController);

        assert!(!state.reset_confirmation_pending());
        assert!(state.controller_modified());
    }

    #[test]
    fn toggling_the_secondary_pane_flips_visibility_and_navigation_resets_it() {
        let mut state = AppState::new();
        state.apply(Msg::ToggleSecondaryPane);
        assert!(state.narrow_secondary_visible());

        state.apply(Msg::Navigate(View::Signals));
        assert!(!state.narrow_secondary_visible());
    }

    #[test]
    fn help_scroll_moves_up_and_down_and_saturates_at_zero() {
        let mut state = AppState::new();
        state.apply(Msg::ScrollHelpDown);
        state.apply(Msg::ScrollHelpDown);
        assert_eq!(state.help_scroll(), 2);

        state.apply(Msg::ScrollHelpUp);
        state.apply(Msg::ScrollHelpUp);
        state.apply(Msg::ScrollHelpUp);
        assert_eq!(state.help_scroll(), 0);
    }

    #[test]
    fn help_scroll_is_capped_and_recoverable() {
        let mut state = AppState::new();
        for _ in 0..1000 {
            state.apply(Msg::ScrollHelpDown);
        }
        let capped = state.help_scroll();
        assert!(capped < 1000);

        for _ in 0..capped {
            state.apply(Msg::ScrollHelpUp);
        }
        assert_eq!(state.help_scroll(), 0);
    }

    #[test]
    fn resizing_clears_the_narrow_secondary_pane_flag() {
        let mut state = AppState::new();
        state.apply(Msg::ToggleSecondaryPane);
        assert!(state.narrow_secondary_visible());

        state.handle_resize();

        assert!(!state.narrow_secondary_visible());
    }

    #[test]
    fn request_quit_sets_the_quit_flag_when_there_is_nothing_to_lose() {
        let mut state = AppState::new();

        state.apply(Msg::RequestQuit);

        assert!(state.should_quit());
    }

    #[test]
    fn request_quit_with_an_unmodified_controller_still_quits_immediately() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);

        state.apply(Msg::RequestQuit);

        assert!(state.should_quit());
    }

    #[test]
    fn request_quit_with_a_modified_controller_requires_confirmation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));

        state.apply(Msg::RequestQuit);

        assert!(!state.should_quit());
        assert!(state.quit_confirmation_pending());
    }

    #[test]
    fn confirming_quit_sets_the_quit_flag() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::RequestQuit);

        state.apply(Msg::ConfirmQuit);

        assert!(state.should_quit());
    }

    #[test]
    fn cancelling_quit_keeps_the_session_open_and_the_edit_intact() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::RequestQuit);

        state.apply(Msg::CancelQuit);

        assert!(!state.should_quit());
        assert!(!state.quit_confirmation_pending());
        assert!(state.controller_modified());
    }
}
