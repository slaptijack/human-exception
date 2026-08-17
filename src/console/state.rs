//! Session/navigation state for the resistance console.
//!
//! This module is deliberately independent of any rendering or terminal
//! library so it can be tested without a real terminal and so later issues
//! can grow the state model without coupling it to widget code.

use super::editor::{EditOp, Editor};
use super::intel::authored_signals;
use crate::lua_controller::{self, ControllerError, LiveOperation, TickRecord};

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

/// A deployed controller and its accumulated live-run state.
///
/// `live` is `None` only when the deployment never started at all (the
/// source failed to load); `error` then holds the deploy-time failure.
/// Once `live` is `Some`, `error` stays `None` until a [`LiveOperation::step`]
/// call itself fails, at which point the run is over and `error` holds that
/// terminal failure instead. Both cases are "finished" from the player's
/// perspective, which [`Operation::is_finished`] treats uniformly.
#[derive(Debug)]
struct Operation {
    live: Option<LiveOperation>,
    /// The exact source that was deployed, independent of whatever the
    /// player has since typed into the editor (`docs/TUI_DESIGN.md`, "Run
    /// records and source provenance").
    deployed_source: String,
    /// A compact, session-local identifier for this deployment, shown in
    /// After Action / Review Run so the player can tell which run produced
    /// a given result even after the working controller source has since
    /// changed (`docs/TUI_DESIGN.md`, "Run records and source provenance").
    run_id: u32,
    /// One entry per completed tick, oldest first, for telemetry and the
    /// satellite feed's latest discovered state.
    records: Vec<TickRecord>,
    paused: bool,
    error: Option<ControllerError>,
}

impl Operation {
    fn is_finished(&self) -> bool {
        self.error.is_some() || self.live.as_ref().is_some_and(LiveOperation::is_finished)
    }
}

/// A read-only view of the current [`Operation`] for rendering, decoupled
/// from its internal representation (`LiveOperation` isn't meant to be
/// exposed directly outside this module).
pub struct OperationView<'a> {
    pub deployed_source: &'a str,
    pub run_id: u32,
    pub records: &'a [TickRecord],
    pub paused: bool,
    pub finished: bool,
    pub error: Option<&'a ControllerError>,
    pub starting_budget: u32,
    /// The most recent state to render the satellite feed and telemetry
    /// from: the last completed tick's, or (before any tick has run, or if
    /// deploy itself failed) a starting snapshot.
    pub current: OperationSnapshot,
}

/// A satellite-feed-safe snapshot of a deployment's current state — only
/// ever built from [`crate::simulation::Simulation::observe`]'s already
/// hidden-information-safe `Observation`, or the fixed scenario's public
/// starting facts, never from raw scenario/map internals.
pub struct OperationSnapshot {
    pub drone_position: crate::simulation::Position,
    pub map_width: i32,
    pub map_height: i32,
    pub discovered: Vec<crate::simulation::DiscoveredTile>,
    pub tick: u32,
    pub budget_remaining: u32,
}

/// A player intent, decoupled from whatever key produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Bracketed-paste text inserted at the current controller cursor as a
    /// single operation. Already newline-normalized (CRLF/CR -> LF) by the
    /// caller in `console::mod`; empty text must not touch `source` or
    /// `validation`.
    PasteController(String),
    /// Checks whether the current controller source is loadable Lua that
    /// defines `on_tick`, without running anything.
    ValidateController,
    /// `F7`: restores the starter controller, subject to confirmation if
    /// doing so would discard edits.
    RequestResetController,
    ConfirmResetController,
    CancelResetController,
    /// `Ctrl+Q`: quits, subject to confirmation if the controller is
    /// modified or a run is active.
    RequestQuit,
    ConfirmQuit,
    CancelQuit,
    /// `F6`: deploys the current controller source, subject to confirmation
    /// if another run is already active.
    RequestDeploy,
    ConfirmDeploy,
    CancelDeploy,
    /// `Space` in Operation: pauses or resumes the active run.
    TogglePauseOperation,
    /// `Enter` in Operation while paused: advances exactly one tick and
    /// remains paused.
    StepOperationTick,
}

