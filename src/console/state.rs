//! Session/navigation state for the resistance console.
//!
//! This module is deliberately independent of any rendering or terminal
//! library so it can be tested without a real terminal and so later issues
//! can grow the state model without coupling it to widget code.

use std::collections::HashMap;

use super::document::ControllerDocument;
use super::editor::EditOp;
use super::intel::visible_signals;
use crate::lua_controller::{self, ControllerError, LiveOperation, TickRecord};

/// An upper bound on how far any scrollable pane can scroll. This exists
/// only so `ScrollDown` can never run away toward `u16::MAX` and leave the
/// player pressing `Up` an impractical number of times to recover — the
/// real, content- and frame-size-aware bound is a per-pane `*_max_scroll`
/// function (e.g. `ui::help_max_scroll`), which `console::should_redraw`
/// re-clamps the stored offset against after every scroll key, so this only
/// needs to be comfortably above any scrollable pane's actual line count
/// (not tuned to match it), not an accurate ceiling in its own right. A
/// ceiling tuned too close to that count silently caps scrolling below the
/// real content height the next time a scrollable pane's content grows.
const MAX_PANE_SCROLL: u16 = 500;

/// The ordered synchronization/update steps Network Bootstrap reveals one at
/// a time after First Contact's connecting success (`docs/TUI_DESIGN.md`,
/// "Network Bootstrap"). Wording, step count, and styling are implementation
/// details left open by that section — this list is restrained package/
/// signature-style status text, never a first-person `slaptijack@` voice.
pub(crate) const NETWORK_BOOTSTRAP_STEPS: &[&str] = &[
    "uplink established",
    "peer network reachable",
    "syncing console package index",
    "verifying module signatures",
    "syncing operator keys (signed: slaptijack@)",
    "subscribing to shared intel feed",
    "network services online",
];

/// How many extra ticks Network Bootstrap holds at its fully-revealed,
/// 100%-progress state before the transition actually completes and the
/// modal closes — beyond the one cadence that showing the last step itself
/// already occupies. Issue #179 calls for the completed state to linger
/// "briefly" so the Player can register completion before the reveal; a
/// single cadence read as closing immediately, so this gives it more than
/// one.
pub(crate) const NETWORK_BOOTSTRAP_LINGER_TICKS: usize = 2;

/// Whether `pane` can be scrolled at all. Only [`PaneId::Help`] and
/// [`PaneId::Report`] have scrollable content today — every other pane must
/// never accumulate a `scroll_offsets` entry. Kept in sync with the
/// pane-to-max-scroll match in `console::pane_max_scroll`, which decides the
/// same set of panes for render-time clamping.
fn pane_is_scrollable(pane: PaneId) -> bool {
    matches!(pane, PaneId::Help | PaneId::Report)
}

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

impl View {
    /// The pane focused in `self` by default, and whenever no explicit
    /// focus has been recorded yet (`docs/TUI_DESIGN.md`, "Pane focus" >
    /// "Default focus").
    pub(crate) fn default_pane(self) -> PaneId {
        match self {
            View::Help => PaneId::Help,
            View::Signals => PaneId::SignalsList,
            View::Target => PaneId::TargetIntelligence,
            View::Controller => PaneId::ControllerSource,
            View::Operation => PaneId::Satellite,
            View::AfterAction => PaneId::Report,
        }
    }

    /// Every pane that belongs to `self`, in the order defined by
    /// `docs/TUI_DESIGN.md`'s "Panes per view" table.
    fn panes(self) -> &'static [PaneId] {
        match self {
            View::Help => &[PaneId::Help],
            View::Signals => &[PaneId::SignalsList, PaneId::SelectedSignal],
            View::Target => &[PaneId::TargetIntelligence, PaneId::Provenance],
            View::Controller => &[PaneId::ControllerSource, PaneId::LuaFieldReference],
            View::Operation => &[PaneId::Satellite, PaneId::OperationTelemetry],
            View::AfterAction => &[PaneId::Report, PaneId::FinalFrame],
        }
    }
}

/// Which of the two read-only surfaces the Run Inspector (`PaneId::
/// OperationTelemetry`, once a deployment has finished) currently shows —
/// `docs/TUI_DESIGN.md`, "Review Run". `Tab`, while the Run Inspector is
/// focused, toggles between them (`navigation::focused_nav_surface`
/// returning `NavSurface::ReviewRun`).
/// `Timeline` is the chronology index plus the selected review point's
/// evidence (unchanged by this mode's existence); `Source` is the complete,
/// unbounded `Operation::deployed_source`, independently scrollable via
/// [`AppState::source_scroll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunInspectorMode {
    #[default]
    Timeline,
    Source,
}

/// A named pane within a multi-pane view (`docs/TUI_DESIGN.md`, "Pane
/// focus"). Every view has one or two of these; which combinations are
/// valid is defined by [`View::panes`], and the only place `AppState` writes
/// a pane into its per-view focus map (`AppState::set_focused_pane`)
/// enforces that a pane always belongs to the view it's recorded against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    Help,
    SignalsList,
    SelectedSignal,
    TargetIntelligence,
    Provenance,
    ControllerSource,
    LuaFieldReference,
    Satellite,
    OperationTelemetry,
    Report,
    FinalFrame,
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
    /// The legitimate pre-tick observation, captured once immediately after
    /// `LiveOperation::deploy` succeeded and before any tick executed.
    /// `None` only when deploy itself failed and no simulation ever
    /// started — see `Operation::live`'s doc comment. Unlike `records`,
    /// this never changes once set: Review Run must be able to tell what
    /// was already known at deployment from what tick 1 discovered
    /// (`docs/TUI_DESIGN.md`, "After Action is an operation state, not a
    /// disconnected popup").
    initial_snapshot: Option<OperationSnapshot>,
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
    /// How the run ended, if it has, so rendering doesn't need to
    /// re-infer meaning from `records`/`error` itself. `None` until the
    /// operation is finished.
    pub conclusion: Option<OperationConclusion<'a>>,
    /// The Review Run chronology: `INITIAL`, then one point per completed
    /// tick, then an optional terminal-failure boundary. Empty when deploy
    /// itself failed (no execution ever started) — see [`review_points`].
    pub review_points: Vec<ReviewPoint<'a>>,
    /// The player's currently selected review point, as an index into
    /// `review_points` above. `None` before the run has finished, or if
    /// `review_points` is empty (a deploy-time load failure never produced
    /// a reviewable point). Re-clamped here against the freshly computed
    /// `review_points`, so a stale stored index can never be read as valid.
    pub review_selected: Option<usize>,
    /// Which of the Run Inspector's two modes is currently showing.
    pub run_inspector_mode: RunInspectorMode,
    /// The Run Inspector's current scroll offset into `deployed_source`
    /// while `run_inspector_mode` is [`RunInspectorMode::Source`]; ignored
    /// in [`RunInspectorMode::Timeline`].
    pub source_scroll: u16,
}

/// How a finished deployment ended, derived once from the authoritative
/// `TickOutcome`/`ControllerError` rather than left for rendering to
/// re-infer from raw records or drone position.
#[derive(Debug, Clone, Copy)]
pub enum ConclusionKind<'a> {
    Success,
    BudgetExhausted,
    ControllerError(&'a ControllerError),
}

/// A structured, read-only account of how a finished deployment ended,
/// with the evidence `docs/TUI_DESIGN.md` §5 requires on the initial
/// After Action screen (final budget, ticks executed, tiles discovered,
/// hazards entered, run identifier) precomputed so rendering doesn't
/// recalculate gameplay rules.
#[derive(Debug, Clone, Copy)]
pub struct OperationConclusion<'a> {
    pub kind: ConclusionKind<'a>,
    pub ticks_executed: u32,
    pub tiles_discovered: u32,
    pub hazards_entered: u32,
    pub final_budget: u32,
    pub run_id: u32,
}

/// A satellite-feed-safe snapshot of a deployment's current state — only
/// ever built from [`crate::simulation::Simulation::observe`]'s already
/// hidden-information-safe `Observation`, or the fixed scenario's public
/// starting facts, never from raw scenario/map internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSnapshot {
    pub drone_position: crate::simulation::Position,
    pub map_width: i32,
    pub map_height: i32,
    pub discovered: Vec<crate::simulation::DiscoveredTile>,
    pub tick: u32,
    pub budget_remaining: u32,
}

/// A hidden-information-safe snapshot of `live`'s current state, built only
/// from [`LiveOperation::observe`] and the fixed scenario's public map
/// dimensions — never from raw scenario/simulation internals.
fn observe_snapshot(live: &LiveOperation) -> OperationSnapshot {
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

/// One inspectable boundary in a finished (or in-progress) operation's
/// chronology for Review Run: `INITIAL`, then one point per completed
/// tick, then an optional terminal-failure boundary. Derived only from the
/// immutable run record ([`Operation`]'s `initial_snapshot`/`records`/
/// `error`) — never by replaying the simulation or consulting hidden
/// scenario state (`docs/VISION.md`; epic #130's "Review chronology
/// contract").
#[derive(Debug, Clone)]
pub struct ReviewPoint<'a> {
    pub kind: ReviewPointKind<'a>,
    /// The satellite-safe state at this point: drone position, budget, map
    /// dimensions, and the full cumulative discovered set.
    pub snapshot: OperationSnapshot,
    /// Tiles present in `snapshot.discovered` but absent from the
    /// preceding review point's. Empty for [`ReviewPointKind::Initial`]
    /// (nothing precedes it, so nothing is "new" relative to a prior
    /// point) and for [`ReviewPointKind::TerminalFailure`] (its snapshot
    /// is identical to the point before it, since no tick executed to
    /// discover anything new).
    pub newly_discovered: Vec<crate::simulation::DiscoveredTile>,
}

/// What kind of chronology boundary a [`ReviewPoint`] represents.
#[derive(Debug, Clone, Copy)]
pub enum ReviewPointKind<'a> {
    /// The legitimate pre-tick observation: no action, no events.
    Initial,
    /// A completed tick: action, resulting position/budget, outcome, and
    /// structured events/costs all come from the borrowed [`TickRecord`].
    Tick(&'a TickRecord),
    /// A controller/runtime failure after the deployment started but with
    /// no further valid `TickRecord` — the run's terminal `error` while
    /// `initial_snapshot` is `Some`. Carries the last known position and
    /// budget (the preceding point's snapshot) rather than a fabricated
    /// tick.
    TerminalFailure(&'a ControllerError),
}

/// The [`crate::simulation::DiscoveredTile`]s in `next` whose position
/// isn't present in `previous`. Both `discovered` lists are cumulative and
/// only ever grow (`Simulation::observe`), so a position-keyed set
/// difference is exact regardless of ordering.
fn discovered_since(
    previous: &[crate::simulation::DiscoveredTile],
    next: &[crate::simulation::DiscoveredTile],
) -> Vec<crate::simulation::DiscoveredTile> {
    let known: std::collections::HashSet<_> = previous.iter().map(|tile| tile.position).collect();
    next.iter()
        .filter(|tile| !known.contains(&tile.position))
        .copied()
        .collect()
}

/// Projects an operation's immutable facts into its ordered Review Run
/// chronology. Returns an empty `Vec` when `initial_snapshot` is `None`:
/// deploy itself failed before any execution started, so there is no
/// reviewable point at all — not even a fabricated `INITIAL` — matching
/// the deploy-failure boundary already carried by [`OperationView::error`].
fn review_points<'a>(
    initial_snapshot: &'a Option<OperationSnapshot>,
    records: &'a [TickRecord],
    error: Option<&'a ControllerError>,
) -> Vec<ReviewPoint<'a>> {
    let Some(initial) = initial_snapshot else {
        return Vec::new();
    };

    let mut points = vec![ReviewPoint {
        kind: ReviewPointKind::Initial,
        snapshot: initial.clone(),
        newly_discovered: Vec::new(),
    }];

    let mut previous_discovered = &initial.discovered;
    let mut last_snapshot = initial.clone();
    for record in records {
        let newly_discovered = discovered_since(previous_discovered, &record.discovered);
        let snapshot = OperationSnapshot {
            drone_position: record.drone_position,
            map_width: record.map_width,
            map_height: record.map_height,
            discovered: record.discovered.clone(),
            tick: record.tick,
            budget_remaining: record.budget_remaining,
        };
        points.push(ReviewPoint {
            kind: ReviewPointKind::Tick(record),
            snapshot: snapshot.clone(),
            newly_discovered,
        });
        previous_discovered = &record.discovered;
        last_snapshot = snapshot;
    }

    // `error` is only ever set at deploy time (which forces
    // `initial_snapshot` to `None`, already handled above) or when
    // `LiveOperation::step` itself returns `Err` — never alongside a
    // `Succeeded`/`BudgetExhausted` `TickOutcome`, which instead arrives as
    // `Ok(record)` and is pushed onto `records` above. So this can never
    // duplicate a real terminal tick.
    if let Some(err) = error {
        points.push(ReviewPoint {
            kind: ReviewPointKind::TerminalFailure(err),
            snapshot: last_snapshot,
            newly_discovered: Vec::new(),
        });
    }

    points
}