/// The console's full session state.
///
/// Deliberately not `Clone`/`PartialEq`/`Eq` (unlike most other state in
/// this module): `operation` can hold a live `mlua::Lua` deployment via
/// [`Operation`]/[`LiveOperation`], which can't support either. Nothing
/// outside this module ever clones or compares a whole `AppState`.
#[derive(Debug)]
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
    /// `Ctrl+Q` was pressed with a modified controller or an active run; the
    /// player must confirm before the session exits.
    quit_confirmation_pending: bool,
    /// The current deployment, if the player has deployed a controller at
    /// least once this session. Persists (paused or finished) after the
    /// player navigates away or the run ends, until replaced by another
    /// deploy.
    operation: Option<Operation>,
    /// The `run_id` to assign to the next deployment, incremented every
    /// time [`AppState::deploy`] runs so each run gets a distinct,
    /// human-readable identifier.
    next_run_id: u32,
    /// `F6` was pressed while another run is still active; the player must
    /// confirm before it's replaced.
    redeploy_confirmation_pending: bool,
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
            operation: None,
            next_run_id: 1,
            redeploy_confirmation_pending: false,
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

    pub fn redeploy_confirmation_pending(&self) -> bool {
        self.redeploy_confirmation_pending
    }

    /// A read-only view of the current deployment, if the player has
    /// deployed a controller at least once this session.
    pub fn operation(&self) -> Option<OperationView<'_>> {
        self.operation.as_ref().map(|op| {
            let starting_budget = crate::simulation::Scenario::first_contact().starting_budget();
            let current = match (op.records.last(), op.live.as_ref()) {
                (Some(record), _) => OperationSnapshot {
                    drone_position: record.drone_position,
                    map_width: record.map_width,
                    map_height: record.map_height,
                    discovered: record.discovered.clone(),
                    tick: record.tick,
                    budget_remaining: record.budget_remaining,
                },
                (None, Some(live)) => {
                    let observation = live.observe();
                    OperationSnapshot {
                        drone_position: observation.drone_position,
                        map_width: live.map_width(),
                        map_height: live.map_height(),
                        discovered: observation.discovered,
                        tick: observation.tick,
                        budget_remaining: observation.budget_remaining,
                    }
                }
                // Deploy itself failed: nothing was ever observed, so fall
                // back to the fixed scenario's public starting facts rather
                // than any raw map/scenario internals.
                (None, None) => {
                    let scenario = crate::simulation::Scenario::first_contact();
                    OperationSnapshot {
                        drone_position: scenario.drone_start(),
                        map_width: scenario.map().width(),
                        map_height: scenario.map().height(),
                        discovered: Vec::new(),
                        tick: 0,
                        budget_remaining: starting_budget,
                    }
                }
            };
            OperationView {
                deployed_source: &op.deployed_source,
                run_id: op.run_id,
                records: &op.records,
                paused: op.paused,
                finished: op.is_finished(),
                error: op.error.as_ref(),
                starting_budget,
                current,
            }
        })
    }

    /// Whether an operation exists and hasn't finished (succeeded, failed,
    /// or errored) yet — i.e. there is something a redeploy or quit would
    /// abandon.
    fn operation_active(&self) -> bool {
        self.operation.as_ref().is_some_and(|op| !op.is_finished())
    }

    /// Whether the current view should be advancing the active run one tick
    /// per timer wakeup, independent of any player key: `View::Operation`
    /// is showing, a deployment exists, and it's neither paused nor
    /// finished. Navigating away (`F2`/`F3`/`F4`) sets `paused` so this
    /// naturally stops; opening Help changes `current_view` away from
    /// `Operation` without touching `paused`, so this also naturally stops
    /// and then resumes on its own once Help is dismissed back to
    /// `Operation` — see `docs/TUI_DESIGN.md`, "Navigation while a
    /// deployment is active."
    pub fn operation_auto_advancing(&self) -> bool {
        self.current_view == View::Operation
            && self
                .operation
                .as_ref()
                .is_some_and(|op| !op.paused && !op.is_finished())
    }

    /// Advances the active run by exactly one tick if [`operation_auto_advancing`]
    /// allows it right now, returning whether anything actually changed (and
    /// so a redraw is needed). Driven by the terminal event loop's timer,
    /// not a player key — see [`Msg::StepOperationTick`] for the
    /// player-initiated equivalent while paused.
    ///
    /// [`operation_auto_advancing`]: AppState::operation_auto_advancing
    pub fn advance_running_operation(&mut self) -> bool {
        if !self.operation_auto_advancing() {
            return false;
        }
        self.step_operation()
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
                    // "F2, F3, or F4 while a run is active first pauses the
                    // run, then navigates" (`docs/TUI_DESIGN.md`, "Navigation
                    // while a deployment is active"). Operation and
                    // AfterAction are excluded: returning to Operation must
                    // preserve whatever paused/running state the run was
                    // already in, not force a pause.
                    if matches!(view, View::Signals | View::Target | View::Controller)
                        && let Some(op) = self.operation.as_mut()
                        && !op.is_finished()
                    {
                        op.paused = true;
                    }
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
            Msg::PasteController(text) => {
                if let Some(controller) = self.controller.as_mut()
                    && controller.insert_text(&text)
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
                if self.controller_modified() || self.operation_active() {
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
            Msg::RequestDeploy => {
                if self.controller_source().is_some() {
                    if self.operation_active() {
                        // The redeploy-confirmation prompt only ever
                        // renders inside Operation (`ui::draw_operation`).
                        // `F6` is a *global* binding (reachable from
                        // Controller, Signals, etc. while a paused run sits
                        // in the background), so setting the flag alone
                        // would leave the player staring at whatever view
                        // they were already on with most keys silently
                        // swallowed by the pending-confirmation guard and
                        // no visible explanation why. Surfacing the prompt
                        // takes priority here over Operation's own
                        // leaving-pauses-the-run rule — there's no running
                        // presentation to interrupt yet, just a decision to
                        // make before one exists.
                        self.redeploy_confirmation_pending = true;
                        self.current_view = View::Operation;
                        self.narrow_secondary_visible = false;
                    } else {
                        self.deploy();
                    }
                }
            }
            Msg::ConfirmDeploy => {
                self.deploy();
                self.redeploy_confirmation_pending = false;
            }
            Msg::CancelDeploy => {
                self.redeploy_confirmation_pending = false;
            }
            Msg::TogglePauseOperation => {
                if let Some(op) = self.operation.as_mut()
                    && !op.is_finished()
                {
                    op.paused = !op.paused;
                }
            }
            Msg::StepOperationTick => {
                // "`Enter` advances exactly one tick while paused and
                // remains paused afterward" (`docs/TUI_DESIGN.md`, "Pacing
                // controls") — while running, ticks already advance on
                // their own via `advance_running_operation`, so `Enter`
                // manually stepping too would silently fast-forward past
                // what the player is watching.
                if self.operation.as_ref().is_some_and(|op| op.paused) {
                    self.step_operation();
                }
            }
        }
    }

    /// Deploys the current controller source as a fresh [`Operation`],
    /// replacing any prior one, and switches to Operation. A no-op if no
    /// controller source is loaded (nothing to deploy). Called for both a
    /// fresh `F6` (no active run to replace) and a confirmed redeploy.
    fn deploy(&mut self) {
        let Some(source) = self.controller_source() else {
            return;
        };
        let source = source.to_string();
        let run_id = self.next_run_id;
        self.next_run_id += 1;

        let operation = match LiveOperation::deploy(&source) {
            Ok(live) => Operation {
                live: Some(live),
                deployed_source: source,
                run_id,
                records: Vec::new(),
                paused: false,
                error: None,
            },
            Err(err) => Operation {
                live: None,
                deployed_source: source,
                run_id,
                records: Vec::new(),
                paused: false,
                error: Some(err),
            },
        };
        // A deployment that fails to load (bad Lua syntax, no `on_tick`)
        // finishes the instant it's created, with no live run to watch —
        // send the player straight to After Action's compact report rather
        // than parking them on an Operation view that has nothing to show
        // yet (`docs/TUI_DESIGN.md`, "After Action is an operation state,
        // not a disconnected popup").
        self.current_view = if operation.is_finished() {
            View::AfterAction
        } else {
            View::Operation
        };
        self.operation = Some(operation);
        self.narrow_secondary_visible = false;
    }

    /// Advances the active operation by exactly one tick, appending the
    /// result or recording the terminal error. A no-op (returns `false`) if
    /// there's no operation, it already finished, or it never successfully
    /// deployed. Returns whether a tick actually advanced.
    fn step_operation(&mut self) -> bool {
        let Some(op) = self.operation.as_mut() else {
            return false;
        };
        if op.is_finished() {
            return false;
        }
        let Some(live) = op.live.as_mut() else {
            return false;
        };
        match live.step() {
            Ok(record) => op.records.push(record),
            Err(err) => op.error = Some(err),
        }
        // The run just reached a terminal outcome while the player was
        // watching Operation (the only view this can be reached from — see
        // `AppState::deploy` for the synchronous-failure case, which can
        // happen from anywhere else). Hand off to After Action so the
        // player moves from "watching it run" to "learning what happened."
        if op.is_finished() {
            self.current_view = View::AfterAction;
            self.narrow_secondary_visible = false;
        }
        true
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
    fn paste_controller_inserts_multiline_text_and_invalidates_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);

        state.apply(Msg::PasteController("-- a\n-- b\n".to_string()));

        assert!(
            state
                .controller_source()
                .is_some_and(|source| source.ends_with("-- a\n-- b\n"))
        );
        assert_eq!(state.validation(), &Validation::Unchecked);
    }

    #[test]
    fn paste_controller_with_empty_string_does_not_change_source_or_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);
        let source_before = state.controller_source().unwrap().to_string();

        state.apply(Msg::PasteController(String::new()));

        assert_eq!(state.controller_source(), Some(source_before.as_str()));
        assert_eq!(state.validation(), &Validation::Valid);
    }

    #[test]
    fn paste_controller_with_no_loaded_controller_is_a_noop() {
        let mut state = AppState::new();
        assert_eq!(state.controller_source(), None);

        state.apply(Msg::PasteController("x".to_string()));

        assert_eq!(state.controller_source(), None);
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

    /// Commits to First Contact (seeding the starter controller) and
    /// returns a ready-to-deploy state, matching every deploy-focused
    /// test's shared setup.
    fn working_state() -> AppState {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state
    }

    const ALWAYS_ERRORS: &str = "function on_tick(observation) error('boom') end";
    const ROUTE_TO_UPLINK: &str = r#"
        local route = { "north", "east", "east", "east", "east", "north", "north", "north" }
        local step = 0
        function on_tick(observation)
            step = step + 1
            return route[step]
        end
    "#;

    #[test]
    fn deploying_the_starter_controller_navigates_to_operation_and_starts_running() {
        let mut state = working_state();

        state.apply(Msg::RequestDeploy);

        assert_eq!(state.current_view(), View::Operation);
        let op = state.operation().expect("a deploy just happened");
        assert!(!op.paused);
        assert!(!op.finished);
        assert!(op.records.is_empty());
        assert_eq!(op.deployed_source, super::super::intel::STARTER_CONTROLLER);
    }

    #[test]
    fn deploying_a_script_with_a_load_error_surfaces_it_without_a_live_run() {
        let mut state = working_state();
        state.controller = Some(Editor::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        let op = state.operation().expect("deploy always records an outcome");
        assert!(op.finished);
        assert!(matches!(op.error, Some(ControllerError::ScriptInvalid(_))));
        // No live run was ever shown — go straight to the After Action
        // report rather than parking on an empty Operation view.
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn redeploying_a_finished_operation_needs_no_confirmation() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();
        assert!(state.operation().unwrap().finished);
        assert_eq!(state.current_view(), View::AfterAction);

        state.apply(Msg::RequestDeploy);

        assert!(!state.redeploy_confirmation_pending());
        // A fresh deploy replaces the finished operation's records and
        // resumes on Operation since the starter controller loads fine.
        assert!(state.operation().unwrap().records.is_empty());
        assert_eq!(state.current_view(), View::Operation);
    }

    #[test]
    fn redeploying_an_active_operation_requires_confirmation() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy); // starter controller: running, unfinished

        state.apply(Msg::RequestDeploy);

        assert!(state.redeploy_confirmation_pending());
        // The original run is untouched until confirmed.
        assert!(state.operation().unwrap().records.is_empty());
    }

    #[test]
    fn requesting_redeploy_from_elsewhere_navigates_to_operation_so_the_prompt_is_visible() {
        // F6 is a global binding: a player can pause a run by leaving to
        // Controller/Signals/Target and press F6 from there. The
        // confirmation prompt only ever renders inside Operation, so the
        // request must bring the player back to it — otherwise the pending
        // flag silently swallows most other input on a screen that gives
        // no indication anything is waiting on a response.
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        state.apply(Msg::Navigate(View::Controller)); // pauses the run
        assert_eq!(state.current_view(), View::Controller);

        state.apply(Msg::RequestDeploy);

        assert!(state.redeploy_confirmation_pending());
        assert_eq!(state.current_view(), View::Operation);
    }

    #[test]
    fn confirming_redeploy_replaces_the_active_operation() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        state.apply(Msg::RequestDeploy); // pending confirmation

        state.apply(Msg::ConfirmDeploy);

        assert!(!state.redeploy_confirmation_pending());
        assert_eq!(state.current_view(), View::Operation);
    }

    #[test]
    fn cancelling_redeploy_leaves_the_active_operation_running() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();
        state.apply(Msg::RequestDeploy); // pending confirmation

        state.apply(Msg::CancelDeploy);

        assert!(!state.redeploy_confirmation_pending());
        assert_eq!(state.operation().unwrap().records.len(), 1);
    }

    #[test]
    fn step_operation_tick_advances_exactly_one_tick_and_stays_paused() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        state.apply(Msg::TogglePauseOperation);
        assert!(state.operation().unwrap().paused);

        state.apply(Msg::StepOperationTick);

        let op = state.operation().unwrap();
        assert_eq!(op.records.len(), 1);
        assert!(op.paused, "stepping while paused must not resume the run");
    }

    #[test]
    fn toggle_pause_flips_paused_and_is_a_no_op_once_finished() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();
        assert!(state.operation().unwrap().finished);

        state.apply(Msg::TogglePauseOperation);

        assert!(
            !state.operation().unwrap().paused,
            "a finished operation can't be paused/resumed"
        );
    }

    #[test]
    fn navigating_away_from_an_active_operation_pauses_it() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        assert!(!state.operation().unwrap().paused);

        state.apply(Msg::Navigate(View::Controller));

        assert!(state.operation().unwrap().paused);
        assert_eq!(state.current_view(), View::Controller);
    }

    #[test]
    fn returning_to_operation_preserves_the_paused_state() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        state.apply(Msg::Navigate(View::Controller));

        state.apply(Msg::Navigate(View::Operation));

        assert!(
            state.operation().unwrap().paused,
            "F2/F3/F4 pauses; returning to Operation must not silently resume"
        );
    }

    #[test]
    fn opening_and_dismissing_help_does_not_change_the_paused_flag() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        assert!(!state.operation().unwrap().paused);

        state.apply(Msg::OpenHelp);
        assert!(
            !state.operation().unwrap().paused,
            "Help pauses presentation via operation_auto_advancing, not the paused flag itself"
        );

        state.apply(Msg::DismissHelp);
        assert!(!state.operation().unwrap().paused);
    }

    #[test]
    fn operation_auto_advancing_only_while_viewing_operation_unpaused_and_unfinished() {
        let mut state = working_state();
        assert!(!state.operation_auto_advancing(), "nothing deployed yet");

        state.apply(Msg::RequestDeploy);
        assert!(state.operation_auto_advancing());

        state.apply(Msg::Navigate(View::Controller));
        assert!(
            !state.operation_auto_advancing(),
            "left Operation and the run is now paused"
        );

        state.apply(Msg::Navigate(View::Operation));
        assert!(
            !state.operation_auto_advancing(),
            "back on Operation, but still paused from leaving"
        );

        state.apply(Msg::TogglePauseOperation);
        assert!(state.operation_auto_advancing());
    }

    #[test]
    fn advance_running_operation_steps_once_when_auto_advancing_and_otherwise_is_a_no_op() {
        let mut state = working_state();
        assert!(!state.advance_running_operation());

        state.apply(Msg::RequestDeploy);
        let advanced = state.advance_running_operation();

        assert!(advanced);
        assert_eq!(state.operation().unwrap().records.len(), 1);

        state.apply(Msg::TogglePauseOperation);
        assert!(!state.advance_running_operation());
        assert_eq!(state.operation().unwrap().records.len(), 1);
    }

    #[test]
    fn a_callback_error_finishes_the_operation_and_is_recorded() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);

        state.advance_running_operation();

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert!(matches!(op.error, Some(ControllerError::CallbackFailed(_))));
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn running_a_route_to_completion_reaches_an_unambiguous_success() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        for _ in 0..8 {
            state.advance_running_operation();
        }

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert!(op.error.is_none());
        assert_eq!(
            op.records.last().map(|record| record.outcome),
            Some(crate::simulation::TickOutcome::Succeeded)
        );
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn reviewing_a_finished_run_returns_to_operation_with_the_frozen_telemetry() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        for _ in 0..8 {
            state.advance_running_operation();
        }
        assert_eq!(state.current_view(), View::AfterAction);

        state.apply(Msg::Navigate(View::Operation));

        assert_eq!(state.current_view(), View::Operation);
        let op = state.operation().unwrap();
        assert_eq!(op.records.len(), 8);
        assert!(op.finished);
    }

    #[test]
    fn each_deploy_gets_a_distinct_run_id() {
        let mut state = working_state();

        state.apply(Msg::RequestDeploy); // starter controller: still running
        let first_run_id = state.operation().unwrap().run_id;

        state.apply(Msg::RequestDeploy); // active run: needs confirmation
        state.apply(Msg::ConfirmDeploy);
        let second_run_id = state.operation().unwrap().run_id;

        assert_ne!(first_run_id, second_run_id);
    }

    #[test]
    fn quitting_with_an_active_operation_requires_confirmation() {
        let mut state = working_state();
        assert!(!state.controller_modified());
        state.apply(Msg::RequestDeploy);

        state.apply(Msg::RequestQuit);

        assert!(!state.should_quit());
        assert!(state.quit_confirmation_pending());
    }

    #[test]
    fn quitting_after_the_operation_finished_needs_no_confirmation_for_it() {
        let mut state = working_state();
        state.controller = Some(Editor::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();
        assert!(state.operation().unwrap().finished);
        // Restore the controller to the unmodified starter so this test
        // isolates the finished operation's own contribution to the quit
        // gate from the (unrelated) modified-controller gate.
        state.controller = Some(Editor::new(super::super::intel::STARTER_CONTROLLER));
        assert!(!state.controller_modified());

        state.apply(Msg::RequestQuit);

        assert!(state.should_quit());
    }
}