/// A player intent, decoupled from whatever key produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Navigate(View),
    OpenHelp,
    DismissHelp,
    /// `Enter` while the first-launch bootstrap introduction (issue #173)
    /// is showing: dismisses it for the rest of this session and durably,
    /// so it never shows again.
    AcknowledgeBootstrapIntro,
    SelectPreviousSignal,
    SelectNextSignal,
    SelectFirstSignal,
    SelectLastSignal,
    /// `0` is a placeholder page size, rewritten to the real visible list
    /// page by `console::mod::should_redraw` — the same convention
    /// `SelectReviewPointPageBackward`/`PageForward` already use.
    SelectSignalPageBackward(usize),
    SelectSignalPageForward(usize),
    /// Context-sensitive "Enter": inspects the selected signal in Signals,
    /// or commits to working the current dossier's opportunity in Target.
    Activate,
    /// `F8`: moves focus to the next pane in the current view, wrapping
    /// around. A no-op in single-pane views (Help). Both panes of a
    /// two-pane view always render; this only moves which one carries the
    /// focus marker and input ownership.
    FocusNextPane,
    /// Scrolls the current view's focused pane up/down by one row, if it's
    /// scrollable (see `navigation::focused_nav_surface`'s `NavSurface::
    /// Scroll`). Applies to whichever pane is focused when the message is
    /// applied.
    ScrollUp,
    ScrollDown,
    /// Moves the focused scrollable pane's offset backward/forward by one
    /// visible page, clamped at the top/bottom the same way
    /// `ScrollUp`/`ScrollDown` are. The carried count is the number of rows
    /// actually visible at the current frame size; `event::map` has no
    /// access to rendered geometry, so it always emits `0` here, and
    /// `console::mod`'s dispatch loop rewrites it from
    /// `ui::scroll_pane_visible_rows` before calling `apply` — the same
    /// placeholder-and-rewrite pattern `SelectReviewPointPageBackward`/
    /// `PageForward` and `ScrollSourcePageBackward`/`PageForward` already
    /// use.
    ScrollPageBackward(usize),
    ScrollPageForward(usize),
    /// Jumps the focused scrollable pane's offset straight to the
    /// beginning/end of its content. `JumpScrollEnd` sets a large sentinel
    /// value, clamped down to the real end by `console::should_redraw` the
    /// same way `ScrollDown`/`JumpSourceEnd` are.
    JumpScrollStart,
    JumpScrollEnd,
    /// Review Run chronology navigation (`navigation::focused_nav_surface`
    /// returning `NavSurface::ReviewRun`): moves `review_selected` by one
    /// review point, clamped at the first/
    /// last point — never wraps. A no-op unless the focused run is
    /// finished and has at least one review point (live Operation pacing
    /// is unaffected; see `Msg::TogglePauseOperation`/`StepOperationTick`).
    SelectPreviousReviewPoint,
    SelectNextReviewPoint,
    /// Jumps straight to the first/terminal review point — semantic
    /// chronology jumps, not viewport movement.
    SelectFirstReviewPoint,
    SelectLastReviewPoint,
    /// Moves `review_selected` backward/forward by one visible chronology
    /// page, clamped at the first/last point. The carried count is the
    /// number of chronology rows actually visible at the current frame
    /// size; `event::map` has no access to rendered geometry, so it always
    /// emits `0` here, and `console::mod`'s dispatch loop rewrites it from
    /// `ui::review_chronology_visible_rows` before calling `apply` — the
    /// same place that already recomputes `pane_max_scroll` to clamp
    /// `ScrollUp`/`ScrollDown` after applying them.
    SelectReviewPointPageBackward(usize),
    SelectReviewPointPageForward(usize),
    /// `Tab`, while the Run Inspector is focused on a finished run
    /// (`navigation::focused_nav_surface` returning `NavSurface::
    /// ReviewRun`): flips `run_inspector_mode`
    /// between `Timeline` and `Source`. A no-op unless the focused run is
    /// finished and has at least one review point — the same gate
    /// chronology navigation shares (`AppState::review_chronology_len`).
    ToggleRunInspectorMode,
    /// Scrolls `source_scroll` by one row while `run_inspector_mode` is
    /// `Source`. `event::map` only emits these when that mode is active
    /// (the same key otherwise produces `SelectPrevious`/`NextReviewPoint`
    /// in `Timeline`), so `apply` need only re-check the run-finished gate,
    /// not the mode itself.
    ScrollSourceUp,
    ScrollSourceDown,
    /// Moves `source_scroll` backward/forward by one visible source-pane
    /// page. Carries a placeholder `0` from `event::map`, rewritten by
    /// `console::mod`'s dispatch loop from `ui::review_source_visible_rows`
    /// before calling `apply` — the same placeholder-and-rewrite pattern
    /// `SelectReviewPointPageBackward`/`PageForward` already use.
    ScrollSourcePageBackward(usize),
    ScrollSourcePageForward(usize),
    /// Jumps `source_scroll` straight to the beginning/end of
    /// `deployed_source`. `JumpSourceEnd` sets a large sentinel value,
    /// clamped down to the real end by `console::should_redraw` the same
    /// way `ScrollDown` is clamped via `pane_max_scroll`/`clamp_scroll`.
    JumpSourceStart,
    JumpSourceEnd,
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
    /// Index into [`visible_signals`], moved by `SelectPreviousSignal` /
    /// `SelectNextSignal`.
    selected_signal: usize,
    /// True once the player has inspected the actionable signal, making
    /// Target reachable even before a working set is committed.
    target_known: bool,
    /// The player's current Lua source and cursor for the working set,
    /// seeded from the starter controller the first time an opportunity is
    /// committed to.
    controller: Option<ControllerDocument>,
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
    /// Scroll offset for each scrollable pane, keyed by [`PaneId`]. A
    /// missing entry means an offset of 0 — only panes a player has
    /// actually scrolled ever get an entry; non-scrollable panes (see
    /// `pane_is_scrollable`) never get one.
    scroll_offsets: HashMap<PaneId, u16>,
    /// Focused pane for each view, keyed by [`View`]. A missing entry means
    /// that view's documented default pane ([`View::default_pane`]) — only
    /// views whose focus has actually diverged from the default get an
    /// entry.
    focused_panes: HashMap<View, PaneId>,
    /// Index into the current operation's `review_points`, or `None` before
    /// the run has finished or if it produced no reviewable point at all (a
    /// deploy-time load failure). Lives independently of `Operation` — see
    /// its own doc comment — so Review Run can track "what the player is
    /// looking at" without the immutable run record ever needing to
    /// represent navigation. Set once a run finishes ([`AppState::deploy`],
    /// [`AppState::step_operation`]) and otherwise left untouched, so
    /// navigating away and back, and opening/dismissing Help, preserve it
    /// for free; a fresh deploy always resets it.
    review_selected: Option<usize>,
    /// Which Run Inspector mode is currently showing (`docs/TUI_DESIGN.md`,
    /// "Review Run"). Lives independently of `Operation`, the same way
    /// `review_selected` does, and for the same reason: reset only where
    /// `review_selected` itself is reset (`AppState::deploy`,
    /// `AppState::step_operation`'s terminal branch), and otherwise left
    /// untouched, so navigating away and back preserves it for free.
    run_inspector_mode: RunInspectorMode,
    /// The Run Inspector's scroll offset into `deployed_source` while
    /// `run_inspector_mode` is [`RunInspectorMode::Source`]. Reset alongside
    /// `run_inspector_mode`; otherwise preserved the same way, so leaving
    /// and returning to the same reviewed run's SOURCE view keeps the
    /// player's place. The real content-and-frame-size-aware bound is
    /// `ui::review_source_max_scroll`, which `console::should_redraw`
    /// re-clamps this against after every source-scroll key, mirroring
    /// `scroll_offsets`/`clamp_scroll`.
    source_scroll: u16,
    should_quit: bool,
    /// Whether this Player has established operator-network connectivity by
    /// succeeding at First Contact — the one fact
    /// [`console::profile`](super::profile) durably persists across
    /// relaunches (`docs/TUI_DESIGN.md`, "Bootstrap and network
    /// connectivity"). Lives independently of `current_view`/`operation` so
    /// it can be seeded from disk before the session's first draw and
    /// queried without inferring it from transient state. Set exactly once,
    /// in [`AppState::step_operation`]'s terminal branch, and never
    /// reverted.
    connected: bool,
    /// Whether First Contact's connecting success is currently awaiting or
    /// mid-way through its one-time Network Bootstrap presentation
    /// (`docs/TUI_DESIGN.md`, "Network Bootstrap"). Set exactly once,
    /// alongside `connected`, in [`AppState::step_operation`]'s terminal
    /// branch, the moment connectivity becomes authoritative — never seeded
    /// at startup, never persisted. Cleared by
    /// [`AppState::advance_network_bootstrap`] once every step has shown,
    /// and never set again afterward.
    network_bootstrap_pending: bool,
    /// How many Network Bootstrap steps have been revealed so far, while
    /// [`network_bootstrap_pending`] holds. Indexes into
    /// [`NETWORK_BOOTSTRAP_STEPS`]; reset is unnecessary since the flag
    /// above never becomes `true` a second time.
    ///
    /// [`network_bootstrap_pending`]: AppState::network_bootstrap_pending
    network_bootstrap_step: usize,
    /// Whether the first-launch bootstrap introduction from `slaptijack@`
    /// (issue #173) is currently showing over the rest of the console.
    /// Seeded once before the session's first draw from durably persisted
    /// acknowledgement (see [`console::intro`](super::intro)) and from
    /// `connected` — an already-connected Player never sees it — then
    /// cleared exactly once, by [`Msg::AcknowledgeBootstrapIntro`], and
    /// never set again for the rest of the session.
    bootstrap_intro_visible: bool,
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
            scroll_offsets: HashMap::new(),
            focused_panes: HashMap::new(),
            review_selected: None,
            run_inspector_mode: RunInspectorMode::Timeline,
            source_scroll: 0,
            should_quit: false,
            connected: false,
            network_bootstrap_pending: false,
            network_bootstrap_step: 0,
            bootstrap_intro_visible: false,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this Player has established operator-network connectivity.
    /// `false` for a fresh/no-state installation; becomes `true` and stays
    /// `true` once First Contact has succeeded, independently of the
    /// current view or operation.
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// Whether the console should currently *render* as connected — durable
    /// connectivity (`connected`) may already be `true` the instant First
    /// Contact succeeds, but the console must keep presenting as
    /// pre-connectivity (`LOCAL LOG`, disconnected header, no Signals
    /// content) for the entire Network Bootstrap window, only switching to
    /// the connected presentation as the single final reveal beat once the
    /// modal closes (`docs/TUI_DESIGN.md`, "Network Bootstrap"). Every
    /// rendering call site in `ui.rs` that used to read [`connected`]
    /// directly reads this instead, so they can't disagree about whether
    /// connected content is visible yet.
    ///
    /// [`connected`]: AppState::connected
    pub fn presentation_connected(&self) -> bool {
        self.connected && !self.network_bootstrap_pending
    }

    /// Seeds the connectivity fact from durably persisted state. Only
    /// meant to be called once, before the session's first draw — see
    /// `console::bootstrap_state`. Never used to un-set connectivity.
    pub(crate) fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// Whether First Contact's connecting success is currently awaiting or
    /// mid-way through its one-time Network Bootstrap presentation. See the
    /// field doc comment for the full contract.
    pub fn network_bootstrap_pending(&self) -> bool {
        self.network_bootstrap_pending
    }

    /// Whether Network Bootstrap should advance one tick per timer wakeup,
    /// independent of any player key — the same "presentation paces itself,
    /// not the player" rule as [`AppState::operation_auto_advancing`], but
    /// with no way for the player to pause or leave it, per `docs/
    /// TUI_DESIGN.md`'s "ordinary gameplay/navigation/editing input is
    /// suppressed while it owns the interface." Stays `true` for
    /// [`NETWORK_BOOTSTRAP_LINGER_TICKS`] extra wakeups after the last step
    /// has been revealed — see [`AppState::advance_network_bootstrap`] — so
    /// that final, fully-progressed state is actually shown for more than a
    /// single cadence before the transition completes.
    pub fn network_bootstrap_auto_advancing(&self) -> bool {
        self.network_bootstrap_pending
    }

    /// Advances Network Bootstrap by exactly one tick if
    /// [`network_bootstrap_auto_advancing`] allows it right now, returning
    /// whether anything actually changed (and so a redraw is needed).
    /// Reaching the last entry in [`NETWORK_BOOTSTRAP_STEPS`] does not by
    /// itself clear `network_bootstrap_pending` — the internal tick counter
    /// keeps counting past `NETWORK_BOOTSTRAP_STEPS.len()`, up to
    /// [`NETWORK_BOOTSTRAP_LINGER_TICKS`] further, with
    /// [`network_bootstrap_steps_shown`]/[`network_bootstrap_progress`]
    /// clamping their view of it to the step list's own length so the
    /// fully-revealed, 100%-progress state simply holds on screen across
    /// those extra ticks. Only the tick *after* that lingering window clears
    /// the flag, without revealing anything further — only then does the
    /// console return to its already-routed connected `SIGNALS` view.
    ///
    /// [`network_bootstrap_auto_advancing`]: AppState::network_bootstrap_auto_advancing
    /// [`network_bootstrap_steps_shown`]: AppState::network_bootstrap_steps_shown
    /// [`network_bootstrap_progress`]: AppState::network_bootstrap_progress
    pub fn advance_network_bootstrap(&mut self) -> bool {
        if !self.network_bootstrap_auto_advancing() {
            return false;
        }
        let total_ticks = NETWORK_BOOTSTRAP_STEPS.len() + NETWORK_BOOTSTRAP_LINGER_TICKS;
        if self.network_bootstrap_step < total_ticks {
            self.network_bootstrap_step += 1;
        } else {
            self.network_bootstrap_pending = false;
        }
        true
    }

    /// The Network Bootstrap steps revealed so far, oldest first, for
    /// rendering as a running log. Empty before the first
    /// [`AppState::advance_network_bootstrap`] call. Clamped to
    /// [`NETWORK_BOOTSTRAP_STEPS`]'s own length — the internal tick counter
    /// keeps advancing during the post-completion linger window, but there's
    /// nothing further to reveal by then.
    pub fn network_bootstrap_steps_shown(&self) -> &'static [&'static str] {
        &NETWORK_BOOTSTRAP_STEPS[..self
            .network_bootstrap_step
            .min(NETWORK_BOOTSTRAP_STEPS.len())]
    }

    /// `(revealed, total)` step counts for a progress indicator. `revealed`
    /// is clamped to `total` for the same reason as
    /// [`network_bootstrap_steps_shown`](AppState::network_bootstrap_steps_shown).
    pub fn network_bootstrap_progress(&self) -> (usize, usize) {
        (
            self.network_bootstrap_step
                .min(NETWORK_BOOTSTRAP_STEPS.len()),
            NETWORK_BOOTSTRAP_STEPS.len(),
        )
    }

    /// Whether the first-launch bootstrap introduction is currently
    /// showing. See the field doc comment for the full contract.
    pub fn bootstrap_intro_visible(&self) -> bool {
        self.bootstrap_intro_visible
    }

    /// Seeds whether the bootstrap introduction should show this session.
    /// Only meant to be called once, before the session's first draw — see
    /// `console::bootstrap_state`.
    pub(crate) fn set_bootstrap_intro_visible(&mut self, visible: bool) {
        self.bootstrap_intro_visible = visible;
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

    pub fn controller_source(&self) -> Option<String> {
        self.controller.as_ref().map(ControllerDocument::source)
    }

    /// The current controller document itself, for `ui.rs` to render
    /// through the editor foundation's own widget (issue #92). `None` when
    /// no working set has a controller loaded yet.
    pub(crate) fn controller(&self) -> Option<&ControllerDocument> {
        self.controller.as_ref()
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

    /// The raw stored review-selection index, independent of any particular
    /// operation's `review_points`. Prefer `OperationView::review_selected`
    /// (from [`AppState::operation`]) where a clamped, definitely-valid
    /// index is needed.
    ///
    /// Not yet read by any rendering code — see `OperationView::review_selected`'s
    /// doc comment — but exercised directly by this module's tests.
    #[allow(dead_code)]
    pub fn review_selected(&self) -> Option<usize> {
        self.review_selected
    }

    /// A read-only view of the current deployment, if the player has
    /// deployed a controller at least once this session.
    pub fn operation(&self) -> Option<OperationView<'_>> {
        self.operation.as_ref().map(|op| {
            let starting_budget = crate::simulation::Scenario::first_contact().starting_budget();
            let current = match (op.records.last(), &op.initial_snapshot) {
                (Some(record), _) => OperationSnapshot {
                    drone_position: record.drone_position,
                    map_width: record.map_width,
                    map_height: record.map_height,
                    discovered: record.discovered.clone(),
                    tick: record.tick,
                    budget_remaining: record.budget_remaining,
                },
                // No tick has completed yet: the deploy-time snapshot is
                // still the current state, since the fixed scenario never
                // changes on its own between deploy and the first tick.
                (None, Some(initial)) => initial.clone(),
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
            let finished = op.is_finished();
            let kind = if !finished {
                None
            } else if let Some(err) = &op.error {
                Some(ConclusionKind::ControllerError(err))
            } else {
                match op.records.last().map(|record| record.outcome) {
                    Some(crate::simulation::TickOutcome::Succeeded) => {
                        Some(ConclusionKind::Success)
                    }
                    Some(crate::simulation::TickOutcome::Failed(
                        crate::simulation::FailureReason::BudgetExhausted,
                    )) => Some(ConclusionKind::BudgetExhausted),
                    // A run only finishes via an error or a terminal
                    // `TickOutcome`, so this shouldn't happen in practice;
                    // treat it as "no conclusion yet" rather than
                    // asserting.
                    Some(crate::simulation::TickOutcome::Running) | None => None,
                }
            };
            let conclusion = kind.map(|kind| {
                let hazards_entered = op
                    .records
                    .iter()
                    .flat_map(|record| &record.events)
                    .filter(|event| {
                        matches!(event, crate::simulation::SimEvent::HazardEntered { .. })
                    })
                    .count() as u32;
                OperationConclusion {
                    kind,
                    ticks_executed: op.records.len() as u32,
                    tiles_discovered: current.discovered.len() as u32,
                    hazards_entered,
                    final_budget: current.budget_remaining,
                    run_id: op.run_id,
                }
            });
            let review_points = review_points(&op.initial_snapshot, &op.records, op.error.as_ref());
            let review_selected = self.review_selected.filter(|&i| i < review_points.len());
            OperationView {
                deployed_source: &op.deployed_source,
                run_id: op.run_id,
                records: &op.records,
                paused: op.paused,
                finished,
                error: op.error.as_ref(),
                starting_budget,
                current,
                conclusion,
                review_points,
                review_selected,
                run_inspector_mode: self.run_inspector_mode,
                source_scroll: self.source_scroll,
            }
        })
    }

    /// Whether an operation exists and hasn't finished (succeeded, failed,
    /// or errored) yet — i.e. there is something a redeploy or quit would
    /// abandon.
    fn operation_active(&self) -> bool {
        self.operation.as_ref().is_some_and(|op| !op.is_finished())
    }

    /// The current review chronology's length, or `None` if there's nothing
    /// to navigate: no deployment, a still-live run (chronology navigation
    /// is Review Run-only — live pacing stays on `Space`/`Enter`), or a
    /// deploy-time load failure that never produced a review point. The
    /// gate every `Select*ReviewPoint*` handler in [`Self::apply`] shares.
    fn review_chronology_len(&self) -> Option<usize> {
        let op = self.operation.as_ref()?;
        if !op.is_finished() {
            return None;
        }
        let len = review_points(&op.initial_snapshot, &op.records, op.error.as_ref()).len();
        (len > 0).then_some(len)
    }

    /// Whether the Run Inspector's mode toggle and SOURCE scrolling apply
    /// right now: an operation exists and has finished, regardless of
    /// whether it produced any reviewable chronology point. Deliberately
    /// broader than [`Self::review_chronology_len`] — a zero-tick
    /// deploy-time load failure still has a frozen `deployed_source` worth
    /// inspecting in SOURCE even though it has no chronology to browse in
    /// TIMELINE, so gating SOURCE on chronology existing would make an
    /// already-failed deployment's exact source unreachable.
    fn run_inspector_available(&self) -> bool {
        self.operation.as_ref().is_some_and(Operation::is_finished)
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

    /// The current scroll offset for `pane`, or 0 if it's never been
    /// scrolled.
    pub fn scroll_offset(&self, pane: PaneId) -> u16 {
        self.scroll_offsets.get(&pane).copied().unwrap_or(0)
    }

    /// Bounds `pane`'s stored scroll offset itself against `max`, not just
    /// the value used for a single render. Without this, repeated `Down`
    /// presses can advance the offset toward `MAX_PANE_SCROLL` even once
    /// the content is fully visible, and `Up` then appears to do nothing
    /// until the stored offset drops back below the real maximum.
    pub fn clamp_scroll(&mut self, pane: PaneId, max: u16) {
        if let Some(offset) = self.scroll_offsets.get_mut(&pane) {
            *offset = (*offset).min(max);
        }
    }

    /// Which Run Inspector mode is currently showing.
    pub fn run_inspector_mode(&self) -> RunInspectorMode {
        self.run_inspector_mode
    }

    /// The Run Inspector's current scroll offset into `deployed_source`.
    ///
    /// Not yet read by any rendering code — `ui.rs` reads
    /// `OperationView::source_scroll` instead, the same split
    /// `AppState::review_selected`'s doc comment documents — but exercised
    /// directly by this module's tests.
    #[allow(dead_code)]
    pub fn source_scroll(&self) -> u16 {
        self.source_scroll
    }

    /// Bounds the stored `source_scroll` against `max`, the same role
    /// [`AppState::clamp_scroll`] plays for `scroll_offsets` — without this,
    /// repeated `Down`/`PageDown` presses in `RunInspectorMode::Source`
    /// could advance the offset arbitrarily far past the last visible
    /// source line.
    pub fn clamp_source_scroll(&mut self, max: u16) {
        self.source_scroll = self.source_scroll.min(max);
    }

    /// The pane currently focused in `view`, or its documented default if
    /// focus hasn't been recorded (`docs/TUI_DESIGN.md`, "Pane focus" >
    /// "Default focus").
    pub fn focused_pane(&self, view: View) -> PaneId {
        self.focused_panes
            .get(&view)
            .copied()
            .unwrap_or_else(|| view.default_pane())
    }

    /// Records `view`'s focused pane as `pane`. The only place
    /// `focused_panes` is written, so an invalid view/pane combination can
    /// never be stored: `pane` must belong to `view`'s pane set
    /// ([`View::panes`]).
    fn set_focused_pane(&mut self, view: View, pane: PaneId) {
        debug_assert!(
            view.panes().contains(&pane),
            "{pane:?} is not a pane of {view:?}"
        );
        self.focused_panes.insert(view, pane);
    }

    /// Whether `view`, as currently rendered, presents a real choice between
    /// multiple focusable panes — i.e. whether a focus marker, the `F8`
    /// footer hint, and `F8` itself should do anything right now. The single
    /// source of truth so the marker, the footer, and `F8`'s own routing
    /// can't disagree (`docs/TUI_DESIGN.md`, "F8 -- next pane").
    ///
    /// Operation and After Action share one underlying deployment: both are
    /// single-pane placeholders ("no operation deployed yet" /
    /// "no operation has concluded yet") until `self.operation` exists, the
    /// same condition their own placeholder rendering branches on.
    pub fn focus_movement_available(&self, view: View) -> bool {
        if self.quit_confirmation_pending
            || self.reset_confirmation_pending
            || self.redeploy_confirmation_pending
        {
            return false;
        }
        match view {
            View::Help => false,
            View::Operation | View::AfterAction => self.operation.is_some(),
            View::Signals | View::Target | View::Controller => true,
        }
    }

    /// Moves focus to the next pane in the current view, wrapping around. A
    /// no-op whenever [`AppState::focus_movement_available`] says the
    /// currently rendered surface doesn't present a real multi-pane choice
    /// (Help, or an Operation/After Action placeholder), matching `F8`'s
    /// documented inert behavior there (`docs/TUI_DESIGN.md`, "F8 -- next
    /// pane").
    fn focus_next_pane(&mut self) {
        let view = self.current_view;
        if !self.focus_movement_available(view) {
            return;
        }
        let panes = view.panes();
        let current = self.focused_pane(view);
        let idx = panes
            .iter()
            .position(|&pane| pane == current)
            .expect("focused_pane always returns a pane belonging to its view");
        self.set_focused_pane(view, panes[(idx + 1) % panes.len()]);
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
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
                }
            }
            Msg::OpenHelp => {
                if self.current_view != View::Help {
                    self.help_return_view = Some(self.current_view);
                    self.current_view = View::Help;
                    self.scroll_offsets.insert(PaneId::Help, 0);
                }
            }
            Msg::DismissHelp => {
                if let Some(view) = self.help_return_view.take() {
                    self.current_view = view;
                }
            }
            Msg::AcknowledgeBootstrapIntro => {
                self.bootstrap_intro_visible = false;
            }
            Msg::SelectPreviousSignal => {
                self.selected_signal = self.selected_signal.saturating_sub(1);
            }
            Msg::SelectNextSignal => {
                let last = visible_signals(self.connected).len().saturating_sub(1);
                self.selected_signal = (self.selected_signal + 1).min(last);
            }
            Msg::SelectFirstSignal => {
                self.selected_signal = 0;
            }
            Msg::SelectLastSignal => {
                self.selected_signal = visible_signals(self.connected).len().saturating_sub(1);
            }
            Msg::SelectSignalPageBackward(page) => {
                self.selected_signal = self.selected_signal.saturating_sub(page.max(1));
            }
            Msg::SelectSignalPageForward(page) => {
                let last = visible_signals(self.connected).len().saturating_sub(1);
                self.selected_signal = (self.selected_signal + page.max(1)).min(last);
            }
            Msg::Activate => self.activate(),
            Msg::FocusNextPane => self.focus_next_pane(),
            Msg::ScrollUp => {
                let pane = self.focused_pane(self.current_view);
                if pane_is_scrollable(pane) {
                    let offset = self.scroll_offsets.entry(pane).or_insert(0);
                    *offset = offset.saturating_sub(1);
                }
            }
            Msg::ScrollDown => {
                let pane = self.focused_pane(self.current_view);
                if pane_is_scrollable(pane) {
                    let offset = self.scroll_offsets.entry(pane).or_insert(0);
                    *offset = offset.saturating_add(1).min(MAX_PANE_SCROLL);
                }
            }
            Msg::ScrollPageBackward(page) => {
                let pane = self.focused_pane(self.current_view);
                if pane_is_scrollable(pane) {
                    let page = page.max(1).min(u16::MAX as usize) as u16;
                    let offset = self.scroll_offsets.entry(pane).or_insert(0);
                    *offset = offset.saturating_sub(page);
                }
            }
            Msg::ScrollPageForward(page) => {
                let pane = self.focused_pane(self.current_view);
                if pane_is_scrollable(pane) {
                    let page = page.max(1).min(u16::MAX as usize) as u16;
                    let offset = self.scroll_offsets.entry(pane).or_insert(0);
                    *offset = offset.saturating_add(page).min(MAX_PANE_SCROLL);
                }
            }
            Msg::JumpScrollStart => {
                let pane = self.focused_pane(self.current_view);
                if pane_is_scrollable(pane) {
                    self.scroll_offsets.insert(pane, 0);
                }
            }
            Msg::JumpScrollEnd => {
                let pane = self.focused_pane(self.current_view);
                if pane_is_scrollable(pane) {
                    self.scroll_offsets.insert(pane, MAX_PANE_SCROLL);
                }
            }
            Msg::SelectPreviousReviewPoint => {
                if let Some(len) = self.review_chronology_len() {
                    let current = self.review_selected.unwrap_or(len - 1);
                    self.review_selected = Some(current.saturating_sub(1));
                }
            }
            Msg::SelectNextReviewPoint => {
                if let Some(len) = self.review_chronology_len() {
                    let current = self.review_selected.unwrap_or(len - 1);
                    self.review_selected = Some((current + 1).min(len - 1));
                }
            }
            Msg::SelectFirstReviewPoint => {
                if self.review_chronology_len().is_some() {
                    self.review_selected = Some(0);
                }
            }
            Msg::SelectLastReviewPoint => {
                if let Some(len) = self.review_chronology_len() {
                    self.review_selected = Some(len - 1);
                }
            }
            Msg::SelectReviewPointPageBackward(page) => {
                if let Some(len) = self.review_chronology_len() {
                    let current = self.review_selected.unwrap_or(len - 1);
                    self.review_selected = Some(current.saturating_sub(page.max(1)));
                }
            }
            Msg::SelectReviewPointPageForward(page) => {
                if let Some(len) = self.review_chronology_len() {
                    let current = self.review_selected.unwrap_or(len - 1);
                    self.review_selected = Some((current + page.max(1)).min(len - 1));
                }
            }
            Msg::ToggleRunInspectorMode => {
                if self.run_inspector_available() {
                    self.run_inspector_mode = match self.run_inspector_mode {
                        RunInspectorMode::Timeline => RunInspectorMode::Source,
                        RunInspectorMode::Source => RunInspectorMode::Timeline,
                    };
                }
            }
            Msg::ScrollSourceUp => {
                if self.run_inspector_available() {
                    self.source_scroll = self.source_scroll.saturating_sub(1);
                }
            }
            Msg::ScrollSourceDown => {
                if self.run_inspector_available() {
                    // No upper bound here beyond `u16`'s own range: unlike
                    // `ScrollUp`/`ScrollDown`'s shared `MAX_PANE_SCROLL`
                    // cap (sized for authored Help/Report content),
                    // `deployed_source` is arbitrary-length player Lua and
                    // must stay reachable to its true end however long it
                    // is. `console::should_redraw` re-clamps this down to
                    // the real content height via `ui::review_source_max_scroll`
                    // right after this runs, the same way it already does
                    // for `ScrollUp`/`ScrollDown`.
                    self.source_scroll = self.source_scroll.saturating_add(1);
                }
            }
            Msg::ScrollSourcePageBackward(page) => {
                if self.run_inspector_available() {
                    let page = page.max(1).min(u16::MAX as usize) as u16;
                    self.source_scroll = self.source_scroll.saturating_sub(page);
                }
            }
            Msg::ScrollSourcePageForward(page) => {
                if self.run_inspector_available() {
                    let page = page.max(1).min(u16::MAX as usize) as u16;
                    self.source_scroll = self.source_scroll.saturating_add(page);
                }
            }
            Msg::JumpSourceStart => {
                if self.run_inspector_available() {
                    self.source_scroll = 0;
                }
            }
            Msg::JumpSourceEnd => {
                if self.run_inspector_available() {
                    // A large sentinel, not the real end (this module has
                    // no notion of pane width/height to compute it) — see
                    // `ScrollSourceDown`'s comment above for the same
                    // reasoning, and `Msg::JumpSourceEnd`'s doc comment for
                    // where it gets clamped down to the true end.
                    self.source_scroll = u16::MAX;
                }
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
                    self.validation = match lua_controller::validate(&source) {
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
        let run_id = self.next_run_id;
        self.next_run_id += 1;

        let operation = match LiveOperation::deploy(&source) {
            Ok(live) => {
                // Captured now, before any tick executes, so later ticks
                // (and the eventual terminal outcome) can never retroactively
                // change what the player is shown as "already known at
                // deployment" — see `Operation::initial_snapshot`.
                let initial_snapshot = Some(observe_snapshot(&live));
                Operation {
                    live: Some(live),
                    deployed_source: source,
                    run_id,
                    records: Vec::new(),
                    initial_snapshot,
                    paused: false,
                    error: None,
                }
            }
            Err(err) => Operation {
                live: None,
                deployed_source: source,
                run_id,
                records: Vec::new(),
                initial_snapshot: None,
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
        // A fresh deploy always starts a new review chronology: any prior
        // run's selection must not leak into this one (a live run has
        // nothing reviewable yet; a synchronous load failure jumps straight
        // to its own terminal boundary, mirroring the AfterAction focus
        // reset just below).
        self.review_selected = if operation.is_finished() {
            review_points(
                &operation.initial_snapshot,
                &operation.records,
                operation.error.as_ref(),
            )
            .len()
            .checked_sub(1)
        } else {
            None
        };
        // A fresh deploy always shows the new run's Run Inspector starting
        // in TIMELINE, regardless of what mode/scroll the previous run was
        // left in — `docs/TUI_DESIGN.md`, "Review Run".
        self.run_inspector_mode = RunInspectorMode::Timeline;
        self.source_scroll = 0;
        if operation.is_finished() {
            self.set_focused_pane(View::Operation, PaneId::OperationTelemetry);
        }
        self.operation = Some(operation);
        // A fresh report may be shorter than whatever was last scrolled to,
        // so start it at the top rather than carrying over an offset from a
        // previous run's report — same reasoning as `OpenHelp`'s reset.
        self.scroll_offsets.insert(PaneId::Report, 0);
        // A fresh terminal result always focuses the report pane, so the
        // outcome hierarchy stays primary regardless of what was last
        // focused in After Action (`docs/TUI_DESIGN.md`, "Focus
        // persistence").
        self.set_focused_pane(View::AfterAction, PaneId::Report);
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
            // First Contact's success is the one durable progression fact
            // this console remembers across relaunches (`docs/TUI_DESIGN.md`,
            // "State and information rules") — recorded here, the moment
            // the outcome is authoritatively determined, not when any later
            // transition presentation finishes. Once set it never reverts:
            // a later failed run mustn't undo an already-established
            // connection.
            let just_connected = !self.connected
                && matches!(
                    op.records.last().map(|record| record.outcome),
                    Some(crate::simulation::TickOutcome::Succeeded)
                );
            if just_connected {
                self.connected = true;
                self.network_bootstrap_pending = true;
            }
            // The run's chronology is now frozen, so its terminal review
            // point is already known — select it now, before the player
            // has even navigated to Review Run, the same way the report
            // focus below is set proactively rather than on first render.
            let terminal_review_point =
                review_points(&op.initial_snapshot, &op.records, op.error.as_ref())
                    .len()
                    .checked_sub(1);
            // The connecting success plays Network Bootstrap in place of
            // the automatic landing on After Action, and lands on
            // connected Signals rather than After Action once it's owed
            // (`docs/TUI_DESIGN.md`, "Network Bootstrap"). After Action and
            // Review Run remain reachable afterward exactly as before —
            // nothing below this branches on `just_connected`.
            self.current_view = if just_connected {
                View::Signals
            } else {
                View::AfterAction
            };
            self.scroll_offsets.insert(PaneId::Report, 0);
            self.set_focused_pane(View::AfterAction, PaneId::Report);
            self.review_selected = terminal_review_point;
            self.run_inspector_mode = RunInspectorMode::Timeline;
            self.source_scroll = 0;
            self.set_focused_pane(View::Operation, PaneId::OperationTelemetry);
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
                let selected = visible_signals(self.connected)
                    .get(self.selected_signal)
                    .copied();
                if selected.is_some_and(|signal| signal.is_actionable()) {
                    self.target_known = true;
                    self.current_view = View::Target;
                }
            }
            View::Target => {
                if self.working_set != Some(WorkingSet::FirstContact) {
                    self.working_set = Some(WorkingSet::FirstContact);
                    self.controller =
                        Some(ControllerDocument::new(super::intel::STARTER_CONTROLLER));
                }
                self.current_view = View::Controller;
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
    fn every_view_resolves_a_focused_pane_that_belongs_to_it() {
        let state = AppState::new();

        for view in [
            View::Help,
            View::Signals,
            View::Target,
            View::Controller,
            View::Operation,
            View::AfterAction,
        ] {
            let pane = state.focused_pane(view);
            assert!(
                view.panes().contains(&pane),
                "{pane:?} is not a pane of {view:?}"
            );
        }
    }

    #[test]
    fn default_focused_panes_match_the_design_contract() {
        let state = AppState::new();

        assert_eq!(state.focused_pane(View::Help), PaneId::Help);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SignalsList);
        assert_eq!(state.focused_pane(View::Target), PaneId::TargetIntelligence);
        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::ControllerSource
        );
        assert_eq!(state.focused_pane(View::Operation), PaneId::Satellite);
        assert_eq!(state.focused_pane(View::AfterAction), PaneId::Report);
    }

    #[test]
    fn focus_persists_independently_per_view_across_navigation() {
        let mut state = AppState::new();
        state.set_focused_pane(View::Controller, PaneId::LuaFieldReference);
        state.set_focused_pane(View::Signals, PaneId::SelectedSignal);

        state.apply(Msg::Navigate(View::AfterAction));
        state.apply(Msg::Navigate(View::Signals));
        state.apply(Msg::Navigate(View::AfterAction));

        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::LuaFieldReference
        );
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);
        assert_eq!(state.focused_pane(View::AfterAction), PaneId::Report);
    }

    #[test]
    fn opening_and_dismissing_help_preserves_the_underlying_views_focus() {
        let mut state = AppState::new();
        state.set_focused_pane(View::Signals, PaneId::SelectedSignal);

        state.apply(Msg::OpenHelp);
        assert_eq!(state.focused_pane(View::Help), PaneId::Help);

        state.apply(Msg::DismissHelp);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);
    }

    #[test]
    #[should_panic(expected = "is not a pane of")]
    fn set_focused_pane_panics_on_an_invalid_view_pane_combination() {
        let mut state = AppState::new();
        state.set_focused_pane(View::Signals, PaneId::Report);
    }

    #[test]
    fn signal_selection_does_not_move_past_the_ends_of_the_list() {
        let mut state = AppState::new();

        state.apply(Msg::SelectPreviousSignal);
        assert_eq!(state.selected_signal(), 0);

        for _ in 0..visible_signals(false).len() + 2 {
            state.apply(Msg::SelectNextSignal);
        }
        assert_eq!(state.selected_signal(), visible_signals(false).len() - 1);
    }

    #[test]
    fn home_and_end_jump_signal_selection_to_the_first_and_last_signal() {
        let mut state = AppState::new();
        let last = visible_signals(false).len() - 1;

        state.apply(Msg::SelectLastSignal);
        assert_eq!(state.selected_signal(), last);

        state.apply(Msg::SelectFirstSignal);
        assert_eq!(state.selected_signal(), 0);

        // Inert at the boundary they already sit at.
        state.apply(Msg::SelectFirstSignal);
        assert_eq!(state.selected_signal(), 0);
    }

    #[test]
    fn signal_page_movement_clamps_at_the_ends_without_wrapping() {
        let mut state = AppState::new();
        let last = visible_signals(false).len() - 1;

        state.apply(Msg::SelectSignalPageForward(2));
        assert_eq!(state.selected_signal(), 2.min(last));

        state.apply(Msg::SelectSignalPageForward(last + 10));
        assert_eq!(state.selected_signal(), last);

        state.apply(Msg::SelectSignalPageBackward(2));
        assert_eq!(state.selected_signal(), last.saturating_sub(2));

        state.apply(Msg::SelectSignalPageBackward(last + 10));
        assert_eq!(state.selected_signal(), 0);
    }

    #[test]
    fn signal_page_movement_treats_a_zero_page_size_as_one() {
        let mut state = AppState::new();

        state.apply(Msg::SelectSignalPageForward(0));
        assert_eq!(
            state.selected_signal(),
            1.min(visible_signals(false).len() - 1)
        );

        state.apply(Msg::SelectSignalPageBackward(0));
        assert_eq!(state.selected_signal(), 0);
    }

    #[test]
    fn connected_signal_selection_clamps_against_the_full_authored_list() {
        use super::super::intel::authored_signals;

        let mut state = AppState::new();
        state.set_connected(true);
        let last = authored_signals().len() - 1;
        assert!(
            last > visible_signals(false).len() - 1,
            "the connected list must be a strict superset of the disconnected one"
        );

        state.apply(Msg::SelectLastSignal);
        assert_eq!(state.selected_signal(), last);

        state.apply(Msg::SelectFirstSignal);
        assert_eq!(state.selected_signal(), 0);
    }

    #[test]
    fn activating_the_actionable_signal_marks_target_known_and_opens_it() {
        let mut state = AppState::new();
        let actionable = visible_signals(false)
            .iter()
            .position(|signal| signal.is_actionable())
            .expect("exactly one signal is actionable");
        for _ in 0..actionable {
            state.apply(Msg::SelectNextSignal);
        }

        state.apply(Msg::Activate);

        assert!(state.target_known);
        assert_eq!(state.current_view(), View::Target);
    }

    #[test]
    fn activating_a_non_actionable_signal_is_a_no_op() {
        let mut state = AppState::new();
        let non_actionable = visible_signals(false)
            .iter()
            .position(|signal| !signal.is_actionable())
            .expect("at least one signal is non-actionable");
        for _ in 0..non_actionable {
            state.apply(Msg::SelectNextSignal);
        }

        state.apply(Msg::Activate);

        assert!(!state.target_known);
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
            Some(super::super::intel::STARTER_CONTROLLER.to_string())
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
            Some(format!("{}!", super::super::intel::STARTER_CONTROLLER))
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

        assert_eq!(state.controller_source(), Some(source_before.clone()));
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

        state.apply(Msg::EditController(EditOp::MoveLeft(false)));
        state.apply(Msg::EditController(EditOp::MoveDown(false)));

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
        // are both no-ops (ControllerDocument::apply reports no mutation),
        // so neither should invalidate a result that's still accurate.
        state.apply(Msg::EditController(EditOp::DeleteForward));
        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "DeleteForward at the end of the document didn't change anything"
        );

        state.apply(Msg::EditController(EditOp::PageUp(false)));
        state.apply(Msg::EditController(EditOp::PageUp(false)));
        state.apply(Msg::EditController(EditOp::MoveLineStart(false)));
        state.apply(Msg::EditController(EditOp::Backspace));
        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "Backspace at the start of the document didn't change anything either"
        );
    }

    #[test]
    fn creating_a_selection_without_mutating_preserves_a_prior_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);

        state.apply(Msg::EditController(EditOp::MoveLineStart(true)));
        state.apply(Msg::EditController(EditOp::SelectAll));

        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "creating a selection doesn't change the source, so a prior validation is still accurate"
        );
    }

    #[test]
    fn undo_that_actually_changes_content_invalidates_a_prior_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);

        // Appending 'x' at the end of the starter breaks its syntax, but
        // that doesn't matter here — either result (`Valid` or `Invalid`)
        // is something a subsequent undo must invalidate back to
        // `Unchecked`, since undo is itself a content-changing edit.
        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::ValidateController);
        assert_ne!(
            state.validation(),
            &Validation::Unchecked,
            "sanity: validating the edited script must produce a result to invalidate"
        );

        state.apply(Msg::EditController(EditOp::Undo));

        assert_eq!(
            state.validation(),
            &Validation::Unchecked,
            "undo restores the pre-'x' source, which is still a real content change"
        );
    }

    #[test]
    fn undo_with_no_history_preserves_a_prior_validation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        assert_eq!(state.validation(), &Validation::Valid);

        state.apply(Msg::EditController(EditOp::Undo));

        assert_eq!(
            state.validation(),
            &Validation::Valid,
            "there's nothing to undo yet, so nothing changed"
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
            Some(super::super::intel::STARTER_CONTROLLER.to_string())
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
            Some(super::super::intel::STARTER_CONTROLLER.to_string())
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
    fn undoing_back_to_the_starter_text_clears_modified_and_needs_no_reset_confirmation() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        assert!(!state.controller_modified());

        state.apply(Msg::EditController(EditOp::Insert('x')));
        assert!(state.controller_modified());

        state.apply(Msg::EditController(EditOp::Undo));

        assert!(
            !state.controller_modified(),
            "undoing back to exactly the starter text should clear \"modified\", \
             since it is a content comparison rather than an undo-depth flag"
        );
        assert_eq!(
            state.validation(),
            &Validation::Unchecked,
            "the undo did change content, so any prior validation is invalidated"
        );

        state.apply(Msg::RequestResetController);

        assert!(!state.reset_confirmation_pending());
        assert_eq!(
            state.controller_source(),
            Some(super::super::intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn undoing_partway_back_still_leaves_the_controller_modified() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);

        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::EditController(EditOp::Insert('y')));
        state.apply(Msg::EditController(EditOp::Undo));

        assert!(
            state.controller_modified(),
            "one undo removed only the second insert; the first still \
             differs from the starter text"
        );
    }

    #[test]
    fn focus_next_pane_advances_through_a_views_panes_and_wraps_around() {
        let mut state = AppState::new();
        assert_eq!(state.focused_pane(View::Signals), PaneId::SignalsList);

        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);

        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SignalsList);
    }

    #[test]
    fn focus_next_pane_is_inert_in_help() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        assert_eq!(state.focused_pane(View::Help), PaneId::Help);

        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Help), PaneId::Help);
    }

    #[test]
    fn navigating_away_and_back_preserves_moved_focus() {
        let mut state = AppState::new();
        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);

        state.apply(Msg::Navigate(View::AfterAction));
        state.apply(Msg::Navigate(View::Signals));

        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);
    }

    #[test]
    fn focus_movement_available_is_false_only_for_help() {
        let state = AppState::new();
        assert!(!state.focus_movement_available(View::Help));
        assert!(state.focus_movement_available(View::Signals));
        assert!(state.focus_movement_available(View::Target));
        assert!(state.focus_movement_available(View::Controller));
    }

    #[test]
    fn focus_movement_available_is_false_for_operation_and_after_action_before_any_deploy() {
        let state = AppState::new();
        assert!(!state.focus_movement_available(View::Operation));
        assert!(!state.focus_movement_available(View::AfterAction));
    }

    #[test]
    fn focus_movement_available_is_true_for_operation_and_after_action_once_deployed() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::RequestDeploy);

        assert!(state.focus_movement_available(View::Operation));
        assert!(state.focus_movement_available(View::AfterAction));
    }

    #[test]
    fn focus_movement_available_is_false_while_reset_confirmation_is_pending() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));

        state.apply(Msg::RequestResetController);
        assert!(!state.focus_movement_available(View::Controller));
    }

    #[test]
    fn focus_movement_available_is_false_while_quit_confirmation_is_pending() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::Insert('x')));

        state.apply(Msg::RequestQuit);
        assert!(!state.focus_movement_available(View::Signals));
    }

    #[test]
    fn focus_movement_available_is_false_while_redeploy_confirmation_is_pending() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);

        state.apply(Msg::RequestDeploy);
        state.apply(Msg::RequestDeploy); // an active run already exists: pending confirmation
        assert!(!state.focus_movement_available(View::Operation));
    }

    #[test]
    fn f8_on_the_undeployed_operation_placeholder_does_not_disturb_the_first_deployments_focus() {
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::Navigate(View::Operation));

        // A stray F8 press before anything is deployed must stay inert.
        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Operation), PaneId::Satellite);

        state.apply(Msg::RequestDeploy);
        assert_eq!(state.focused_pane(View::Operation), PaneId::Satellite);
    }

    #[test]
    fn help_scroll_moves_up_and_down_and_saturates_at_zero() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        state.apply(Msg::ScrollDown);
        state.apply(Msg::ScrollDown);
        assert_eq!(state.scroll_offset(PaneId::Help), 2);

        state.apply(Msg::ScrollUp);
        state.apply(Msg::ScrollUp);
        state.apply(Msg::ScrollUp);
        assert_eq!(state.scroll_offset(PaneId::Help), 0);
    }

    #[test]
    fn help_scroll_is_capped_and_recoverable() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        for _ in 0..1000 {
            state.apply(Msg::ScrollDown);
        }
        let capped = state.scroll_offset(PaneId::Help);
        assert!(capped < 1000);

        for _ in 0..capped {
            state.apply(Msg::ScrollUp);
        }
        assert_eq!(state.scroll_offset(PaneId::Help), 0);
    }

    #[test]
    fn scroll_offsets_are_independent_per_pane() {
        let mut state = AppState::new();

        state.apply(Msg::OpenHelp);
        state.apply(Msg::ScrollDown);
        state.apply(Msg::ScrollDown);
        state.apply(Msg::DismissHelp);

        state.set_focused_pane(View::AfterAction, PaneId::Report);
        state.apply(Msg::Navigate(View::AfterAction));
        state.apply(Msg::ScrollDown);

        assert_eq!(state.scroll_offset(PaneId::Help), 2);
        assert_eq!(state.scroll_offset(PaneId::Report), 1);
    }

    #[test]
    fn scrolling_a_non_scrollable_pane_is_a_no_op() {
        let mut state = AppState::new();
        state.set_focused_pane(View::AfterAction, PaneId::FinalFrame);
        state.apply(Msg::Navigate(View::AfterAction));

        state.apply(Msg::ScrollDown);
        state.apply(Msg::ScrollDown);

        assert_eq!(state.scroll_offset(PaneId::FinalFrame), 0);
        assert!(!state.scroll_offsets.contains_key(&PaneId::FinalFrame));
    }

    #[test]
    fn help_page_movement_moves_by_the_carried_page_size_and_clamps_at_zero() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);

        state.apply(Msg::ScrollPageForward(5));
        assert_eq!(state.scroll_offset(PaneId::Help), 5);

        state.apply(Msg::ScrollPageBackward(3));
        assert_eq!(state.scroll_offset(PaneId::Help), 2);

        state.apply(Msg::ScrollPageBackward(100));
        assert_eq!(
            state.scroll_offset(PaneId::Help),
            0,
            "paging backward past the start clamps to 0 rather than wrapping"
        );
    }

    #[test]
    fn help_page_movement_treats_a_zero_page_size_as_one() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);

        state.apply(Msg::ScrollPageForward(0));
        assert_eq!(state.scroll_offset(PaneId::Help), 1);

        state.apply(Msg::ScrollPageBackward(0));
        assert_eq!(state.scroll_offset(PaneId::Help), 0);
    }

    #[test]
    fn help_page_forward_is_capped_by_max_pane_scroll() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);

        state.apply(Msg::ScrollPageForward(10_000));
        assert!(state.scroll_offset(PaneId::Help) <= 500);
    }

    #[test]
    fn home_and_end_jump_the_scroll_offset_to_the_start_and_sentinel_end() {
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        state.apply(Msg::ScrollDown);
        state.apply(Msg::ScrollDown);

        state.apply(Msg::JumpScrollStart);
        assert_eq!(state.scroll_offset(PaneId::Help), 0);

        state.apply(Msg::JumpScrollEnd);
        assert_eq!(
            state.scroll_offset(PaneId::Help),
            500,
            "JumpScrollEnd sets the MAX_PANE_SCROLL sentinel; console::mod \
             clamps it down to the real content height after applying"
        );
    }

    #[test]
    fn report_page_and_home_end_movement_is_independent_of_help() {
        let mut state = AppState::new();

        state.set_focused_pane(View::AfterAction, PaneId::Report);
        state.apply(Msg::Navigate(View::AfterAction));
        state.apply(Msg::ScrollPageForward(4));
        assert_eq!(state.scroll_offset(PaneId::Report), 4);
        assert_eq!(state.scroll_offset(PaneId::Help), 0);

        state.apply(Msg::JumpScrollEnd);
        assert_eq!(state.scroll_offset(PaneId::Report), 500);
        assert_eq!(state.scroll_offset(PaneId::Help), 0);
    }

    #[test]
    fn page_and_home_end_movement_on_a_non_scrollable_pane_is_a_no_op() {
        let mut state = AppState::new();
        state.set_focused_pane(View::AfterAction, PaneId::FinalFrame);
        state.apply(Msg::Navigate(View::AfterAction));

        state.apply(Msg::ScrollPageForward(5));
        state.apply(Msg::JumpScrollEnd);

        assert_eq!(state.scroll_offset(PaneId::FinalFrame), 0);
        assert!(!state.scroll_offsets.contains_key(&PaneId::FinalFrame));
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
    fn editing_the_controller_after_deploying_does_not_change_the_deployed_source() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));

        state.apply(Msg::RequestDeploy);
        let deployed = state
            .operation()
            .expect("a deploy just happened")
            .deployed_source
            .to_string();
        assert_eq!(deployed, ALWAYS_ERRORS);

        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::EditController(EditOp::Insert('y')));

        assert_eq!(
            state.controller_source().unwrap(),
            format!("{ALWAYS_ERRORS}xy"),
            "the working copy did change"
        );
        assert_eq!(
            state.operation().unwrap().deployed_source,
            deployed,
            "the frozen deploy snapshot must be unreachable from later edits \
             to the separate, still-mutable controller document"
        );

        // The provenance guarantee must also hold once the run has actually
        // finished and the player is looking at Review Run — reached here
        // by navigating back to `View::Operation` once `finished`, exactly
        // as `F5` does.
        state.advance_running_operation();
        assert!(state.operation().unwrap().finished);
        assert_eq!(state.operation().unwrap().deployed_source, deployed);

        state.apply(Msg::Navigate(View::Operation));
        assert_eq!(state.current_view(), View::Operation);
        assert_eq!(state.operation().unwrap().deployed_source, deployed);
    }

    #[test]
    fn undoing_after_deploy_does_not_change_the_deployed_source() {
        let mut state = working_state();

        state.apply(Msg::RequestDeploy);
        let deployed = state
            .operation()
            .expect("a deploy just happened")
            .deployed_source
            .to_string();

        state.apply(Msg::EditController(EditOp::Insert('x')));
        state.apply(Msg::EditController(EditOp::Undo));

        assert_eq!(
            state.controller_source().unwrap(),
            deployed,
            "undo restored the working copy to the deployed text"
        );
        assert_eq!(
            state.operation().unwrap().deployed_source,
            deployed,
            "undo/redo on the working copy must never reach the frozen \
             deploy snapshot either"
        );
    }

    #[test]
    fn run_inspector_starts_in_timeline_mode_once_a_run_finishes() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}

        assert!(state.operation().unwrap().finished);
        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Timeline);
        assert_eq!(state.source_scroll(), 0);
    }

    #[test]
    fn toggle_run_inspector_mode_is_a_noop_before_a_run_finishes() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        assert!(!state.operation().unwrap().finished);

        state.apply(Msg::ToggleRunInspectorMode);

        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Timeline);
    }

    #[test]
    fn toggle_run_inspector_mode_is_a_noop_with_no_deployment_at_all() {
        let mut state = working_state();

        state.apply(Msg::ToggleRunInspectorMode);

        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Timeline);
    }

    #[test]
    fn toggle_run_inspector_mode_works_for_a_zero_tick_deploy_failure() {
        // A load failure never produces a review point, so TIMELINE has
        // nothing to browse — but SOURCE must still reach the exact source
        // that failed to load, not treat this deployment as unreachable.
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));
        state.apply(Msg::RequestDeploy);
        let op = state.operation().expect("a deploy just happened");
        assert!(op.finished);
        assert!(op.review_points.is_empty());

        state.apply(Msg::ToggleRunInspectorMode);

        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Source);
    }

    #[test]
    fn toggle_run_inspector_mode_switches_between_timeline_and_source_once_finished() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}
        assert!(state.operation().unwrap().finished);

        state.apply(Msg::ToggleRunInspectorMode);
        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Source);

        state.apply(Msg::ToggleRunInspectorMode);
        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Timeline);
    }

    #[test]
    fn source_scroll_messages_move_the_scroll_offset_only_once_a_run_has_finished() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);

        // Not finished yet: every source-scroll `Msg` is a no-op.
        state.apply(Msg::ScrollSourceDown);
        assert_eq!(state.source_scroll(), 0);

        while state.advance_running_operation() {}
        assert!(state.operation().unwrap().finished);

        state.apply(Msg::ScrollSourceDown);
        assert_eq!(state.source_scroll(), 1);
        state.apply(Msg::ScrollSourceDown);
        assert_eq!(state.source_scroll(), 2);
        state.apply(Msg::ScrollSourceUp);
        assert_eq!(state.source_scroll(), 1);
        // `Up` never goes negative.
        state.apply(Msg::ScrollSourceUp);
        state.apply(Msg::ScrollSourceUp);
        assert_eq!(state.source_scroll(), 0);

        state.apply(Msg::ScrollSourcePageForward(5));
        assert_eq!(state.source_scroll(), 5);
        state.apply(Msg::ScrollSourcePageBackward(2));
        assert_eq!(state.source_scroll(), 3);

        state.apply(Msg::JumpSourceEnd);
        assert_eq!(state.source_scroll(), u16::MAX);
        state.apply(Msg::JumpSourceStart);
        assert_eq!(state.source_scroll(), 0);
    }

    #[test]
    fn redeploying_resets_run_inspector_mode_and_source_scroll() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}
        assert!(state.operation().unwrap().finished);
        state.apply(Msg::ToggleRunInspectorMode);
        state.apply(Msg::ScrollSourceDown);
        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Source);
        assert_eq!(state.source_scroll(), 1);

        state.apply(Msg::ConfirmDeploy);

        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Timeline);
        assert_eq!(state.source_scroll(), 0);
    }

    #[test]
    fn a_run_finishing_over_several_ticks_resets_run_inspector_mode_and_source_scroll() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        assert!(!state.operation().unwrap().finished);

        // Drive the run to completion via `step_operation`'s terminal
        // branch (`ROUTE_TO_UPLINK` finishes in a bounded number of ticks),
        // rather than `deploy`'s own reset, to prove both reset sites work.
        while !state.operation().unwrap().finished {
            state.advance_running_operation();
        }

        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Timeline);
        assert_eq!(state.source_scroll(), 0);
    }

    #[test]
    fn navigating_away_and_back_preserves_run_inspector_mode_and_source_scroll() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}
        assert!(state.operation().unwrap().finished);
        state.apply(Msg::ToggleRunInspectorMode);
        state.apply(Msg::ScrollSourceDown);
        state.apply(Msg::ScrollSourceDown);

        state.apply(Msg::Navigate(View::Controller));
        state.apply(Msg::Navigate(View::Operation));

        assert_eq!(state.run_inspector_mode(), RunInspectorMode::Source);
        assert_eq!(state.source_scroll(), 2);
    }

    #[test]
    fn deploying_a_script_with_a_load_error_surfaces_it_without_a_live_run() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        let op = state.operation().expect("deploy always records an outcome");
        assert!(op.finished);
        assert!(matches!(op.error, Some(ControllerError::ScriptInvalid(_))));
        // No live run was ever shown — go straight to the After Action
        // report rather than parking on an empty Operation view.
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn deploying_a_script_with_a_load_error_has_no_initial_snapshot() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        let op = state
            .operation
            .as_ref()
            .expect("deploy always records an outcome");
        assert!(
            op.initial_snapshot.is_none(),
            "a deployment that never started a live simulation must not \
             invent an initial execution snapshot"
        );
    }

    #[test]
    fn deploying_the_starter_controller_retains_a_pretick_initial_snapshot() {
        let mut state = working_state();

        state.apply(Msg::RequestDeploy);

        let starting_budget = crate::simulation::Scenario::first_contact().starting_budget();
        let op = state.operation.as_ref().expect("a deploy just happened");
        let initial = op
            .initial_snapshot
            .as_ref()
            .expect("a successfully loaded deployment retains a pre-tick snapshot");
        let scenario = crate::simulation::Scenario::first_contact();
        assert_eq!(initial.tick, 0);
        assert_eq!(initial.budget_remaining, starting_budget);
        assert_eq!(initial.map_width, scenario.map().width());
        assert_eq!(initial.map_height, scenario.map().height());
        assert_eq!(initial.drone_position, scenario.drone_start());
    }

    #[test]
    fn the_initial_snapshot_stays_unchanged_after_ticks_complete() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));

        state.apply(Msg::RequestDeploy);
        let initial = state
            .operation
            .as_ref()
            .expect("a deploy just happened")
            .initial_snapshot
            .clone()
            .expect("a successful deploy retains a pre-tick snapshot");

        while state.advance_running_operation() {}

        assert!(state.operation().unwrap().finished);
        assert_eq!(
            state.operation.as_ref().unwrap().initial_snapshot,
            Some(initial),
            "the pre-tick snapshot must not change once later ticks complete"
        );
    }

    /// Two valid moves (both onto floor tiles adjacent to the fixed First
    /// Contact drone start), then an unconditional error — used to exercise
    /// a controller failure that happens after some ticks have already
    /// completed, as opposed to `ALWAYS_ERRORS`'s first-tick failure.
    const FAILS_AFTER_TWO_TICKS: &str = r#"
        local step = 0
        function on_tick(observation)
            step = step + 1
            if step > 2 then error('boom') end
            return "north"
        end
    "#;

    #[test]
    fn a_successful_run_projects_initial_and_every_completed_tick_with_no_failure_point() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        while state.advance_running_operation() {}

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert_eq!(op.review_points.len(), 1 + op.records.len());
        assert!(matches!(op.review_points[0].kind, ReviewPointKind::Initial));
        for (point, record) in op.review_points[1..].iter().zip(op.records.iter()) {
            match point.kind {
                ReviewPointKind::Tick(r) => assert_eq!(r, record),
                other => panic!("expected a Tick point, got {other:?}"),
            }
        }
        let last = op.review_points.last().unwrap();
        match last.kind {
            ReviewPointKind::Tick(record) => {
                assert_eq!(record.outcome, crate::simulation::TickOutcome::Succeeded)
            }
            other => panic!("expected the terminal point to be a completed tick, got {other:?}"),
        }

        // First Contact's route to the uplink used here also passes through
        // its one hazard tile, so this run doubles as hazard-entry coverage:
        // the corresponding review point's structured events include it.
        assert!(op.review_points.iter().any(|point| {
            matches!(point.kind, ReviewPointKind::Tick(record)
            if record.events.iter().any(|event| matches!(
                event,
                crate::simulation::SimEvent::HazardEntered { .. }
            )))
        }));
    }

    #[test]
    fn a_budget_exhausted_run_projects_no_synthetic_terminal_point() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_WAITS));
        state.apply(Msg::RequestDeploy);

        while state.advance_running_operation() {}

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert_eq!(op.review_points.len(), 1 + op.records.len());
        let last = op.review_points.last().unwrap();
        match last.kind {
            ReviewPointKind::Tick(record) => assert_eq!(
                record.outcome,
                crate::simulation::TickOutcome::Failed(
                    crate::simulation::FailureReason::BudgetExhausted
                )
            ),
            other => panic!("expected the terminal point to be a completed tick, got {other:?}"),
        }
    }

    #[test]
    fn a_controller_failure_after_completed_ticks_appends_a_terminal_failure_point() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(FAILS_AFTER_TWO_TICKS));
        state.apply(Msg::RequestDeploy);

        while state.advance_running_operation() {}

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert_eq!(
            op.records.len(),
            2,
            "exactly two ticks completed before the failure"
        );
        assert_eq!(
            op.review_points.len(),
            1 + 2 + 1,
            "INITIAL, two ticks, one failure point"
        );

        let last_tick = op.review_points[2].clone();
        match last_tick.kind {
            ReviewPointKind::Tick(record) => assert_eq!(record, &op.records[1]),
            other => panic!("expected a Tick point, got {other:?}"),
        }

        let failure = op.review_points.last().unwrap();
        assert!(matches!(
            failure.kind,
            ReviewPointKind::TerminalFailure(ControllerError::CallbackFailed(_))
        ));
        assert_eq!(
            failure.snapshot, last_tick.snapshot,
            "the failure boundary carries the last completed tick's position and budget, \
             not a fabricated one"
        );
        assert!(
            failure.newly_discovered.is_empty(),
            "no tick executed for the failure boundary, so nothing new was discovered"
        );
    }

    #[test]
    fn a_first_tick_controller_failure_appends_a_terminal_failure_point_without_a_tick() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);

        state.advance_running_operation();

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert!(
            op.records.is_empty(),
            "no tick completed before the failure"
        );
        assert_eq!(
            op.review_points.len(),
            2,
            "INITIAL and the failure point only"
        );
        assert!(matches!(op.review_points[0].kind, ReviewPointKind::Initial));
        assert!(matches!(
            op.review_points[1].kind,
            ReviewPointKind::TerminalFailure(ControllerError::CallbackFailed(_))
        ));
        assert_eq!(
            op.review_points[1].snapshot, op.review_points[0].snapshot,
            "with no completed tick, the failure boundary carries the initial snapshot"
        );
    }

    #[test]
    fn a_deploy_time_load_failure_has_no_reviewable_points() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        let op = state.operation().expect("deploy always records an outcome");
        assert!(op.finished);
        assert!(
            op.review_points.is_empty(),
            "a deployment that never started execution has no fabricated INITIAL, \
             tick, or satellite snapshot"
        );
        assert!(
            op.error.is_some(),
            "the deploy failure is still surfaced via `error`"
        );
    }

    #[test]
    fn discovery_deltas_reflect_only_positions_new_since_the_preceding_review_point() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        while state.advance_running_operation() {}

        let op = state.operation().unwrap();
        assert!(
            op.review_points[0].newly_discovered.is_empty(),
            "INITIAL has no preceding point to be new relative to"
        );
        let mut previously_known: std::collections::HashSet<_> = op.review_points[0]
            .snapshot
            .discovered
            .iter()
            .map(|tile| tile.position)
            .collect();
        let mut saw_new_tile = false;
        for point in &op.review_points[1..] {
            for tile in &point.newly_discovered {
                assert!(
                    !previously_known.contains(&tile.position),
                    "a tile reported as newly discovered must not have been known before"
                );
                saw_new_tile = true;
            }
            previously_known.extend(point.snapshot.discovered.iter().map(|tile| tile.position));
        }
        assert!(
            saw_new_tile,
            "this route should discover at least one new tile after INITIAL"
        );
    }

    #[test]
    fn a_newly_completed_run_selects_its_terminal_review_point() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        while state.advance_running_operation() {}

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert_eq!(op.review_selected, Some(op.review_points.len() - 1));
    }

    #[test]
    fn a_controller_failure_selects_its_terminal_review_point() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);

        state.advance_running_operation();

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert_eq!(op.review_selected, Some(op.review_points.len() - 1));
        assert!(matches!(
            op.review_points[op.review_selected.unwrap()].kind,
            ReviewPointKind::TerminalFailure(_)
        ));
    }

    #[test]
    fn a_deploy_time_load_failure_has_no_review_selection() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        assert_eq!(state.review_selected(), None);
        assert_eq!(state.operation().unwrap().review_selected, None);
    }

    #[test]
    fn navigating_away_and_back_preserves_review_selection() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}
        let selected = state.review_selected();

        state.apply(Msg::Navigate(View::Controller));
        state.apply(Msg::Navigate(View::Operation));

        assert_eq!(state.review_selected(), selected);
    }

    #[test]
    fn help_round_trip_preserves_review_selection() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}
        let selected = state.review_selected();

        state.apply(Msg::OpenHelp);
        state.apply(Msg::DismissHelp);

        assert_eq!(state.review_selected(), selected);
    }

    #[test]
    fn redeploying_resets_review_selection_for_the_new_run() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();
        assert!(
            state.review_selected().is_some(),
            "the first run's failure has a terminal review point selected"
        );

        // `ALWAYS_ERRORS` still loads fine — only the callback itself fails
        // once invoked — so this redeploy starts a fresh live run rather
        // than another synchronous load failure.
        state.apply(Msg::RequestDeploy);

        assert_eq!(
            state.review_selected(),
            None,
            "a fresh live deployment has nothing reviewable yet, and must not \
             carry over the prior run's selection"
        );

        state.advance_running_operation();
        let op = state.operation().unwrap();
        assert!(op.finished);
        assert_eq!(op.review_selected, Some(op.review_points.len() - 1));
    }

    /// A run with several review points to page/jump through: a completed
    /// success run has `INITIAL` plus one point per tick, giving plenty of
    /// room to exercise boundary clamping without wrapping.
    fn deployed_and_run_to_completion(source: &str) -> AppState {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(source));
        state.apply(Msg::RequestDeploy);
        while state.advance_running_operation() {}
        state
    }

    #[test]
    fn up_and_down_reach_every_review_point_in_order_without_wrapping() {
        let mut state = deployed_and_run_to_completion(ROUTE_TO_UPLINK);
        let last = state.operation().unwrap().review_points.len() - 1;
        assert!(last > 1, "this route should complete in more than one tick");

        state.apply(Msg::SelectFirstReviewPoint);
        assert_eq!(state.review_selected(), Some(0));

        for expected in 1..=last {
            state.apply(Msg::SelectNextReviewPoint);
            assert_eq!(state.review_selected(), Some(expected));
        }
        // Inert at the terminal point — never wraps back to the start.
        state.apply(Msg::SelectNextReviewPoint);
        assert_eq!(state.review_selected(), Some(last));

        for expected in (0..last).rev() {
            state.apply(Msg::SelectPreviousReviewPoint);
            assert_eq!(state.review_selected(), Some(expected));
        }
        // Inert at the first point — never wraps back to the terminal one.
        state.apply(Msg::SelectPreviousReviewPoint);
        assert_eq!(state.review_selected(), Some(0));
    }

    #[test]
    fn home_and_end_jump_straight_to_the_first_and_terminal_review_points() {
        let mut state = deployed_and_run_to_completion(ROUTE_TO_UPLINK);
        let last = state.operation().unwrap().review_points.len() - 1;
        assert!(last > 1);

        state.apply(Msg::SelectFirstReviewPoint);
        assert_eq!(state.review_selected(), Some(0));

        state.apply(Msg::SelectLastReviewPoint);
        assert_eq!(state.review_selected(), Some(last));

        state.apply(Msg::SelectPreviousReviewPoint);
        assert_eq!(state.review_selected(), Some(last - 1));
        state.apply(Msg::SelectFirstReviewPoint);
        assert_eq!(
            state.review_selected(),
            Some(0),
            "Home is a semantic jump to the first point, not one step back toward it"
        );

        state.apply(Msg::SelectLastReviewPoint);
        assert_eq!(state.review_selected(), Some(last));
    }

    #[test]
    fn page_moves_clamp_to_valid_review_points() {
        let mut state = deployed_and_run_to_completion(ROUTE_TO_UPLINK);
        let last = state.operation().unwrap().review_points.len() - 1;
        assert!(
            last >= 3,
            "this route should complete in enough ticks to page through"
        );

        state.apply(Msg::SelectFirstReviewPoint);
        state.apply(Msg::SelectReviewPointPageForward(2));
        assert_eq!(state.review_selected(), Some(2));

        // A page larger than the remaining chronology clamps to the
        // terminal point rather than overshooting past it.
        state.apply(Msg::SelectReviewPointPageForward(last + 10));
        assert_eq!(state.review_selected(), Some(last));

        state.apply(Msg::SelectReviewPointPageBackward(2));
        assert_eq!(state.review_selected(), Some(last - 2));

        // Same clamping backward, at the first point.
        state.apply(Msg::SelectReviewPointPageBackward(last + 10));
        assert_eq!(state.review_selected(), Some(0));

        // A `0` page count (`event::map`'s placeholder before `console::mod`
        // fills in the real visible-row count) still moves by at least one
        // point rather than being a silent no-op.
        state.apply(Msg::SelectReviewPointPageForward(0));
        assert_eq!(state.review_selected(), Some(1));
    }

    #[test]
    fn chronology_navigation_is_a_no_op_on_a_live_run() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_WAITS));
        state.apply(Msg::RequestDeploy);
        assert!(
            state.operation().is_some_and(|op| !op.finished),
            "the run should still be live"
        );

        for msg in [
            Msg::SelectPreviousReviewPoint,
            Msg::SelectNextReviewPoint,
            Msg::SelectFirstReviewPoint,
            Msg::SelectLastReviewPoint,
            Msg::SelectReviewPointPageBackward(1),
            Msg::SelectReviewPointPageForward(1),
        ] {
            state.apply(msg.clone());
            assert_eq!(
                state.review_selected(),
                None,
                "{msg:?} must not select a review point on a live run"
            );
        }
    }

    #[test]
    fn chronology_navigation_is_a_no_op_before_any_deployment() {
        let mut state = working_state();
        for msg in [
            Msg::SelectPreviousReviewPoint,
            Msg::SelectNextReviewPoint,
            Msg::SelectFirstReviewPoint,
            Msg::SelectLastReviewPoint,
            Msg::SelectReviewPointPageBackward(1),
            Msg::SelectReviewPointPageForward(1),
        ] {
            state.apply(msg);
            assert_eq!(state.review_selected(), None);
        }
    }

    #[test]
    fn chronology_navigation_leaves_live_operation_pacing_unaffected_on_a_finished_run() {
        // Preserve live Operation input behavior unchanged: chronology
        // navigation and `Space`/`Enter` pacing are different messages
        // entirely, but this guards against a shared no-op gate ever being
        // (mis)wired to also swallow pacing on a finished run.
        let mut state = deployed_and_run_to_completion(ALWAYS_WAITS);
        assert!(state.operation().unwrap().finished);

        state.apply(Msg::SelectNextReviewPoint);
        let selected_after_nav = state.review_selected();

        state.apply(Msg::TogglePauseOperation);
        state.apply(Msg::StepOperationTick);
        assert_eq!(
            state.review_selected(),
            selected_after_nav,
            "pacing controls on a finished run must stay no-ops, not disturb review selection"
        );
    }

    #[test]
    fn a_finished_run_focuses_the_run_inspector_pane() {
        let mut state = working_state();
        state.set_focused_pane(View::Operation, PaneId::Satellite);
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);

        state.advance_running_operation();

        assert!(state.operation().unwrap().finished);
        assert_eq!(
            state.focused_pane(View::Operation),
            PaneId::OperationTelemetry
        );
    }

    #[test]
    fn deploying_a_fresh_run_focuses_the_after_action_report() {
        let mut state = working_state();
        state.set_focused_pane(View::AfterAction, PaneId::FinalFrame);
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        assert_eq!(state.current_view(), View::AfterAction);
        assert_eq!(state.focused_pane(View::AfterAction), PaneId::Report);
    }

    #[test]
    fn a_fresh_after_action_report_starts_scrolled_to_the_top() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));
        state.apply(Msg::RequestDeploy);
        state.scroll_offsets.insert(PaneId::Report, 5);

        // A load-error deploy finishes synchronously via `deploy()` itself.
        state.apply(Msg::RequestDeploy);

        assert_eq!(state.scroll_offset(PaneId::Report), 0);
    }

    #[test]
    fn a_report_reached_by_stepping_a_running_operation_starts_scrolled_to_the_top() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy); // starter controller: running, unfinished
        state.scroll_offsets.insert(PaneId::Report, 5);

        state.advance_running_operation();

        assert!(state.operation().unwrap().finished);
        assert_eq!(state.scroll_offset(PaneId::Report), 0);
    }

    #[test]
    fn redeploying_a_finished_operation_needs_no_confirmation() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
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
    fn finishing_a_running_operation_focuses_the_after_action_report() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);
        state.set_focused_pane(View::AfterAction, PaneId::FinalFrame);

        state.advance_running_operation();

        assert!(state.operation().unwrap().finished);
        assert_eq!(state.current_view(), View::AfterAction);
        assert_eq!(state.focused_pane(View::AfterAction), PaneId::Report);
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
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
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
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
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
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
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
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
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
        // This is the connecting run (a fresh state starts disconnected),
        // so it plays Network Bootstrap and lands on Signals rather than
        // After Action — see the `network_bootstrap_pending` tests below.
        assert_eq!(state.current_view(), View::Signals);
    }

    #[test]
    fn an_unfinished_operation_has_no_conclusion_yet() {
        let mut state = working_state();
        state.apply(Msg::RequestDeploy);

        let op = state.operation().unwrap();

        assert!(!op.finished);
        assert!(op.conclusion.is_none());
    }

    #[test]
    fn a_successful_run_reaches_a_success_conclusion_with_matching_evidence() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        for _ in 0..8 {
            state.advance_running_operation();
        }

        let op = state.operation().unwrap();
        let conclusion = op.conclusion.expect("a finished run has a conclusion");
        assert!(matches!(conclusion.kind, ConclusionKind::Success));
        assert_eq!(conclusion.ticks_executed as usize, op.records.len());
        assert_eq!(
            conclusion.tiles_discovered as usize,
            op.current.discovered.len()
        );
        assert_eq!(conclusion.final_budget, op.current.budget_remaining);
        assert_eq!(conclusion.run_id, op.run_id);
    }

    const ALWAYS_WAITS: &str = "function on_tick(observation) return \"wait\" end";

    #[test]
    fn exhausting_the_budget_reaches_a_budget_exhausted_conclusion() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_WAITS));
        state.apply(Msg::RequestDeploy);

        for _ in 0..15 {
            state.advance_running_operation();
        }

        let op = state.operation().unwrap();
        assert!(op.finished);
        let conclusion = op.conclusion.expect("a finished run has a conclusion");
        assert!(matches!(conclusion.kind, ConclusionKind::BudgetExhausted));
        assert_eq!(conclusion.final_budget, 0);
    }

    #[test]
    fn a_fresh_app_state_starts_disconnected() {
        assert!(!AppState::new().connected());
    }

    #[test]
    fn a_successful_first_contact_run_establishes_connectivity() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        assert!(
            !state.connected(),
            "connectivity must not be recorded before the run actually succeeds"
        );

        for _ in 0..8 {
            state.advance_running_operation();
        }

        let op = state.operation().unwrap();
        assert!(op.finished);
        assert!(matches!(
            op.conclusion.as_ref().map(|c| &c.kind),
            Some(ConclusionKind::Success)
        ));
        assert!(state.connected());
    }

    #[test]
    fn a_budget_exhausted_run_does_not_establish_connectivity() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_WAITS));
        state.apply(Msg::RequestDeploy);

        for _ in 0..15 {
            state.advance_running_operation();
        }

        assert!(state.operation().unwrap().finished);
        assert!(!state.connected());
    }

    #[test]
    fn connectivity_once_established_survives_a_later_failed_run() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        for _ in 0..8 {
            state.advance_running_operation();
        }
        assert!(state.connected());

        state.controller = Some(ControllerDocument::new(ALWAYS_WAITS));
        state.apply(Msg::RequestDeploy);
        for _ in 0..15 {
            state.advance_running_operation();
        }

        assert!(
            state.connected(),
            "an already-established connection must not be undone by a later failed run"
        );
    }

    #[test]
    fn a_fresh_app_state_has_no_network_bootstrap_pending() {
        assert!(!AppState::new().network_bootstrap_pending());
    }

    #[test]
    fn a_successful_first_contact_run_enters_network_bootstrap_pending_and_lands_on_signals() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);

        for _ in 0..8 {
            state.advance_running_operation();
        }

        assert!(state.connected());
        assert!(state.network_bootstrap_pending());
        assert_eq!(
            state.current_view(),
            View::Signals,
            "the connecting success plays Network Bootstrap in place of the \
             automatic landing on After Action"
        );
    }

    #[test]
    fn a_budget_exhausted_run_does_not_enter_network_bootstrap_pending() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_WAITS));
        state.apply(Msg::RequestDeploy);

        for _ in 0..15 {
            state.advance_running_operation();
        }

        assert!(!state.network_bootstrap_pending());
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn a_controller_error_run_does_not_enter_network_bootstrap_pending() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);

        state.advance_running_operation();

        assert!(state.operation().unwrap().finished);
        assert!(!state.network_bootstrap_pending());
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn a_deploy_time_load_failure_does_not_enter_network_bootstrap_pending() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        assert!(!state.connected());
        assert!(!state.network_bootstrap_pending());
        assert_eq!(state.current_view(), View::AfterAction);
    }

    #[test]
    fn a_later_successful_run_after_connecting_does_not_replay_network_bootstrap() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        for _ in 0..8 {
            state.advance_running_operation();
        }
        assert!(state.connected());
        assert!(state.network_bootstrap_pending());

        // Let the Network Bootstrap presentation actually finish (one extra
        // call beyond the step count, since the last step stays visible for
        // a cadence before the transition completes), then redeploy and
        // succeed again.
        while state.advance_network_bootstrap() {}
        assert!(!state.network_bootstrap_pending());
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        for _ in 0..8 {
            state.advance_running_operation();
        }

        assert!(
            !state.network_bootstrap_pending(),
            "a second success after connectivity is already established must not \
             re-trigger Network Bootstrap"
        );
        assert_eq!(
            state.current_view(),
            View::AfterAction,
            "a second success routes to After Action like any other terminal outcome"
        );
    }

    #[test]
    fn reviewing_the_connecting_run_preserves_its_immutable_review_data() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        for _ in 0..8 {
            state.advance_running_operation();
        }
        assert_eq!(state.current_view(), View::Signals);

        let review_selected_before = state.review_selected;
        let records_before = state.operation.as_ref().unwrap().records.clone();

        state.apply(Msg::Navigate(View::Operation));

        assert_eq!(state.review_selected, review_selected_before);
        assert_eq!(state.operation.as_ref().unwrap().records, records_before);
        assert!(state.connected());
        assert!(state.network_bootstrap_pending());
    }

    fn connected_and_pending_network_bootstrap() -> AppState {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
        state.apply(Msg::RequestDeploy);
        for _ in 0..8 {
            state.advance_running_operation();
        }
        assert!(state.network_bootstrap_pending());
        state
    }

    #[test]
    fn presentation_connected_lags_durable_connectivity_until_bootstrap_completes() {
        let state = working_state();
        assert!(!state.connected());
        assert!(!state.presentation_connected());

        let mut state = connected_and_pending_network_bootstrap();
        assert!(
            state.connected(),
            "durable connectivity is recorded the instant success is determined"
        );
        assert!(
            !state.presentation_connected(),
            "but presentation must keep rendering as disconnected while the \
             modal is still playing"
        );

        while state.advance_network_bootstrap() {}
        assert!(!state.network_bootstrap_pending());
        assert!(
            state.presentation_connected(),
            "presentation catches up to durable connectivity only once the \
             transition completes"
        );
    }

    #[test]
    fn network_bootstrap_auto_advancing_only_while_pending() {
        let state = working_state();
        assert!(
            !state.network_bootstrap_auto_advancing(),
            "nothing pending yet"
        );

        let mut state = connected_and_pending_network_bootstrap();
        assert!(state.network_bootstrap_auto_advancing());

        for _ in 0..NETWORK_BOOTSTRAP_STEPS.len() {
            state.advance_network_bootstrap();
        }
        assert!(
            state.network_bootstrap_auto_advancing(),
            "every step has shown, but the transition needs its linger \
             window and one more cadence to actually complete — see \
             advance_network_bootstrap_clears_pending_after_the_last_step_\
             and_then_no_ops"
        );

        for _ in 0..NETWORK_BOOTSTRAP_LINGER_TICKS {
            state.advance_network_bootstrap();
            assert!(
                state.network_bootstrap_auto_advancing(),
                "still lingering on the completed state"
            );
        }

        state.advance_network_bootstrap();
        assert!(
            !state.network_bootstrap_auto_advancing(),
            "the completing cadence has now run, so the transition is done"
        );
    }

    #[test]
    fn advance_network_bootstrap_steps_once_when_pending_and_otherwise_is_a_no_op() {
        let mut state = working_state();
        assert!(!state.advance_network_bootstrap());

        let mut state = connected_and_pending_network_bootstrap();
        let advanced = state.advance_network_bootstrap();

        assert!(advanced);
        assert_eq!(state.network_bootstrap_steps_shown().len(), 1);
        assert_eq!(
            state.network_bootstrap_progress(),
            (1, NETWORK_BOOTSTRAP_STEPS.len())
        );
    }

    #[test]
    fn advance_network_bootstrap_clears_pending_after_the_last_step_and_then_no_ops() {
        let mut state = connected_and_pending_network_bootstrap();

        for _ in 0..NETWORK_BOOTSTRAP_STEPS.len() {
            assert!(state.advance_network_bootstrap());
            assert!(
                state.network_bootstrap_pending(),
                "still pending — every step has been revealed, but the final \
                 one hasn't been shown for a cadence yet"
            );
        }
        assert_eq!(
            state.network_bootstrap_steps_shown(),
            NETWORK_BOOTSTRAP_STEPS,
            "every step is revealed by this point, and stays visible \
             through the linger window before the transition completes"
        );

        for _ in 0..NETWORK_BOOTSTRAP_LINGER_TICKS {
            assert!(state.advance_network_bootstrap());
            assert!(
                state.network_bootstrap_pending(),
                "still lingering on the completed, 100%-progress state"
            );
            assert_eq!(
                state.network_bootstrap_steps_shown(),
                NETWORK_BOOTSTRAP_STEPS,
                "the fully-revealed step list stays as-is through lingering"
            );
        }

        assert!(
            state.advance_network_bootstrap(),
            "the completing cadence — no further step to reveal, but this is \
             what actually clears the flag"
        );
        assert!(
            !state.network_bootstrap_pending(),
            "the transition completes only after the last step has lingered \
             through its full window"
        );

        assert!(
            !state.advance_network_bootstrap(),
            "no further steps to advance, and it must never replay"
        );
    }

    #[test]
    fn a_deploy_time_load_failure_reaches_a_controller_error_conclusion() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("function on_tick("));

        state.apply(Msg::RequestDeploy);

        let op = state.operation().unwrap();
        let conclusion = op.conclusion.expect("a finished run has a conclusion");
        assert!(matches!(
            conclusion.kind,
            ConclusionKind::ControllerError(ControllerError::ScriptInvalid(_))
        ));
    }

    #[test]
    fn a_callback_error_reaches_a_controller_error_conclusion() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);

        state.advance_running_operation();

        let op = state.operation().unwrap();
        let conclusion = op.conclusion.expect("a finished run has a conclusion");
        assert!(matches!(
            conclusion.kind,
            ConclusionKind::ControllerError(ControllerError::CallbackFailed(_))
        ));
    }

    #[test]
    fn a_missing_callback_reaches_a_controller_error_conclusion() {
        let mut state = working_state();
        state.controller = Some(ControllerDocument::new("local x = 1"));

        state.apply(Msg::RequestDeploy);

        let op = state.operation().unwrap();
        let conclusion = op.conclusion.expect("a finished run has a conclusion");
        assert!(matches!(
            conclusion.kind,
            ConclusionKind::ControllerError(ControllerError::MissingCallback)
        ));
    }

    #[test]
    fn reviewing_a_finished_run_returns_to_operation_with_the_frozen_telemetry() {
        // Already connected, so this run is a non-connecting success and
        // this test stays about generic post-completion navigation rather
        // than the Network Bootstrap routing covered separately below.
        let mut state = working_state();
        state.set_connected(true);
        state.controller = Some(ControllerDocument::new(ROUTE_TO_UPLINK));
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
        state.controller = Some(ControllerDocument::new(ALWAYS_ERRORS));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();
        assert!(state.operation().unwrap().finished);
        // Restore the controller to the unmodified starter so this test
        // isolates the finished operation's own contribution to the quit
        // gate from the (unrelated) modified-controller gate.
        state.controller = Some(ControllerDocument::new(
            super::super::intel::STARTER_CONTROLLER,
        ));
        assert!(!state.controller_modified());

        state.apply(Msg::RequestQuit);

        assert!(state.should_quit());
    }
}
