//! Renders the resistance console's persistent frame and per-view content.
//!
//! Every function here is a pure `(state, frame area) -> drawn widgets`
//! operation so it can be exercised against `ratatui`'s `TestBackend`
//! without a real terminal.

use super::intel::{Signal, TargetDossier, authored_signals, first_contact_dossier};
use super::state::{
    AppState, ConclusionKind, OperationSnapshot, OperationView, PaneId, ReviewPoint,
    ReviewPointKind, Validation, View, WorkingSet,
};
use crate::lua_controller::{ControllerError, TickRecord};
use crate::render::render_satellite_view;
use crate::simulation::{Action, FailureReason, SimEvent, TickOutcome};
use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

pub const MIN_COLUMNS: u16 = 120;
pub const MIN_ROWS: u16 = 40;

const TITLE: &str = "HUMAN EXCEPTION // RESISTANCE CONSOLE";

/// Draws the full console frame for the current session state.
pub fn draw(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    // Checked before the undersized-geometry return, and drawn without the
    // header/body/footer layout that return skips past: `Ctrl+Q` is global
    // and the confirmation must stay reachable at any size (`docs/
    // TUI_DESIGN.md`, "Below 120 columns" — "Quitting remains available,
    // subject to the same modified-source confirmation rule"). Rendering it
    // only after the geometry check would make it swallow input invisibly
    // whenever a modified session got resized below the minimum.
    if state.quit_confirmation_pending() {
        draw_quit_confirmation(frame, area, state);
        return;
    }

    if area.width < MIN_COLUMNS || area.height < MIN_ROWS {
        draw_geometry_warning(frame, area);
        return;
    }

    draw_console(frame, area, state);
}

/// The header/body/footer layout `draw` renders once the quit-confirmation
/// and minimum-geometry checks above have both passed.
fn draw_console(frame: &mut Frame, area: Rect, state: &AppState) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);

    draw_header(frame, header, state);
    draw_body(frame, body, state);
    draw_footer(frame, footer, state);
}

/// Rendered in place of the entire frame (like [`draw_geometry_warning`]),
/// not just the body, so it stays visible even below the supported minimum
/// geometry.
fn draw_quit_confirmation(frame: &mut Frame, area: Rect, state: &AppState) {
    let active_run = state.operation().is_some_and(|op| !op.finished);
    let lines = quit_confirmation_lines(area.height, state.controller_modified(), active_run);
    frame.render_widget(Paragraph::new(lines), area);
}

/// The bold headline explaining what quitting would lose, given whether the
/// controller is modified and/or a run is currently active — either, both,
/// or (defensively) neither, though `draw_quit_confirmation` only ever
/// shows this dialog when at least one is true.
fn quit_confirmation_headline(controller_modified: bool, active_run: bool) -> &'static str {
    match (controller_modified, active_run) {
        (true, true) => {
            "Modified controller source will be lost and the active run will be abandoned."
        }
        (true, false) => "Modified controller source will be lost.",
        (false, true) => "The active run will be abandoned.",
        (false, false) => "Quit the resistance console?",
    }
}

/// The quit-confirmation dialog's content for a frame `height` rows tall.
/// `Ctrl+Q` and this confirmation are intentionally still reachable below
/// the supported minimum geometry (`docs/TUI_DESIGN.md`'s "Quit safety"),
/// so a terminal resized down to just a few rows must still be able to
/// confirm or cancel — an unconditional, un-prioritized line list would
/// instead have its `Enter`/`Esc` action rows (listed last) clipped first
/// by `Paragraph`'s default truncation, at exactly the moment the player
/// most needs them, while decorative lines above survive. Picks the
/// tallest of three fixed candidates that actually fits, most detail
/// first — the same "drop lowest-priority content first" pattern the
/// header candidate lists use, rather than a signal a mid-dialog resize
/// could keep bouncing between.
fn quit_confirmation_lines(
    height: u16,
    controller_modified: bool,
    active_run: bool,
) -> Vec<Line<'static>> {
    let headline = quit_confirmation_headline(controller_modified, active_run);
    let full = vec![
        Line::from("HUMAN EXCEPTION // resistance console"),
        Line::from(Span::styled(
            headline,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Cross-launch persistence isn't implemented, so this edit can't be saved."),
        Line::from(""),
        Line::from("Enter / y  confirm and quit"),
        Line::from("Esc / n    cancel and return"),
    ];
    if height as usize >= full.len() {
        return full;
    }
    let compact = vec![
        Line::from(Span::styled(
            headline,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Enter / y  confirm and quit"),
        Line::from("Esc / n    cancel and return"),
    ];
    if height as usize >= compact.len() {
        return compact;
    }
    if height == 0 {
        return Vec::new();
    }
    vec![Line::from(
        "Quit and lose edits? Enter/y confirm, Esc/n cancel",
    )]
}

fn draw_geometry_warning(frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::from("HUMAN EXCEPTION // resistance console"),
        Line::from("Terminal link degraded."),
        Line::from(format!(
            "Minimum console geometry: {MIN_COLUMNS}x{MIN_ROWS}"
        )),
        Line::from(format!("Current geometry: {}x{}", area.width, area.height)),
        Line::from("Resize the terminal to restore the resistance console."),
        Line::from(""),
        Line::from("Ctrl+Q Quit"),
    ];
    frame.render_widget(Paragraph::new(text), area);
}

/// `starter`/`modified`/`invalid`, or `None` before a working set has
/// seeded any source. Shared by [`controller_status_field`] (which adds the
/// `CONTROLLER: ` prefix and, for other views, a `STATUS: READY` suffix)
/// and Operation's own header branch (which needs the bare word alongside
/// its own, different `STATUS:` field).
fn controller_status_only(state: &AppState) -> Option<&'static str> {
    state.controller_source()?;
    Some(if matches!(state.validation(), Validation::Invalid(_)) {
        "invalid"
    } else if state.controller_modified() {
        "modified"
    } else {
        "starter"
    })
}

/// `CONTROLLER: starter/modified/invalid`, or `None` before a working set
/// has seeded any source. Shared by every header branch so a modified
/// controller's at-risk status stays visible no matter which view the
/// player is currently looking at (`docs/TUI_DESIGN.md`, "Persistent
/// header").
fn controller_status_field(state: &AppState) -> Option<String> {
    let status = controller_status_only(state)?;
    // `docs/TUI_DESIGN.md`'s persistent-header mockup shows `STATUS: READY`
    // as its own field alongside `CONTROLLER: ...`, not folded into it — a
    // successfully validated controller stays "READY" independent of
    // whether it's still the unmodified starter, so it needs a field of
    // its own rather than a fourth `CONTROLLER:` state. `Invalid` doesn't
    // get a matching `STATUS:` value here since `CONTROLLER: invalid`
    // already says as much on its own.
    let ready = matches!(state.validation(), Validation::Valid).then_some("   STATUS: READY");
    Some(format!("CONTROLLER: {status}{}", ready.unwrap_or_default()))
}

fn draw_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let working_set = match state.working_set() {
        Some(WorkingSet::FirstContact) => "FIRST CONTACT",
        None => "none",
    };
    let working = format!("WORKING SET: {working_set}");

    // Candidates in priority order (most detail first): the working set is
    // always the last-dropped field, since it's the one status the header
    // priorities in `docs/TUI_DESIGN.md` ("Persistent header") most need to
    // stay visible at the supported minimum width.
    let controller_field = controller_status_field(state);

    let candidates: Vec<String> = match state.current_view() {
        View::Signals => {
            let signals = format!("SIGNALS: {:02}", authored_signals().len());
            let mut candidates = Vec::new();
            // A controller's status — `starter`/`modified`/`invalid`, plus
            // `STATUS: READY` once validated — is session-only state the
            // player can lose (an unsaved edit, a validation result), so
            // every candidate that keeps it is tried before any that drops
            // it, MESH included: MESH is a static "link condition" label
            // the player can always re-derive by looking at any other
            // view, but a lost controller status is gone for good. Within
            // each of those two groups, MESH is still dropped before the
            // signals count, and the signals count before controller
            // status itself, matching every other view's priority order.
            if let Some(controller) = &controller_field {
                candidates.push(format!(
                    "MESH: DEGRADED   {signals}   {controller}   {working}"
                ));
                candidates.push(format!("MESH: DEGRADED   {controller}   {working}"));
                candidates.push(format!("{signals}   {controller}   {working}"));
                candidates.push(format!("{controller}   {working}"));
            }
            candidates.push(format!("MESH: DEGRADED   {signals}   {working}"));
            candidates.push(format!("{signals}   {working}"));
            candidates
        }
        View::Target => {
            let dossier = first_contact_dossier();
            let target = format!("TARGET: {}", dossier.title);
            let confidence = format!("CONFIDENCE: {}", dossier.confidence_summary);
            let mut candidates = Vec::new();
            if let Some(controller) = &controller_field {
                candidates.push(format!(
                    "MESH: DEGRADED   {target}   {controller}   {working}"
                ));
                // Drop the Target field itself (its own dossier title)
                // before dropping controller status — a modified controller
                // is session-only and can be lost, so it outranks a title
                // the player can always re-derive by returning to Target
                // (or that's already echoed by WORKING SET once committed).
                candidates.push(format!("MESH: DEGRADED   {controller}   {working}"));
            }
            candidates.push(format!(
                "MESH: DEGRADED   {target}   {confidence}   {working}"
            ));
            // Confidence and controller status are both lower-priority than
            // MESH here — drop them first, not MESH, so Target doesn't lose
            // link condition while every other view keeps it at the same
            // widths.
            candidates.push(format!("MESH: DEGRADED   {target}   {working}"));
            if let Some(controller) = &controller_field {
                candidates.push(format!("{target}   {controller}   {working}"));
                candidates.push(format!("{controller}   {working}"));
            }
            candidates.push(format!("{target}   {confidence}   {working}"));
            candidates.push(format!("{target}   {working}"));
            candidates
        }
        View::Operation | View::AfterAction => operation_status_header_candidates(state, &working),
        _ => match &controller_field {
            Some(controller) => vec![
                format!("MESH: DEGRADED   SATLINK: COMPROMISED   {controller}   {working}"),
                format!("SATLINK: COMPROMISED   {controller}   {working}"),
                format!("{controller}   {working}"),
                working.clone(),
            ],
            None => vec![
                format!("MESH: DEGRADED   SATLINK: COMPROMISED   {working}"),
                format!("SATLINK: COMPROMISED   {working}"),
                working.clone(),
            ],
        },
    };

    let inner_width = area.width.saturating_sub(2) as usize;
    let status = candidates
        .iter()
        .find(|candidate| candidate.chars().count() <= inner_width)
        .or(candidates.last())
        .cloned()
        .unwrap_or_default();

    let block = Block::default().borders(Borders::ALL).title(TITLE);
    frame.render_widget(Paragraph::new(Line::from(status)).block(block), area);
}

/// Header candidates shared by `Operation` and `AfterAction`: both show the
/// deployment's `STATUS: RUNNING/PAUSED/SUCCEEDED/FAILED/...` alongside a
/// bare controller status, deliberately not `controller_field`'s own
/// "STATUS: READY" suffix — that would collide visually with the
/// operation's own status field (`docs/TUI_DESIGN.md`'s "Persistent
/// header" shows exactly one STATUS field per view).
fn operation_status_header_candidates(state: &AppState, working: &str) -> Vec<String> {
    let controller = controller_status_only(state).map(|status| format!("CONTROLLER: {status}"));
    let op_status = state
        .operation()
        .map(|op| format!("STATUS: {}", operation_status_label(&op).to_uppercase()));
    let mut candidates = Vec::new();
    if let (Some(controller), Some(op_status)) = (&controller, &op_status) {
        candidates.push(format!(
            "MESH: DEGRADED   SATLINK: COMPROMISED   {controller}   {op_status}   {working}"
        ));
        candidates.push(format!(
            "SATLINK: COMPROMISED   {controller}   {op_status}   {working}"
        ));
        candidates.push(format!("{controller}   {op_status}   {working}"));
    }
    if let Some(op_status) = &op_status {
        candidates.push(format!("{op_status}   {working}"));
    }
    candidates.push(working.to_string());
    candidates
}

fn draw_body(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.current_view() {
        View::Signals => draw_signals(frame, area, state),
        View::Target => draw_target(frame, area, state),
        View::Help => draw_help(frame, area, state),
        View::Controller => draw_controller(frame, area, state),
        View::Operation => draw_operation(frame, area, state),
        View::AfterAction => draw_after_action(frame, area, state),
    }
}

fn draw_controller(frame: &mut Frame, area: Rect, state: &AppState) {
    let focused = state.focused_pane(View::Controller);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(area);
    draw_controller_source(frame, left, state, focused == PaneId::ControllerSource);
    draw_pane(
        frame,
        right,
        pane_title("LUA FIELD REFERENCE", focused == PaneId::LuaFieldReference),
        lua_field_reference_lines(),
    );
}

/// An upper bound on how many rows [`controller_banner`] can claim, so one
/// long message can't swallow the whole pane.
const MAX_BANNER_ROWS: u16 = 4;

/// How many rows to reserve for `banner` at `width`, wrapping instead of
/// clipping a message that runs past one row — a Lua syntax error easily
/// exceeds the source pane's width, and clipping it can lose the `:line:`
/// location `docs/TUI_DESIGN.md` requires stay visible.
fn banner_height(banner: &[Line<'static>], width: u16) -> u16 {
    (wrapped_row_count(banner, width) as u16).clamp(1, MAX_BANNER_ROWS)
}

fn draw_controller_source(frame: &mut Frame, area: Rect, state: &AppState, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(pane_title("CAPTURED CONTROLLER // controller.lua", focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let banner = controller_banner(state);
    let (content_area, banner_area) = if let Some(banner) = &banner {
        let rows = banner_height(banner, inner.width);
        let [content, banner_row] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(rows)]).areas(inner);
        (content, Some(banner_row))
    } else {
        (inner, None)
    };

    // The editor foundation (issue #90) owns gutter, wrapping, and viewport
    // scroll for its own widget; Controller only supplies the area and,
    // when this pane is focused, forwards the widget's own cursor position
    // to the real terminal cursor (`docs/TUI_DESIGN.md`'s "the cursor is
    // visibly rendered whenever Controller source is the focused pane").
    if let Some(document) = state.controller() {
        let editor = document.sync_for_render(content_area);
        frame.render_widget(&*editor, content_area);
        if focused && let Some(cursor) = editor.get_visible_cursor(&content_area) {
            frame.set_cursor_position(cursor);
        }
    }

    if let (Some(banner_area), Some(banner)) = (banner_area, banner) {
        frame.render_widget(
            Paragraph::new(banner).wrap(Wrap { trim: false }),
            banner_area,
        );
    }
}

/// The confirmation prompt or validation result shown as a fixed last row
/// (or rows) under the source, or `None` when there's nothing to say (an
/// unmodified, unchecked controller keeps the full pane for source).
fn controller_banner(state: &AppState) -> Option<Vec<Line<'static>>> {
    // The quit-confirmation prompt is drawn globally by `draw` (it can be
    // triggered from any view), so it isn't handled here even though it's
    // Controller-adjacent state.
    if state.reset_confirmation_pending() {
        return Some(vec![Line::from(Span::styled(
            "Reset controller? Edits will be lost. Enter/y confirm  Esc/n cancel",
            Style::default().add_modifier(Modifier::BOLD),
        ))]);
    }
    match state.validation() {
        Validation::Unchecked => None,
        Validation::Valid => Some(vec![Line::from(Span::styled(
            "READY: controller loads and defines on_tick",
            Style::default().add_modifier(Modifier::BOLD),
        ))]),
        // A load/runtime error message is player-influenced Lua text (see
        // `strip_control_characters`) and can legitimately contain raw
        // newlines/tabs (e.g. a stack traceback). Splitting on the embedded
        // newlines first, rather than only stripping them, keeps the
        // `:line:` location `docs/TUI_DESIGN.md` requires on its own first
        // row instead of merging it into one wrapped line with whatever
        // traceback text follows it.
        Validation::Invalid(message) => Some(
            format!("INVALID: {message}")
                .lines()
                .map(|line| {
                    Line::from(Span::styled(
                        strip_control_characters(line),
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                })
                .collect(),
        ),
    }
}

/// A short, representative subset of the Lua contract shown as a cheat
/// sheet next to the editor. See `help_lines`'s "Lua contract" section for
/// the complete reference; the two are checked for consistency in tests so
/// they can't silently drift.
fn lua_field_reference_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "on_tick(observation)",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("observation.drone.x / .y"),
        Line::from("observation.tick"),
        Line::from("observation.budget_remaining"),
        Line::from("observation.discovered[]"),
        Line::from(""),
        Line::from(Span::styled(
            "return:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("north south east west"),
        Line::from("wait scan"),
        Line::from(""),
        Line::from(Span::styled(
            "libraries:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("table, string, math only"),
        Line::from(""),
        Line::from("F1 opens the complete reference"),
    ]
}

/// The live operation view: the satellite feed dominates, telemetry is
/// secondary (`docs/TUI_DESIGN.md` §4, "Operation"). Both panes always
/// render; `F8` moves which one carries the focus marker, the same as
/// Controller and Signals.
fn draw_operation(frame: &mut Frame, area: Rect, state: &AppState) {
    // Wins over the normal layout regardless of width, the same way a
    // pending reset confirmation always wins Controller's focus-driven
    // visibility: the prompt (and the `Enter`/`Esc` it's waiting on) must
    // never end up on the pane the player currently isn't looking at.
    if state.redeploy_confirmation_pending() {
        draw_pane(
            frame,
            area,
            "REDEPLOY?".to_string(),
            vec![
                Line::from(Span::styled(
                    "A run is already active. Redeploying replaces it from a clean start.",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("Enter / y  confirm and redeploy"),
                Line::from("Esc / n    cancel and return to the active run"),
            ],
        );
        return;
    }

    let Some(op) = state.operation() else {
        draw_pane(
            frame,
            area,
            "OPERATION".to_string(),
            vec![
                Line::from("No operation is deployed yet."),
                Line::from("F6 deploys the current controller."),
            ],
        );
        return;
    };

    let focused = state.focused_pane(View::Operation);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    // Once the run is finished, this view is functioning as Review Run
    // rather than a live view (`docs/TUI_DESIGN.md` §5, "Review Run"): both
    // panes must describe the same selected review point, not whatever the
    // live view was last showing.
    let (satellite, telemetry) = if op.finished {
        (
            review_run_satellite_lines(&op),
            review_run_telemetry_lines(&op),
        )
    } else {
        (satellite_lines(&op.current), telemetry_lines(&op))
    };
    draw_pane(
        frame,
        left,
        pane_title("COMPROMISED SATELLITE FEED", focused == PaneId::Satellite),
        satellite,
    );
    draw_pane(
        frame,
        right,
        pane_title("OPERATION TELEMETRY", focused == PaneId::OperationTelemetry),
        telemetry,
    );
}

/// The reflective, run-concluded view: the same final satellite frame
/// Operation was last showing, alongside a concise mechanical outcome and
/// summary stats (`docs/TUI_DESIGN.md` §5, "After Action is an operation
/// state, not a disconnected popup"). Reuses `satellite_lines`/`draw_pane`
/// and the same two-pane structure as `draw_operation`.
fn draw_after_action(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(op) = state.operation() else {
        draw_pane(
            frame,
            area,
            "AFTER-ACTION REPORT".to_string(),
            vec![
                Line::from("No operation has concluded yet."),
                Line::from("F4 revises the controller, F6 deploys it."),
            ],
        );
        return;
    };

    let scroll = state.scroll_offset(PaneId::Report);
    let focused = state.focused_pane(View::AfterAction);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    draw_pane(
        frame,
        left,
        pane_title("FINAL SATELLITE FRAME", focused == PaneId::FinalFrame),
        satellite_lines(&op.current),
    );
    draw_after_action_report_pane(frame, right, &op, scroll, focused == PaneId::Report);
}

/// Draws the AFTER-ACTION REPORT pane, pinning the `F4` recovery hint to a
/// fixed last row on failure so a long diagnostic or deployed-source excerpt
/// filling [`MAX_DETAIL_LINES`] can never push it off the bottom of the
/// pane at the console's supported minimum geometry, and scrolling its
/// content by `scroll` rows (`PaneId::Report`'s entry in
/// `event::scroll_focus_matches`, #76/#77) so the success report's outcome
/// hierarchy spacing can't be clipped there either. Built inline rather
/// than through `draw_pane`/`draw_pane_with_pinned_action`, matching
/// `draw_help`'s own scroll-aware rendering, since neither shared helper
/// takes a scroll offset (most of their callers have unscrollable content).
fn draw_after_action_report_pane(
    frame: &mut Frame,
    area: Rect,
    op: &OperationView<'_>,
    scroll: u16,
    focused: bool,
) {
    let lines = after_action_report_lines(op);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(pane_title("AFTER-ACTION REPORT", focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // As in `draw_help`: the event loop already clamps the stored offset
    // via `after_action_max_scroll` as each scroll key arrives; this
    // recomputes against the exact rendered area as a second, render-time
    // backstop.
    let content_area = if after_action_succeeded(op) {
        inner
    } else {
        let [content, action_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        frame.render_widget(Paragraph::new("F4  revise the controller"), action_area);
        content
    };
    let content_rows = wrapped_row_count(&lines, content_area.width);
    let max_scroll = content_rows.saturating_sub(content_area.height as usize) as u16;
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll.min(max_scroll), 0));
    frame.render_widget(paragraph, content_area);
}

/// The after-action report's content: a headline and one-line mechanical
/// explanation (distinguishing success, budget exhaustion, an invalid
/// controller action, and a Lua script/runtime failure from each other, per
/// issue #46's acceptance criteria), summary stats, the deployed run's
/// identifier, and an obvious next step. Failure explanations state the
/// mechanical reason without prescribing the exact solution (`docs/
/// TUI_DESIGN.md` §5).
/// A cap on how many lines (and characters per line) diagnostic or source
/// text can occupy in a report pane. Without a bound, a long player-
/// controlled error message (`error(string.rep("x", 4000))`) or a large
/// deployed script could push a pane's trailing summary stats and next-step
/// guidance off the bottom of the console's supported minimum geometry.
const MAX_DETAIL_LINES: usize = 8;
const MAX_DETAIL_LINE_CHARS: usize = 120;

/// Replaces any control character (e.g. a tab from a Lua stack traceback)
/// with a space. Lua error messages are player-influenced text and can
/// legitimately contain raw control characters; `ratatui`'s `CellWidth`
/// panics if one ever reaches its width calculations unfiltered, so every
/// Lua-derived string must pass through this before becoming `Line`
/// content — splitting on embedded newlines first (`str::lines`) rather
/// than replacing them too, so multi-line text becomes separate `Line`s
/// instead of one line with the break turned into a plain space.
fn strip_control_characters(line: &str) -> String {
    line.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Renders `text` capped to [`MAX_DETAIL_LINES`]/[`MAX_DETAIL_LINE_CHARS`],
/// with a trailing `…` marker whenever something was cut off, so the rest
/// of whatever pane called this always has room for its own content.
fn bounded_detail_lines(text: &str) -> Vec<Line<'static>> {
    let total_lines = text.lines().count();
    let mut lines: Vec<Line<'static>> = text
        .lines()
        .map(strip_control_characters)
        .take(MAX_DETAIL_LINES)
        .map(|line| {
            let char_count = line.chars().count();
            if char_count > MAX_DETAIL_LINE_CHARS {
                let truncated: String = line.chars().take(MAX_DETAIL_LINE_CHARS).collect();
                Line::from(format!("{truncated}…"))
            } else {
                Line::from(line)
            }
        })
        .collect();
    if total_lines > MAX_DETAIL_LINES {
        lines.push(Line::from("…"));
    }
    lines
}

/// Whether an operation's After Action report reflects a successful run —
/// shared by [`after_action_report_lines`] (guidance wording) and
/// [`draw_after_action_report_pane`] (whether to pin the `F4` recovery
/// hint).
fn after_action_succeeded(op: &OperationView<'_>) -> bool {
    matches!(
        op.conclusion.map(|conclusion| conclusion.kind),
        Some(ConclusionKind::Success)
    )
}

/// The success report's headline, per `docs/TUI_DESIGN.md` §5: a resistance
/// network foothold, not generic victory language. Shared with Review Run's
/// `telemetry_lines` via [`outcome_headline`] so the two views describe the
/// same recorded run consistently.
const FOOTHOLD_ESTABLISHED_HEADLINE: &str = "FOOTHOLD ESTABLISHED";

/// The success report's trigger + fictional meaning, echoing Target's
/// original framing (`docs/TUI_DESIGN.md` §5 "Success"). Deliberately does
/// not claim the facility was captured, owned, or made persistently
/// operable — only that a foothold/access point was established.
const FOOTHOLD_ESTABLISHED_MEANING: &str = "The drone reached the facility uplink. Resistance access to the facility network was established before the access window closed.";

const FIRST_CONTACT_COMPLETE: &str = "FIRST CONTACT COMPLETE";

/// The success report's truthful availability statement and next actions
/// (`docs/TUI_DESIGN.md` §5 "Success"): no further operation exists at this
/// facility, and Signals is worthwhile as the wider network, not because
/// another operation here is waiting.
const NO_FURTHER_OPERATION: &str = "No further operation is available at this facility. Review the run, redeploy to try another approach, or return to Signals for the wider network.";

/// The failure report's combined trigger + meaning line for budget
/// exhaustion (`docs/TUI_DESIGN.md` §5 "Failure and controller error"):
/// names the mechanical reason without prescribing the fix, then states the
/// consequence — the failure counterpart to [`FOOTHOLD_ESTABLISHED_MEANING`],
/// pushed the same way (directly, not through `bounded_detail_lines`, since
/// it's trusted fixed copy meant to reflow via `Wrap`).
const BUDGET_EXHAUSTED_MEANING: &str = "The operational budget was exhausted before the drone reached the uplink. No facility foothold was established.";

/// The failure report's combined trigger + meaning line for a controller
/// error, stated before the preserved diagnostic detail
/// (`bounded_detail_lines(&controller_error_detail(..))`) so the mechanical
/// "execution stopped" fact and its consequence read before the specific
/// error text.
const CONTROLLER_EXECUTION_STOPPED: &str = "Controller execution stopped before reaching the uplink. No facility foothold was established.";

/// The failure report's completion line — the direct counterpart to
/// [`FIRST_CONTACT_COMPLETE`], "no less clear" per `docs/TUI_DESIGN.md` §5.
const FIRST_CONTACT_INCOMPLETE: &str = "FIRST CONTACT INCOMPLETE";

/// The failure report's truthful availability statement and next actions
/// (`docs/TUI_DESIGN.md` §5 "Failure and controller error" mockup): the same
/// availability truth as success, but the primary recovery path is revising
/// the Controller.
const NO_FURTHER_OPERATION_FAILURE: &str = "No further operation is available at this facility either way. Revise the controller and try again, or return to Signals.";

fn after_action_report_lines(op: &OperationView<'_>) -> Vec<Line<'static>> {
    if after_action_succeeded(op) {
        after_action_success_lines(op)
    } else {
        after_action_failure_lines(op)
    }
}

/// The successful After Action report's content, in the outcome hierarchy
/// order `docs/TUI_DESIGN.md` §5 requires: outcome, trigger/meaning,
/// completion, evidence, then next actions/availability.
fn after_action_success_lines(op: &OperationView<'_>) -> Vec<Line<'static>> {
    let conclusion = op
        .conclusion
        .expect("after_action_succeeded confirmed a Success conclusion");

    let mut lines = vec![Line::from(Span::styled(
        FOOTHOLD_ESTABLISHED_HEADLINE,
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    // Fixed, trusted copy — pushed directly rather than through
    // `bounded_detail_lines` (which exists to cap unbounded *player*-
    // controlled text like Lua error messages) so the pane's `Wrap` widget
    // reflows the full approved sentence at the pane's actual width instead
    // of hard-truncating it at `MAX_DETAIL_LINE_CHARS`.
    lines.push(Line::from(FOOTHOLD_ESTABLISHED_MEANING));
    // A blank line separates outcome/trigger/meaning from completion, and
    // another separates evidence from availability, matching
    // `after_action_failure_lines`'s spacing and `docs/TUI_DESIGN.md` §5's
    // shared outcome-hierarchy sectioning — without either blank line this
    // report reads as one dense block instead of the failure path's clearly
    // separated sections.
    lines.push(Line::from(""));
    lines.push(Line::from(FIRST_CONTACT_COMPLETE));

    lines.push(Line::from(format!(
        "ticks executed     {:02}",
        conclusion.ticks_executed
    )));
    lines.push(Line::from(format!(
        "tiles discovered   {:02}",
        conclusion.tiles_discovered
    )));
    lines.push(Line::from(format!(
        "hazards entered    {:02}",
        conclusion.hazards_entered
    )));
    lines.push(Line::from(format!(
        "remaining budget   {:02}",
        conclusion.final_budget
    )));
    lines.push(Line::from(format!(
        "deployed rev       run-{:02}",
        conclusion.run_id
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(NO_FURTHER_OPERATION));

    lines
}

/// The failure/controller-error After Action report's content, in the
/// outcome hierarchy order `docs/TUI_DESIGN.md` §5 requires: outcome,
/// trigger, meaning, completion, evidence, then next actions/availability —
/// the failure counterpart to [`after_action_success_lines`].
fn after_action_failure_lines(op: &OperationView<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        after_action_headline(op),
        Style::default().add_modifier(Modifier::BOLD),
    ))];

    // `conclusion` is `None` only for an operation that hasn't finished yet
    // (see `OperationView::conclusion`); After Action shouldn't normally be
    // reached in that state, but fall back to the prior, minimal content
    // rather than panic if it is.
    let Some(conclusion) = op.conclusion else {
        lines.extend(bounded_detail_lines(&after_action_detail(op)));
        return lines;
    };

    let diagnostic = match conclusion.kind {
        ConclusionKind::BudgetExhausted => {
            lines.push(Line::from(BUDGET_EXHAUSTED_MEANING));
            None
        }
        ConclusionKind::ControllerError(error) => {
            lines.push(Line::from(CONTROLLER_EXECUTION_STOPPED));
            Some(controller_error_detail(error))
        }
        ConclusionKind::Success => {
            unreachable!("after_action_failure_lines only runs on a non-success conclusion")
        }
    };
    // A blank line separates outcome/trigger/meaning from completion,
    // matching `docs/TUI_DESIGN.md` §5's failure mockup, without adding
    // enough height to push evidence/recovery out of the pane at the
    // console's minimum supported geometry — the `F4` recovery hint already
    // has its own dedicated pinned row (`draw_pane_with_pinned_action`), so
    // this pane's remaining budget is tighter here than on the success path.
    lines.push(Line::from(""));
    lines.push(Line::from(FIRST_CONTACT_INCOMPLETE));

    // The player-controlled diagnostic text is rendered *after* completion,
    // not before it: `bounded_detail_lines` caps logical lines/characters,
    // but not the rows a long line occupies once `Wrap` reflows it at this
    // narrow (~40%-width) pane — an adversarial multi-line error could
    // otherwise push `FIRST CONTACT INCOMPLETE` off the unscrollable pane
    // at the console's minimum supported geometry, which the outcome
    // hierarchy (`docs/TUI_DESIGN.md` §5) never allows.
    if let Some(diagnostic) = diagnostic {
        lines.extend(bounded_detail_lines(&diagnostic));
    }

    lines.push(Line::from(format!(
        "ticks executed     {:02}",
        conclusion.ticks_executed
    )));
    lines.push(Line::from(format!(
        "tiles discovered   {:02}",
        conclusion.tiles_discovered
    )));
    lines.push(Line::from(format!(
        "hazards entered    {:02}",
        conclusion.hazards_entered
    )));
    lines.push(Line::from(format!(
        "remaining budget   {:02}",
        conclusion.final_budget
    )));
    lines.push(Line::from(format!(
        "deployed rev       run-{:02}",
        conclusion.run_id
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(NO_FURTHER_OPERATION_FAILURE));

    lines
}

/// The failure-path headline. Only called from [`after_action_failure_lines`],
/// which [`after_action_report_lines`] only reaches once
/// [`after_action_succeeded`] is `false` — so a successful outcome can't
/// reach this function's `Succeeded` arm in practice.
fn after_action_headline(op: &OperationView<'_>) -> &'static str {
    if let Some(error) = op.error {
        return controller_error_headline(error);
    }
    match op.records.last().map(|record| record.outcome) {
        Some(TickOutcome::Succeeded) => {
            unreachable!("after_action_failure_lines only runs on a non-success outcome")
        }
        Some(outcome) => outcome_headline(outcome),
        None => "OPERATION FAILED",
    }
}

/// The failure-path detail line. See [`after_action_headline`] for why the
/// `Succeeded` arm can't be reached here.
fn after_action_detail(op: &OperationView<'_>) -> String {
    if let Some(error) = op.error {
        return controller_error_detail(error);
    }
    match op.records.last().map(|record| record.outcome) {
        Some(TickOutcome::Succeeded) => {
            unreachable!("after_action_failure_lines only runs on a non-success outcome")
        }
        Some(TickOutcome::Failed(FailureReason::BudgetExhausted)) => {
            "Operational budget exhausted.".to_string()
        }
        Some(TickOutcome::Running) | None => String::new(),
    }
}

/// The satellite feed's content lines: [`render_satellite_view`]'s grid and
/// legend (already built strictly from `snapshot.discovered` — never raw
/// scenario/map internals — so undiscovered terrain can't leak through
/// here), with its own leading title line dropped since the pane's border
/// already carries one.
fn satellite_lines(snapshot: &OperationSnapshot) -> Vec<Line<'static>> {
    let rendered = render_satellite_view(
        snapshot.drone_position,
        snapshot.map_width,
        snapshot.map_height,
        &snapshot.discovered,
    );
    rendered
        .lines()
        .skip(1)
        .map(|line| Line::from(line.to_string()))
        .collect()
}

/// The live Operation telemetry pane's content. Only ever called while the
/// run is still going (`draw_operation` switches to
/// [`review_run_telemetry_lines`] once `op.finished`), so every branch here
/// can assume `op.error` is `None` and the last recorded tick, if any, is
/// still `TickOutcome::Running` — a finished operation always has either an
/// error or a terminal `TickOutcome`, per [`super::state::Operation::is_finished`].
fn telemetry_lines(op: &OperationView<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!("tick          {:02}", op.current.tick)),
        Line::from(format!(
            "budget        {} / {}",
            op.current.budget_remaining, op.starting_budget
        )),
        Line::from(format!(
            "last action   {}",
            op.records
                .last()
                .map(|record| action_label(record.action))
                .unwrap_or("-")
        )),
        Line::from(format!("controller    {}", operation_status_label(op))),
        Line::from(""),
    ];

    lines.push(Line::from(Span::styled(
        "RECENT EVENTS",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let recent = op.records.iter().rev().take(4);
    let mut recent_lines: Vec<Line<'static>> = recent.map(operation_event_line).collect();
    if recent_lines.is_empty() {
        recent_lines.push(Line::from("(none yet)"));
    }
    lines.extend(recent_lines);
    lines.push(Line::from(""));
    lines.push(Line::from(if op.paused {
        "Space resume"
    } else {
        "Space pause"
    }));
    if op.paused {
        lines.push(Line::from("Enter step (paused)"));
    }

    lines
}

/// The finished-run Review Run satellite pane's content: the selected
/// review point's legitimate discovered snapshot, never `op.current`'s
/// scenario-derived fallback (`docs/TUI_DESIGN.md` §5, "Review Run").
///
/// A deploy-time load failure produces an empty `review_points` — no tick
/// ever executed, so there is nothing legitimate to show, and this must
/// never fall back to manufacturing a frame from authoritative scenario
/// state.
fn review_run_satellite_lines(op: &OperationView<'_>) -> Vec<Line<'static>> {
    let Some(point) = selected_review_point(op) else {
        return vec![
            Line::from(Span::styled(
                "NO RECORDED SATELLITE EXECUTION STATE",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Deployment failed before the drone observed anything."),
        ];
    };
    satellite_lines(&point.snapshot)
}

/// The finished-run Review Run telemetry pane's content: the run identity
/// and, for a deploy-time load failure, the failure that stopped execution
/// before any review point existed (`docs/TUI_DESIGN.md` §5, "Review Run"),
/// otherwise the selected review point's evidence via
/// [`review_point_evidence_lines`].
fn review_run_telemetry_lines(op: &OperationView<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "REVIEW RUN",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("deployed rev  run-{:02}", op.run_id)),
    ];

    let Some(point) = selected_review_point(op) else {
        lines.push(Line::from(""));
        if let Some(error) = op.error {
            lines.push(Line::from(Span::styled(
                controller_error_headline(error),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.extend(bounded_detail_lines(&controller_error_detail(error)));
            lines.push(Line::from(""));
        }
        lines.push(Line::from("F4  revise the controller"));
        lines.push(Line::from("F6  redeploy"));
        return lines;
    };

    lines.extend(review_point_evidence_lines(op, point));
    lines
}

/// The currently selected [`ReviewPoint`], or `None` when `op.review_points`
/// is empty (a deploy-time load failure never produced a reviewable point).
/// Falls back to the chronology's terminal point if `op.review_selected`
/// is somehow unset on a non-empty chronology — `AppState::operation`
/// already keeps this `Some` once a run finishes with any review points at
/// all, but this stays defensive rather than panicking on an index.
fn selected_review_point<'a>(op: &'a OperationView<'a>) -> Option<&'a ReviewPoint<'a>> {
    if op.review_points.is_empty() {
        return None;
    }
    let index = op
        .review_selected
        .unwrap_or(op.review_points.len() - 1)
        .min(op.review_points.len() - 1);
    op.review_points.get(index)
}

/// One review point's full evidence, in the order `docs/TUI_DESIGN.md` §5
/// documents for Review Run: run identity (pushed by the caller), point
/// identity, the recorded action (or its truthful absence), resulting
/// position/budget, newly discovered tiles, structured events, and —
/// only on the point that is actually the run's terminal boundary —
/// success/budget-exhaustion/controller-failure evidence, then the frozen
/// deployed source. Pure and independent of `review_selected`/`AppState` so
/// it can be exercised directly against a hand-built [`ReviewPoint`] for
/// every [`ReviewPointKind`], including ones the player cannot navigate to
/// yet (chronology navigation is out of this issue's scope).
fn review_point_evidence_lines(
    op: &OperationView<'_>,
    point: &ReviewPoint<'_>,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(review_point_identity(op, point))];

    lines.push(Line::from(match point.kind {
        ReviewPointKind::Tick(record) => format!("action        {}", action_label(record.action)),
        ReviewPointKind::Initial => "action        (none — pre-tick observation)".to_string(),
        ReviewPointKind::TerminalFailure(_) => {
            "action        (none — execution stopped)".to_string()
        }
    }));
    lines.push(Line::from(format!(
        "position      ({}, {})",
        point.snapshot.drone_position.x, point.snapshot.drone_position.y
    )));
    lines.push(Line::from(format!(
        "budget        {} / {}",
        point.snapshot.budget_remaining, op.starting_budget
    )));
    lines.push(Line::from(format!(
        "discovered    {} new tile(s)",
        point.newly_discovered.len()
    )));

    if let ReviewPointKind::Tick(record) = point.kind {
        let event_lines: Vec<Line<'static>> =
            record.events.iter().filter_map(sim_event_line).collect();
        if !event_lines.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "EVENTS",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.extend(event_lines);
        }
    }
    lines.push(Line::from(""));

    match point.kind {
        ReviewPointKind::Tick(record) if record.outcome != TickOutcome::Running => {
            lines.push(Line::from(Span::styled(
                outcome_headline(record.outcome),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
        }
        ReviewPointKind::TerminalFailure(error) => {
            lines.push(Line::from(Span::styled(
                controller_error_headline(error),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.extend(bounded_detail_lines(&controller_error_detail(error)));
            lines.push(Line::from(""));
        }
        _ => {}
    }

    lines.push(Line::from(Span::styled(
        "DEPLOYED SOURCE",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.extend(bounded_detail_lines(op.deployed_source));
    lines.push(Line::from(""));

    lines.push(Line::from("F4  revise the controller"));
    lines.push(Line::from("F6  redeploy"));

    lines
}

/// A review point's boundary identity, distinguishing the legitimate
/// pre-tick observation, a completed tick, and a controller/runtime failure
/// boundary from each other and — for a failure — from whatever tick last
/// completed before it (`docs/TUI_DESIGN.md` §5, "A runtime/controller
/// failure boundary must clearly distinguish the last completed tick from
/// the failure that stopped execution").
fn review_point_identity(op: &OperationView<'_>, point: &ReviewPoint<'_>) -> String {
    match point.kind {
        ReviewPointKind::Initial => "INITIAL".to_string(),
        ReviewPointKind::Tick(record) => format!("TICK {:02}", record.tick),
        ReviewPointKind::TerminalFailure(_) => match op.records.last() {
            Some(last) => format!("FAILURE (after tick {:02})", last.tick),
            None => "FAILURE (before any tick completed)".to_string(),
        },
    }
}

/// A single structured event's evidence line, or `None` for an event kind
/// already represented by the terminal outcome headline
/// ([`outcome_headline`]) rather than duplicated here.
fn sim_event_line(event: &SimEvent) -> Option<Line<'static>> {
    match event {
        SimEvent::ActionCost { action, amount } => Some(Line::from(format!(
            "  {} — cost {}",
            action_label(*action),
            amount
        ))),
        SimEvent::HazardEntered { amount, .. } => {
            Some(Line::from(format!("  hazard entered — cost {amount}")))
        }
        SimEvent::OperationSucceeded | SimEvent::BudgetExhausted => None,
    }
}

fn operation_event_line(record: &TickRecord) -> Line<'static> {
    let hazard = record
        .events
        .iter()
        .any(|event| matches!(event, SimEvent::HazardEntered { .. }));
    let text = if hazard {
        format!(
            "{:>2}  {} (hazard)",
            record.tick,
            action_label(record.action)
        )
    } else {
        format!("{:>2}  {}", record.tick, action_label(record.action))
    };
    Line::from(text)
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::MoveNorth => "moved north",
        Action::MoveSouth => "moved south",
        Action::MoveEast => "moved east",
        Action::MoveWest => "moved west",
        Action::Wait => "waited",
        Action::Scan => "scanned",
    }
}

fn operation_status_label(op: &OperationView<'_>) -> &'static str {
    if op.error.is_some() {
        return "error";
    }
    match op.records.last().map(|record| record.outcome) {
        Some(TickOutcome::Succeeded) => "succeeded",
        Some(TickOutcome::Failed(_)) => "failed",
        Some(TickOutcome::Running) | None if op.paused => "paused",
        Some(TickOutcome::Running) | None => "running",
    }
}

fn outcome_headline(outcome: TickOutcome) -> &'static str {
    match outcome {
        TickOutcome::Succeeded => FOOTHOLD_ESTABLISHED_HEADLINE,
        TickOutcome::Failed(FailureReason::BudgetExhausted) => "OPERATION FAILED: budget exhausted",
        TickOutcome::Running => "",
    }
}

fn controller_error_headline(error: &ControllerError) -> &'static str {
    // Checked first: a top-level load that ran out of its execution
    // allowance is reported as `ScriptInvalid` (there's no simulation state
    // yet to attach a distinct variant to — see `ControllerError::
    // is_execution_limit`), but it's the same "runaway controller"
    // diagnostic as a callback caught mid-tick, not an ordinary syntax
    // error.
    if error.is_execution_limit() {
        return "OPERATION FAILED: controller execution limit";
    }
    match error {
        ControllerError::ExecutionLimitExceeded => "OPERATION FAILED: controller execution limit",
        ControllerError::InvalidAction(_) => "OPERATION FAILED: invalid controller action",
        ControllerError::ScriptInvalid(_) | ControllerError::MissingCallback => {
            "OPERATION FAILED: controller script error"
        }
        ControllerError::CallbackFailed(_) => "OPERATION FAILED: controller runtime error",
        ControllerError::ScriptUnreadable { .. } => "OPERATION FAILED: controller error",
    }
}

fn controller_error_detail(error: &ControllerError) -> String {
    error.to_string()
}

fn view_title(view: View) -> &'static str {
    match view {
        View::Signals => "SIGNALS",
        View::Target => "TARGET",
        View::Controller => "CONTROLLER",
        View::Operation => "OPERATION",
        View::AfterAction => "AFTER ACTION",
        View::Help => "HELP",
    }
}

/// Prefixes `base` with the non-color focus marker when `focused` is true,
/// so the focused pane's title is identifiable without relying on color
/// (`docs/TUI_DESIGN.md`, "Non-color focus cue").
fn pane_title(base: &str, focused: bool) -> String {
    if focused {
        format!("> {base}")
    } else {
        base.to_string()
    }
}

fn draw_pane(frame: &mut Frame, area: Rect, title: String, lines: Vec<Line<'static>>) {
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Like [`draw_pane`], but pins `action` to a fixed last row instead of
/// letting it sit at the end of the wrapped `lines`. However `lines` wraps
/// at a given pane width — a variable number of known/unknown facts
/// wrapping at a narrow two-pane width, for example — the local action a
/// player needs (`Enter  work this opportunity`, `Esc  back to signals`)
/// must never be the thing that gets pushed off-screen for it.
fn draw_pane_with_pinned_action(
    frame: &mut Frame,
    area: Rect,
    title: String,
    lines: Vec<Line<'static>>,
    action: &'static str,
) {
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [content, action_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), content);
    frame.render_widget(Paragraph::new(action), action_area);
}

fn draw_signals(frame: &mut Frame, area: Rect, state: &AppState) {
    let signal = &authored_signals()[state.selected_signal()];
    let focused = state.focused_pane(View::Signals);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    draw_pane(
        frame,
        left,
        pane_title("SIGNALS", focused == PaneId::SignalsList),
        signal_list_lines(state),
    );
    draw_signal_detail_pane(frame, right, signal, focused == PaneId::SelectedSignal);
}

fn draw_signal_detail_pane(frame: &mut Frame, area: Rect, signal: &Signal, focused: bool) {
    let title = pane_title("SELECTED SIGNAL", focused);
    if signal.is_actionable() {
        draw_pane_with_pinned_action(
            frame,
            area,
            title,
            signal_detail_lines(signal),
            "Enter  inspect opportunity",
        );
    } else {
        draw_pane(frame, area, title, signal_detail_lines(signal));
    }
}

fn signal_origin(signal: &Signal) -> String {
    format!("{} // {}", signal.source, signal.category.label())
}

fn signal_list_lines(state: &AppState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, signal) in authored_signals().iter().enumerate() {
        let cursor = if index == state.selected_signal() {
            "> "
        } else {
            "  "
        };
        let marker = if signal.is_actionable() {
            "  [OPEN]"
        } else {
            ""
        };
        let header_style = if index == state.selected_signal() {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{cursor}{}  {}{marker}", signal.time, signal_origin(signal)),
            header_style,
        )));
        lines.push(Line::from(format!("  {}", signal.headline)));
        lines.push(Line::from(""));
    }
    lines
}

fn signal_detail_lines(signal: &Signal) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            signal_origin(signal),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(signal.body),
    ];
    if signal.is_actionable() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "ACTIONABLE: FIRST CONTACT",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    lines
}

fn draw_target(frame: &mut Frame, area: Rect, state: &AppState) {
    let dossier = first_contact_dossier();
    let focused = state.focused_pane(View::Target);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
    draw_pane_with_pinned_action(
        frame,
        left,
        pane_title("TARGET INTELLIGENCE", focused == PaneId::TargetIntelligence),
        target_intel_lines(&dossier),
        "Enter  work this opportunity",
    );
    draw_pane_with_pinned_action(
        frame,
        right,
        pane_title("PROVENANCE / ACCESS", focused == PaneId::Provenance),
        target_provenance_lines(&dossier),
        "Esc  back to signals",
    );
}

/// Content for the TARGET INTELLIGENCE pane, excluding the pinned
/// `Enter  work this opportunity` action row: however this wraps at a given
/// pane width, that action always renders on its own fixed last row instead
/// of at the tail of this list, so it can't be pushed off-screen.
fn target_intel_lines(dossier: &TargetDossier) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            dossier.title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(dossier.location),
        Line::from(Span::styled(
            "KNOWN",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    for fact in dossier.known {
        lines.push(Line::from(format!("- {fact}")));
    }
    lines.push(Line::from(Span::styled(
        "UNKNOWN",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for fact in dossier.unknown {
        lines.push(Line::from(format!("- {fact}")));
    }
    lines.push(Line::from(dossier.opportunity));
    lines
}

fn target_provenance_lines(dossier: &TargetDossier) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "SOURCE",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    for source in dossier.source {
        lines.push(Line::from(*source));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "ACCESS",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for access in dossier.access {
        lines.push(Line::from(*access));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "CONFIDENCE",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (fact, level) in dossier.confidence {
        lines.push(Line::from(format!("{fact}   {level}")));
    }
    lines
}

/// The Help pane's inner (content) dimensions for a given full frame size,
/// matching the header/footer/border geometry [`draw`] and [`draw_help`]
/// actually use. Exposed so callers outside this module (the event loop)
/// can bound `help_scroll` against the real viewport without duplicating
/// that layout math.
pub(crate) fn help_inner_dimensions(frame_width: u16, frame_height: u16) -> (u16, u16) {
    if frame_width < MIN_COLUMNS || frame_height < MIN_ROWS {
        return (0, 0);
    }
    const HEADER_AND_FOOTER_HEIGHT: u16 = 6; // draw()'s two Length(3) rows
    const BORDER_INSET: u16 = 2; // draw_help()'s own Block::borders(ALL)
    let body_height = frame_height.saturating_sub(HEADER_AND_FOOTER_HEIGHT);
    (
        frame_width.saturating_sub(BORDER_INSET),
        body_height.saturating_sub(BORDER_INSET),
    )
}

/// How far `help_scroll` can advance before it would scroll past the last
/// rendered row of Help's content at this frame size.
pub(crate) fn help_max_scroll(state: &AppState, frame_width: u16, frame_height: u16) -> u16 {
    let (content_width, content_height) = help_inner_dimensions(frame_width, frame_height);
    let content_rows = wrapped_row_count(&help_lines(state), content_width);
    content_rows.saturating_sub(content_height as usize) as u16
}

/// The AFTER-ACTION REPORT pane's inner (content) dimensions for a given
/// full frame size, matching the header/footer/border/two-pane-split
/// geometry [`draw`] and [`draw_after_action`] actually use — the same role
/// [`help_inner_dimensions`] plays for Help. Reuses the exact `Layout` split
/// `draw_after_action` renders with, rather than reimplementing the
/// percentage math, so this can't drift from the real render.
pub(crate) fn after_action_report_inner_dimensions(
    frame_width: u16,
    frame_height: u16,
) -> (u16, u16) {
    if frame_width < MIN_COLUMNS || frame_height < MIN_ROWS {
        return (0, 0);
    }
    const HEADER_AND_FOOTER_HEIGHT: u16 = 6; // draw()'s two Length(3) rows
    const BORDER_INSET: u16 = 2; // the report pane's own Block::borders(ALL)
    let body_height = frame_height.saturating_sub(HEADER_AND_FOOTER_HEIGHT);

    let [_, right] = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
        .areas(Rect::new(0, 0, frame_width, body_height));
    let pane_width = right.width;

    (
        pane_width.saturating_sub(BORDER_INSET),
        body_height.saturating_sub(BORDER_INSET),
    )
}

/// How far the After Action report's scroll offset can advance before it
/// would scroll past the last rendered row of its content at this frame
/// size, accounting for the failure report's pinned `F4` recovery row
/// (`draw_after_action_report_pane`). `0` once no operation has concluded,
/// since there's nothing to scroll.
pub(crate) fn after_action_max_scroll(
    state: &AppState,
    frame_width: u16,
    frame_height: u16,
) -> u16 {
    let Some(op) = state.operation() else {
        return 0;
    };
    let (content_width, content_height) =
        after_action_report_inner_dimensions(frame_width, frame_height);
    let pinned_action_row = u16::from(!after_action_succeeded(&op));
    let content_rows = wrapped_row_count(&after_action_report_lines(&op), content_width);
    content_rows.saturating_sub(content_height.saturating_sub(pinned_action_row) as usize) as u16
}

fn draw_help(frame: &mut Frame, area: Rect, state: &AppState) {
    // Help has exactly one `PaneId` and `F8` is a no-op here, so
    // `focus_movement_available` is always `false` for it — the marker
    // never shows, matching the product decision that Help not imply a
    // focus choice that doesn't exist.
    let block = Block::default().borders(Borders::ALL).title(pane_title(
        view_title(View::Help),
        state.focus_movement_available(View::Help),
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = help_lines(state);
    // The event loop already clamps the stored offset via `help_max_scroll`
    // as each scroll key arrives; this recomputes against the exact `inner`
    // Rect as a second, render-time backstop (e.g. for the very first draw,
    // or a caller that renders without going through that event loop).
    let content_rows = wrapped_row_count(&lines, inner.width);
    let max_scroll = content_rows.saturating_sub(inner.height as usize) as u16;
    let scroll = state.scroll_offset(PaneId::Help).min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);
}

/// How many terminal rows `lines` occupies once wrapped to `width`, matching
/// the `Wrap { trim: false }` behavior used to render Help and the
/// Controller banner.
fn wrapped_row_count(lines: &[Line<'static>], width: u16) -> usize {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            word_wrapped_row_count(&text, width)
        })
        .sum()
}

/// How many rows `text` occupies once word-wrapped to `width` columns,
/// matching ratatui's own greedy word-wrapping with `Wrap { trim: false }`
/// (never splitting a word across rows unless the word alone is wider than
/// `width`, and preserving whitespace runs rather than collapsing them). A
/// word and the whitespace run immediately before it move together as one
/// unit — the same grouping `Wrap { trim: false }` uses — so e.g. a
/// 70-column word, 10 columns of spaces, and a short recovery word in a
/// 78-column pane wrap to two rows (word alone; then the 10 spaces plus the
/// recovery word, which can't fit on the first row) rather than the one row
/// a naive total-width division would predict.
fn word_wrapped_row_count(text: &str, width: usize) -> usize {
    let width = width.max(1);
    let mut rows = 0usize;
    let mut current_width = 0usize; // 0 means the row being built is empty
    let mut chars = text.chars().peekable();
    let mut saw_content = false;

    loop {
        // Measured via `CellWidth` (the same calculation ratatui's own
        // renderer uses), not a per-character `unicode-width` sum: they can
        // disagree for character combinations `CellWidth` treats specially
        // (see `display_width_of_prefix`), which could otherwise reserve
        // too few rows for a banner and clip its final word.
        let mut whitespace = String::new();
        while let Some(&c) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            whitespace.push(c);
            chars.next();
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            word.push(c);
            chars.next();
        }
        if whitespace.is_empty() && word.is_empty() {
            break; // end of text
        }
        saw_content = true;

        let segment_width =
            whitespace.as_str().cell_width() as usize + word.as_str().cell_width() as usize;
        if segment_width >= width {
            // The whitespace-plus-word segment alone doesn't fit a row and
            // hard-wraps across as many rows as it needs, the same way a
            // single overlong word does.
            if current_width > 0 {
                rows += 1;
            }
            rows += segment_width / width;
            current_width = segment_width % width;
            continue;
        }
        if current_width + segment_width <= width {
            current_width += segment_width;
        } else {
            rows += 1;
            current_width = segment_width;
        }
    }
    if current_width > 0 {
        rows += 1;
    }
    if !saw_content {
        rows = rows.max(1);
    }
    rows.max(1)
}

/// Two-level contextual help: the current (or Help-opened-from) view's
/// controls first, then the complete Lua contract below, reachable by
/// scrolling. See `docs/TUI_DESIGN.md`, "6. Help".
fn help_lines(state: &AppState) -> Vec<Line<'static>> {
    let context_view = state.help_return_view().unwrap_or(View::Signals);

    let mut lines = vec![Line::from(Span::styled(
        "This view",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(view_specific_help(context_view));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Scrolling",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(
        "Up/Down  scroll this pane for the Lua reference, terminology, and symbols below",
    ));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Global controls",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from("F1 Help          toggle this overlay"));
    lines.push(Line::from("F2 Signals       the intelligence stream"));
    lines.push(Line::from(if state.view_available(View::Target) {
        "F3 Target        dossier for the current opportunity"
    } else {
        "F3 Target        (unavailable: inspect the [OPEN] signal first)"
    }));
    lines.push(Line::from(if state.view_available(View::Controller) {
        "F4 Controller    the Lua editor for the working set"
    } else {
        "F4 Controller    (unavailable: work an opportunity from Target first)"
    }));
    lines.push(Line::from(if state.view_available(View::Operation) {
        "F5 Operation     the live satellite/telemetry view"
    } else {
        "F5 Operation     (unavailable: work an opportunity from Target first)"
    }));
    lines.push(Line::from(if state.controller_source().is_some() {
        "F6 Deploy        run the current controller (confirms if a run is active)"
    } else {
        "F6 Deploy        (unavailable: work an opportunity from Target first)"
    }));
    lines.push(Line::from(if state.view_available(View::Controller) {
        "F7 Reset         (Controller) restore the starter controller"
    } else {
        "F7 Reset         (Controller, unavailable: work an opportunity first)"
    }));
    lines.push(Line::from(
        "Ctrl+V           (Controller) load the source and check for on_tick,",
    ));
    lines.push(Line::from(
        "                 without calling on_tick itself (top-level code",
    ));
    lines.push(Line::from(
        "                 outside on_tick does run, e.g. local state setup)",
    ));
    lines.push(Line::from("Ctrl+Q Quit      exit and restore the terminal"));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Lua contract",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(
        "Define a global on_tick(observation) function. It runs once per",
    ));
    lines.push(Line::from("tick and must return the name of one action."));
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Available standard libraries: table, string, math. Player Lua is",
    ));
    lines.push(Line::from(
        "untrusted input, so io, os, package, coroutine, debug, load, dofile,",
    ));
    lines.push(Line::from(
        "and loadfile are not available; scripts using them will fail to load.",
    ));
    lines.push(Line::from(
        "print is not available either (it would corrupt the console's own",
    ));
    lines.push(Line::from("display)."));
    lines.push(Line::from(
        "math.random always starts from the same fixed seed, so a controller",
    ));
    lines.push(Line::from(
        "using it behaves identically on every deployment; math.randomseed is",
    ));
    lines.push(Line::from(
        "not available (it would undo that determinism if a script called it).",
    ));
    lines.push(Line::from(
        "string.pack, unpack, packsize, and dump are also not available (they",
    ));
    lines.push(Line::from(
        "expose native platform layout); nor is collectgarbage.",
    ));
    lines.push(Line::from(
        "pairs/next only iterate tables keyed by booleans, numbers, or",
    ));
    lines.push(Line::from(
        "strings; a table or function key makes the whole traversal fail.",
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(
        "observation.drone.x / .y      the drone's current position",
    ));
    lines.push(Line::from(
        "observation.tick              the current tick number",
    ));
    lines.push(Line::from(
        "observation.budget_remaining  the operational budget left",
    ));
    lines.push(Line::from(
        "observation.discovered[]      tiles learned about so far, each",
    ));
    lines.push(Line::from(
        "                               {x, y, tile, traversable, uplink}",
    ));
    lines.push(Line::from(
        "  tile          \"floor\", \"wall\", or \"hazard\"",
    ));
    lines.push(Line::from(
        "  traversable   whether the drone could occupy that tile",
    ));
    lines.push(Line::from(
        "  uplink        whether it's the network-uplink objective",
    ));
    lines.push(Line::from(
        "the drone's own tile and its four cardinal neighbours are added",
    ));
    lines.push(Line::from(
        "automatically every tick; nothing farther is visible until",
    ));
    lines.push(Line::from("discovered (e.g. by a scan)"));
    lines.push(Line::from(""));
    lines.push(Line::from(
        "valid return values: north south east west wait scan",
    ));
    lines.push(Line::from(
        "any other value, a move off the map, or a move into a wall ends",
    ));
    lines.push(Line::from(
        "the run with an error and does not consume budget",
    ));
    lines.push(Line::from(format!(
        "each action costs {} budget; entering a hazard tile costs {} more,",
        crate::simulation::ACTION_COST,
        crate::simulation::HAZARD_ENTRY_COST
    )));
    lines.push(Line::from("charged only on the tick the drone enters it"));
    lines.push(Line::from(
        "scan does not move the drone; it reveals every tile within 2 tiles",
    ));
    lines.push(Line::from(
        "in any direction (5x5), regardless of walls in the way",
    ));
    lines.push(Line::from(
        "discoveries, whether passive or from a scan, persist for the run",
    ));
    lines.push(Line::from(
        "the operation fails if budget runs out before reaching the uplink;",
    ));
    lines.push(Line::from(
        "reaching it always succeeds, even on the action that would have",
    ));
    lines.push(Line::from("exhausted the budget"));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Terminology",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(
        "working set     the opportunity you are currently working, if any",
    ));
    lines.push(Line::from(
        "MACHINE INTERCEPT   traffic captured from compromised infrastructure",
    ));
    lines.push(Line::from(
        "SHARED INTEL        a fragment another resistance hacker chose to pass on",
    ));
    lines.push(Line::from(
        "REQUEST             another cell asking for help; not addressed to you",
    ));
    lines.push(Line::from(
        "ANOMALY              unexplained activity from a passive sensor",
    ));
    lines.push(Line::from(
        "[OPEN]               this signal currently has a workable opportunity",
    ));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Satellite symbols",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from("D drone   U uplink   . floor"));
    lines.push(Line::from("# wall    ~ hazard   ? undiscovered"));

    lines
}

fn view_specific_help(view: View) -> Vec<Line<'static>> {
    match view {
        View::Signals => vec![
            Line::from("Up/Down  move the selection"),
            Line::from("Enter    open Target (only signals marked [OPEN] respond)"),
            Line::from("Its detail shows alongside the list automatically;"),
            Line::from("F8 moves focus there"),
        ],
        View::Target => vec![
            Line::from("Enter  work this opportunity"),
            Line::from("Esc    back to Signals"),
            Line::from("F8     move focus between intel and provenance"),
        ],
        View::Controller => vec![
            Line::from("Type to edit; arrows/Home/End/PageUp/PageDown move the cursor"),
            Line::from(
                "Shift+move select, Ctrl+A select all, Ctrl+Z/Ctrl+Y undo/redo, Ctrl+Left/Right by word",
            ),
            Line::from("Tab/Shift+Tab indent/unindent"),
            Line::from("F7          reset to the starter controller (confirms if modified)"),
            Line::from("Ctrl+V      load the source and check for on_tick, without calling it"),
            Line::from("F8          move focus between source and reference"),
        ],
        View::Operation => vec![
            Line::from("F6     deploy the current controller (confirms if a run is active)"),
            Line::from("Space  pause/resume the run"),
            Line::from("Enter  advance exactly one tick while paused"),
            Line::from("F8     move focus between feed and telemetry"),
            Line::from("Leaving via F2/F3/F4 pauses the run; F5 returns to it as you left it."),
        ],
        View::AfterAction => vec![
            Line::from("Up/Down  scroll the report if it doesn't fully fit"),
            Line::from("F2       back to Signals"),
            Line::from("F4       edit the controller (your edits are preserved)"),
            Line::from("F5       review this run's frozen source and telemetry (Review Run)"),
            Line::from("F6       redeploy from a clean scenario state"),
            Line::from("F8       move focus between frame and report"),
        ],
        View::Help => Vec::new(),
    }
}

/// `(full label, compact label, enabled)` for each footer hint. The compact
/// form is used whenever the full labels would crowd out the `Ctrl+Q Quit`
/// hint, which must always stay visible.
fn footer_hint_items(state: &AppState, show_f8: bool) -> Vec<(&'static str, &'static str, bool)> {
    // In After Action, F5 returns to Operation to inspect the finished
    // run's frozen telemetry rather than "the" operation view in general,
    // so it's relabeled "Review Run" there (`docs/TUI_DESIGN.md` §5).
    let f5_item = if state.current_view() == View::AfterAction {
        (
            "F5 Review Run",
            "F5 Rvw",
            state.view_available(View::Operation),
        )
    } else {
        (
            "F5 Operation",
            "F5 Op",
            state.view_available(View::Operation),
        )
    };
    let mut items = vec![
        ("F1 Help", "F1 Help", true),
        ("F2 Signals", "F2 Sig", true),
        ("F3 Target", "F3 Tgt", state.view_available(View::Target)),
        (
            "F4 Controller",
            "F4 Ctl",
            state.view_available(View::Controller),
        ),
        f5_item,
        deploy_footer_item(state),
    ];
    if state.current_view() == View::Controller {
        let has_controller = state.controller_source().is_some();
        items.push(("F7 Reset", "F7 Rst", has_controller));
        items.push(("Ctrl+V Validate", "^V Val", has_controller));
    }
    if state.current_view() == View::Operation
        && let Some(op) = state.operation()
        && !op.finished
    {
        items.push(("Space Pause/Resume", "Spc P/R", true));
        if op.paused {
            items.push(("Enter Step", "Ent Step", true));
        }
    }
    if show_f8 {
        items.push(("F8 Next Pane", "F8 Pane", true));
    }
    items
}

/// `F6`'s footer label: "Deploy" before anything has been deployed yet (or
/// once a working set exists but nothing's running), "Redeploy" once an
/// operation exists — either wording is enabled exactly when there's a
/// controller source to deploy.
fn deploy_footer_item(state: &AppState) -> (&'static str, &'static str, bool) {
    let enabled = state.controller_source().is_some();
    if state.operation().is_some() {
        ("F6 Redeploy", "F6 Rdp", enabled)
    } else {
        ("F6 Deploy", "F6 Dep", enabled)
    }
}

const FOOTER_RIGHT_HINT: &str = "Ctrl+Q Quit";

/// Lays out `labels` (already chosen enabled/dimmed) against `inner_width`,
/// keeping `FOOTER_RIGHT_HINT` visible whenever the labels leave room for it.
fn footer_line(labels: &[(&'static str, bool)], inner_width: usize) -> Line<'static> {
    let mut spans = Vec::with_capacity(labels.len() * 2 + 2);
    let mut left_len = 0usize;
    for (index, (label, enabled)) in labels.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
            left_len += 1;
        }
        let style = if *enabled {
            Style::default()
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        spans.push(Span::styled(*label, style));
        left_len += label.len();
    }

    let used = left_len + 1 + FOOTER_RIGHT_HINT.len();
    if used <= inner_width {
        spans.push(Span::raw(" ".repeat(inner_width - used)));
        spans.push(Span::raw(FOOTER_RIGHT_HINT));
    }
    // If even the left-hand labels alone don't fit, the quit hint is
    // dropped rather than truncated into something misleading; this only
    // happens below the widths this console otherwise supports.

    Line::from(spans)
}

fn footer_line_width(labels: &[(&'static str, bool)]) -> usize {
    labels.iter().map(|(label, _)| label.len()).sum::<usize>() + labels.len().saturating_sub(1)
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    // `F8` moves focus at every supported width, not just when it also
    // changes what's visible (`docs/TUI_DESIGN.md`, "F8 -- next pane"), so
    // the hint shows regardless of frame width — but only when the
    // currently rendered surface actually presents a multi-pane choice
    // (`AppState::focus_movement_available` is also what gates `F8` itself
    // in `event::map`, so the hint and the key can't disagree).
    let show_f8 = state.focus_movement_available(state.current_view());
    let items = footer_hint_items(state, show_f8);
    let inner_width = area.width.saturating_sub(2) as usize;

    let full: Vec<(&'static str, bool)> = items
        .iter()
        .map(|(full, _, enabled)| (*full, *enabled))
        .collect();
    let compact: Vec<(&'static str, bool)> = items
        .iter()
        .map(|(_, compact, enabled)| (*compact, *enabled))
        .collect();

    // Prefer full labels; fall back to compact ones once the quit hint
    // would otherwise be crowded out at the narrower supported widths.
    let labels = if footer_line_width(&full) + 1 + FOOTER_RIGHT_HINT.len() <= inner_width {
        &full
    } else {
        &compact
    };

    frame.render_widget(
        Paragraph::new(footer_line(labels, inner_width))
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(width: u16, height: u16, state: &AppState) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        terminal
            .draw(|frame| draw(frame, state))
            .expect("draw should succeed");
        terminal
    }

    fn buffer_contains(terminal: &Terminal<TestBackend>, needle: &str) -> bool {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains(needle)
    }

    #[test]
    fn wide_signals_view_shows_all_authored_signals_and_one_open_marker() {
        let state = AppState::new();
        let terminal = render(120, 40, &state);

        for signal in authored_signals() {
            assert!(buffer_contains(&terminal, signal.source));
        }
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(content.matches("[OPEN]").count(), 1);
    }

    #[test]
    fn target_dossier_never_renders_hidden_scenario_state() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "TARGET INTELLIGENCE"));
        assert!(buffer_contains(&terminal, "uplink location"));
        assert!(!buffer_contains(&terminal, "4, 4"));
        assert!(!buffer_contains(&terminal, "(4,4)"));
    }

    #[test]
    fn help_shows_context_controls_and_lua_reference() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "This view"));
        assert!(buffer_contains(
            &terminal,
            "only signals marked [OPEN] respond"
        ));
        assert!(buffer_contains(&terminal, "Global controls"));
        assert!(buffer_contains(&terminal, "on_tick(observation)"));
    }

    #[test]
    fn help_documents_the_restricted_lua_standard_library() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 60, &state);

        assert!(buffer_contains(&terminal, "table, string, math"));
        assert!(buffer_contains(&terminal, "io, os, package"));
        assert!(buffer_contains(&terminal, "dofile"));
        assert!(buffer_contains(&terminal, "math.randomseed"));
    }

    #[test]
    fn help_advertises_its_own_scrolling_at_the_supported_minimum_geometry() {
        use super::super::state::Msg;

        // The Lua/terminology/symbol sections only fit below the fold at
        // the supported minimum, so the hint that Up/Down reaches them
        // must itself be visible without scrolling.
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "Up/Down  scroll this pane"));
    }

    #[test]
    fn help_title_never_carries_the_focus_marker() {
        use super::super::state::Msg;

        // Help has exactly one pane and was never meant to imply a focus
        // choice that doesn't exist (`F8` is already a no-op there, per
        // `AppState::focus_movement_available`) — the title renders bare,
        // never `> HELP`.
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        state.apply(Msg::FocusNextPane);

        for (width, height) in [(MIN_COLUMNS, MIN_ROWS), (120, 40)] {
            let terminal = render(width, height, &state);
            assert!(buffer_contains(&terminal, "HELP"));
            assert!(!buffer_contains(&terminal, "> HELP"));
        }
    }

    #[test]
    fn signals_view_shows_both_panes_and_moves_the_focus_marker_with_f8() {
        let state = AppState::new();
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "SIGNALS"));
        assert!(buffer_contains(&terminal, "SELECTED SIGNAL"));
        assert!(buffer_contains(&terminal, "> SIGNALS"));
        assert!(!buffer_contains(&terminal, "> SELECTED SIGNAL"));
    }

    #[test]
    fn signals_focus_survives_a_resize() {
        use super::super::state::Msg;

        // Focus itself is `AppState` state, entirely independent of the
        // frame width `render` is called with — so "resizing" here is
        // simply rendering the same state at different widths, mirroring
        // what `should_redraw`'s `Event::Resize` arm (which no longer
        // touches pane state at all) actually preserves.
        let mut state = AppState::new();
        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);

        let small = render(120, 40, &state);
        assert!(buffer_contains(&small, "SIGNALS"));
        assert!(buffer_contains(&small, "SELECTED SIGNAL"));
        assert!(buffer_contains(&small, "> SELECTED SIGNAL"));
        assert!(!buffer_contains(&small, "> SIGNALS"));

        let large = render(150, 50, &state);
        assert!(buffer_contains(&large, "SELECTED SIGNAL"));
        assert!(buffer_contains(&large, "> SELECTED SIGNAL"));

        let small_again = render(120, 40, &state);
        assert!(buffer_contains(&small_again, "SIGNALS"));
        assert!(buffer_contains(&small_again, "SELECTED SIGNAL"));
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);
    }

    #[test]
    fn target_view_at_minimum_supported_geometry_still_shows_the_enter_hint() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "work this opportunity"));
    }

    #[test]
    fn target_view_at_minimum_supported_geometry_shows_both_pinned_actions() {
        use super::super::state::Msg;

        // At the console's real minimum, the 55% intelligence pane wraps
        // enough known facts and the opportunity blurb that the action row
        // would be the first thing pushed off-screen if it weren't pinned
        // separately.
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "work this opportunity"));
        assert!(buffer_contains(&terminal, "back to signals"));
    }

    #[test]
    fn target_focus_marker_moves_between_panes_and_survives_a_resize() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        assert_eq!(state.focused_pane(View::Target), PaneId::TargetIntelligence);

        let small = render(120, 40, &state);
        assert!(buffer_contains(&small, "> TARGET INTELLIGENCE"));
        assert!(!buffer_contains(&small, "> PROVENANCE / ACCESS"));

        state.apply(Msg::FocusNextPane);
        assert_eq!(state.focused_pane(View::Target), PaneId::Provenance);

        let small = render(120, 40, &state);
        assert!(buffer_contains(&small, "> PROVENANCE / ACCESS"));
        assert!(!buffer_contains(&small, "> TARGET INTELLIGENCE"));

        let large = render(150, 50, &state);
        assert!(buffer_contains(&large, "PROVENANCE / ACCESS"));
        assert!(buffer_contains(&large, "> PROVENANCE / ACCESS"));

        let small_again = render(120, 40, &state);
        assert!(buffer_contains(&small_again, "> PROVENANCE / ACCESS"));
        assert_eq!(state.focused_pane(View::Target), PaneId::Provenance);
    }

    #[test]
    fn signal_detail_pane_at_minimum_supported_geometry_still_shows_the_pinned_action() {
        let state = AppState::new();
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "inspect opportunity"));
    }

    #[test]
    fn footer_keeps_the_quit_hint_visible_at_minimum_width() {
        let state = AppState::new();
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "Ctrl+Q Quit"));
    }

    #[test]
    fn footer_keeps_the_quit_hint_visible_in_controller_at_minimum_width() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(
            buffer_contains(&terminal, "Ctrl+Q Quit"),
            "Controller's extra F7/Ctrl+V hints must not crowd out the global quit hint"
        );
    }

    #[test]
    fn controller_footer_and_help_advertise_ctrl_v_not_ctrl_enter() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        let terminal = render(200, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "Ctrl+V Validate"));
        assert!(!buffer_contains(&terminal, "Ctrl+Enter"));

        state.apply(Msg::OpenHelp);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "Ctrl+V"));
        assert!(!buffer_contains(&terminal, "Ctrl+Enter"));
    }

    #[test]
    fn header_keeps_working_set_visible_at_minimum_width() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::Navigate(View::Target));
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "WORKING SET: FIRST CONTACT"));
    }

    #[test]
    fn target_header_keeps_link_condition_at_minimum_width_before_and_after_commitment() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            buffer_contains(&terminal, "MESH: DEGRADED"),
            "Target should keep link condition before committing, like every other view"
        );

        state.apply(Msg::Activate);
        state.apply(Msg::Navigate(View::Target));
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            buffer_contains(&terminal, "MESH: DEGRADED"),
            "Target should keep link condition after committing too"
        );
    }

    #[test]
    fn help_documents_discovered_tile_field_meanings() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 60, &state);

        assert!(buffer_contains(
            &terminal,
            "\"floor\", \"wall\", or \"hazard\""
        ));
        assert!(buffer_contains(&terminal, "whether the drone could occupy"));
        assert!(buffer_contains(&terminal, "network-uplink objective"));
        assert!(buffer_contains(
            &terminal,
            "own tile and its four cardinal neighbours are added"
        ));
    }

    #[test]
    fn help_documents_action_mechanics() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 71, &state);

        assert!(buffer_contains(&terminal, "scan does not move the drone"));
        assert!(buffer_contains(&terminal, "regardless of walls in the way"));
        assert!(buffer_contains(&terminal, "persist for the run"));
        assert!(buffer_contains(&terminal, "reaching it always succeeds"));
        assert!(buffer_contains(
            &terminal,
            "move off the map, or a move into a wall"
        ));
        assert!(buffer_contains(&terminal, "does not consume budget"));
    }

    #[test]
    fn signals_help_describes_focus_movement_accurately() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 40, &state);

        assert!(!buffer_contains(&terminal, "follows automatically"));
        assert!(buffer_contains(&terminal, "F8 moves focus there"));
    }

    #[test]
    fn help_lists_signal_terminology_and_satellite_symbols() {
        use super::super::state::Msg;

        // Tall enough that the full two-level help content (view controls,
        // global controls, Lua contract, terminology, symbol legend) fits
        // without needing to scroll.
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 83, &state);

        assert!(buffer_contains(&terminal, "Terminology"));
        assert!(buffer_contains(&terminal, "MACHINE INTERCEPT"));
        assert!(buffer_contains(&terminal, "Satellite symbols"));
        assert!(buffer_contains(&terminal, "D drone"));
    }

    #[test]
    fn help_at_a_tall_viewport_does_not_scroll_past_its_own_content() {
        use super::super::state::Msg;

        // At this height the full Help document already fits without
        // scrolling; MAX_PANE_SCROLL alone would still let repeated Down
        // presses blank the pane, so the render-time clamp must catch it.
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        for _ in 0..60 {
            state.apply(Msg::ScrollDown);
        }
        let terminal = render(120, 60, &state);

        assert!(buffer_contains(&terminal, "Satellite symbols"));
        assert!(buffer_contains(&terminal, "? undiscovered"));
    }

    #[test]
    fn controller_view_shows_the_seeded_starter_source() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "function on_tick(observation)"));
        assert!(buffer_contains(&terminal, "CAPTURED CONTROLLER"));
    }

    #[test]
    fn controller_header_shows_the_seeded_controller_state() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "CONTROLLER: starter"));
    }

    #[test]
    fn controller_source_renders_at_true_minimum_geometry() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "CAPTURED CONTROLLER"));
        assert!(buffer_contains(&terminal, "function on_tick(observation)"));
    }

    #[test]
    fn the_terminal_cursor_is_shown_inside_the_source_pane_when_it_is_focused() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        // Controller source is the default focused pane on first entry.
        let terminal = render(120, 40, &state);

        assert!(terminal.backend().cursor_visible());
        let cursor = terminal.backend().cursor_position();
        // The source pane occupies the left ~70% of the frame, below the
        // header row and inside the bordered block — a loose bound is
        // enough to prove the cursor lands inside the source content, not
        // that it exactly reproduces the widget's own gutter math.
        assert!(cursor.x > 0 && cursor.x < 90, "cursor.x = {}", cursor.x);
        assert!(cursor.y > 1 && cursor.y < 40, "cursor.y = {}", cursor.y);
    }

    #[test]
    fn the_terminal_cursor_is_hidden_when_the_reference_pane_is_focused_instead() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::FocusNextPane); // source -> reference
        let terminal = render(120, 40, &state);

        assert!(
            !terminal.backend().cursor_visible(),
            "the source pane's cursor must not be shown while it isn't focused"
        );
    }

    #[test]
    fn controller_status_stays_visible_after_navigating_away() {
        use super::super::state::Msg;
        use super::super::state::View;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::Navigate(View::Signals));
        let terminal = render(120, 40, &state);

        assert!(
            buffer_contains(&terminal, "CONTROLLER: modified"),
            "a session-only edit at risk of being lost must stay visible from Signals too"
        );
    }

    #[test]
    fn header_shows_status_ready_after_a_successful_validation_from_any_view() {
        use super::super::state::Msg;
        use super::super::state::View;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::ValidateController);
        state.apply(Msg::Navigate(View::Signals));
        let terminal = render(120, 40, &state);

        assert!(
            buffer_contains(&terminal, "STATUS: READY"),
            "docs/TUI_DESIGN.md's persistent header shows STATUS: READY \
             alongside CONTROLLER: ..., not only inside Controller's own banner"
        );
    }

    #[test]
    fn quit_confirmation_is_visible_from_every_view_not_only_controller() {
        use super::super::state::Msg;
        use super::super::state::View;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::Navigate(View::Signals));
        state.apply(Msg::RequestQuit);
        let terminal = render(120, 40, &state);

        assert!(state.quit_confirmation_pending());
        assert!(buffer_contains(
            &terminal,
            "Modified controller source will be lost."
        ));
    }

    #[test]
    fn quit_confirmation_is_visible_even_below_the_supported_minimum_geometry() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::RequestQuit);
        let terminal = render(60, 20, &state);

        assert!(buffer_contains(
            &terminal,
            "Modified controller source will be lost."
        ));
    }

    #[test]
    fn quit_confirmation_keeps_its_confirm_and_cancel_keys_visible_at_very_short_heights() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::RequestQuit);

        for height in [1, 2, 3, 5] {
            let terminal = render(60, height, &state);
            let visible = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                visible.contains("Enter") && visible.contains("Esc"),
                "at height {height}, both the confirm (Enter) and cancel (Esc) \
                 keys should stay visible, got: {visible:?}"
            );
        }
    }

    #[test]
    fn reset_confirmation_is_visible_even_when_the_reference_pane_was_focused() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::FocusNextPane); // move focus to the Lua reference pane
        state.apply(Msg::RequestResetController);
        let terminal = render(120, 40, &state);

        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::LuaFieldReference
        );
        assert!(state.reset_confirmation_pending());
        assert!(buffer_contains(
            &terminal,
            "Reset controller? Edits will be lost."
        ));

        // The confirmation's banner and Enter/Esc prompt render in the
        // source pane without moving focus off the reference pane — the
        // marker must track the stored focus, not which pane the banner
        // happens to render in, so the source stays unmarked here.
        assert!(!buffer_contains(&terminal, "> CAPTURED CONTROLLER"));

        state.apply(Msg::CancelResetController);
        let after_cancel = render(120, 40, &state);
        assert!(buffer_contains(&after_cancel, "> LUA FIELD REFERENCE"));
        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::LuaFieldReference
        );
    }

    #[test]
    fn reset_confirmation_hides_the_f8_hint() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::RequestResetController);
        assert!(state.reset_confirmation_pending());

        for (width, height) in [(120, 40), (150, 50)] {
            let terminal = render(width, height, &state);
            assert!(!buffer_contains(&terminal, "F8 Next Pane"));
            assert!(!buffer_contains(&terminal, "F8 Pane"));
        }
    }

    #[test]
    fn controller_focus_survives_a_resize() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::FocusNextPane); // move focus to the Lua reference pane

        let small = render(120, 40, &state);
        assert!(buffer_contains(&small, "CAPTURED CONTROLLER"));
        assert!(buffer_contains(&small, "LUA FIELD REFERENCE"));
        assert!(buffer_contains(&small, "> LUA FIELD REFERENCE"));
        assert!(!buffer_contains(&small, "> CAPTURED CONTROLLER"));

        let large = render(150, 50, &state);
        assert!(buffer_contains(&large, "LUA FIELD REFERENCE"));
        assert!(buffer_contains(&large, "> LUA FIELD REFERENCE"));
        assert!(!buffer_contains(&large, "> CAPTURED CONTROLLER"));

        let small_again = render(120, 40, &state);
        assert!(buffer_contains(&small_again, "CAPTURED CONTROLLER"));
        assert!(buffer_contains(&small_again, "LUA FIELD REFERENCE"));
        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::LuaFieldReference
        );
    }

    #[test]
    fn typed_input_only_reaches_the_pane_the_focus_marker_identifies() {
        use super::super::event;
        use super::super::state::Msg;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let key = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE);

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::FocusNextPane); // move focus to the Lua reference pane
        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::LuaFieldReference
        );

        // Both panes render, but typing must not reach the source while the
        // reference pane carries the focus marker, matching `event::map`'s
        // pane-local routing.
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "> LUA FIELD REFERENCE"));

        let msg = event::map(
            key,
            state.current_view(),
            state.reset_confirmation_pending(),
            state.quit_confirmation_pending(),
            state.redeploy_confirmation_pending(),
            state.focused_pane(View::Controller),
            state.focus_movement_available(state.current_view()),
        );
        assert_eq!(msg, None);

        // Once focus (and so the marker) moves back to the source pane, the
        // same key is routed there.
        state.apply(Msg::FocusNextPane);
        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::ControllerSource
        );
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "> CAPTURED CONTROLLER"));

        let msg = event::map(
            key,
            state.current_view(),
            state.reset_confirmation_pending(),
            state.quit_confirmation_pending(),
            state.redeploy_confirmation_pending(),
            state.focused_pane(View::Controller),
            state.focus_movement_available(state.current_view()),
        );
        assert_eq!(
            msg,
            Some(Msg::EditController(super::super::editor::EditOp::Insert(
                'z'
            )))
        );
        state.apply(msg.unwrap());
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "z"));
    }

    #[test]
    fn a_long_source_line_keeps_the_cursor_visible_by_scrolling_horizontally() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        let long_marker = "Z".repeat(100);
        for c in long_marker.chars() {
            state.apply(Msg::EditController(super::super::editor::EditOp::Insert(c)));
        }
        let terminal = render(120, 40, &state);

        assert!(
            buffer_contains(&terminal, "ZZZZZZZZZZ"),
            "the tail of a long line (where the cursor now is) must still be on screen"
        );
    }

    #[test]
    fn growing_the_pane_after_a_scroll_reveals_the_top_of_the_document_again() {
        // Scrolled to the bottom of a document taller than a short pane,
        // then resized much taller without the cursor moving: the widget's
        // own `focus()` only ever nudges an offset far enough to keep the
        // cursor visible and never shrinks it back just because the
        // viewport grew, so the offset must be recomputed from scratch each
        // render (not left over from the smaller pane) or the top of the
        // document stays hidden behind a blank margin even though the
        // grown viewport can now show the whole thing.
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        for _ in 0..60 {
            state.apply(Msg::EditController(super::super::editor::EditOp::Newline));
        }

        let short = render(120, MIN_ROWS, &state);
        assert!(
            !buffer_contains(&short, "function on_tick(observation)"),
            "the short pane must have scrolled the starter's first lines off screen"
        );

        let tall = render(120, 90, &state);
        assert!(
            buffer_contains(&tall, "function on_tick(observation)"),
            "growing the pane must reveal the top of the document again, not \
             keep the short pane's scroll offset"
        );
    }

    // A hand-authored, realistically sized Lua controller for issue #96's
    // "large representative Lua programs remain responsive" and viewport
    // coverage: several helper functions, a persistent state table, loops,
    // and comments — well past `STARTER_CONTROLLER`'s ~13 lines, and taller
    // than any supported pane can show at once. Decision recorded on issue
    // #96: this lives as a test-only constant rather than a new
    // `tests/fixtures/*.lua` file, matching how `STARTER_CONTROLLER` itself
    // is just a Rust string constant.
    const LARGE_REPRESENTATIVE_CONTROLLER: &str = r#"-- A more complete resistance controller than the starter: it remembers
-- which tiles it has already visited, prefers unexplored ground, and only
-- falls back to scanning once every reachable neighbor has been seen.
--
-- observation.drone     -- { x = <int>, y = <int> }
-- observation.discovered -- array of { x, y, traversable, uplink }

local visited = {}

local function tile_key(x, y)
  return x .. "," .. y
end

local function mark_visited(x, y)
  visited[tile_key(x, y)] = true
end

local function was_visited(x, y)
  return visited[tile_key(x, y)] == true
end

local function find_tile(observation, x, y)
  for _, tile in ipairs(observation.discovered) do
    if tile.x == x and tile.y == y then
      return tile
    end
  end
  return nil
end

local function is_open(observation, x, y)
  local tile = find_tile(observation, x, y)
  return tile ~= nil and tile.traversable
end

local function find_uplink(observation)
  for _, tile in ipairs(observation.discovered) do
    if tile.uplink then
      return tile
    end
  end
  return nil
end

-- Walks the four cardinal neighbors of (x, y) in a fixed, predictable
-- order and returns the first one that is open and not yet visited, or
-- nil if every open neighbor has already been visited.
local function unvisited_open_neighbor(observation, x, y)
  local candidates = {
    { dx = 0, dy = -1, action = "north" },
    { dx = 0, dy = 1, action = "south" },
    { dx = 1, dy = 0, action = "east" },
    { dx = -1, dy = 0, action = "west" },
  }

  for _, candidate in ipairs(candidates) do
    local nx, ny = x + candidate.dx, y + candidate.dy
    if is_open(observation, nx, ny) and not was_visited(nx, ny) then
      return candidate.action
    end
  end

  return nil
end

-- Same walk, but accepts any open neighbor regardless of whether it has
-- already been visited — used once every unvisited option is exhausted so
-- the drone keeps moving rather than idling next to the uplink forever.
local function any_open_neighbor(observation, x, y)
  local candidates = {
    { dx = 0, dy = -1, action = "north" },
    { dx = 0, dy = 1, action = "south" },
    { dx = 1, dy = 0, action = "east" },
    { dx = -1, dy = 0, action = "west" },
  }

  for _, candidate in ipairs(candidates) do
    local nx, ny = x + candidate.dx, y + candidate.dy
    if is_open(observation, nx, ny) then
      return candidate.action
    end
  end

  return nil
end

local function direction_towards_uplink(observation, uplink, x, y)
  if y > uplink.y and is_open(observation, x, y - 1) then
    return "north"
  end
  if y < uplink.y and is_open(observation, x, y + 1) then
    return "south"
  end
  if x > uplink.x and is_open(observation, x - 1, y) then
    return "west"
  end
  if x < uplink.x and is_open(observation, x + 1, y) then
    return "east"
  end
  return nil
end

function on_tick(observation)
  local x, y = observation.drone.x, observation.drone.y
  mark_visited(x, y)

  local uplink = find_uplink(observation)
  if uplink ~= nil then
    if x == uplink.x and y == uplink.y then
      return "wait"
    end

    local towards = direction_towards_uplink(observation, uplink, x, y)
    if towards ~= nil then
      return towards
    end
  end

  local unvisited = unvisited_open_neighbor(observation, x, y)
  if unvisited ~= nil then
    return unvisited
  end

  local anywhere = any_open_neighbor(observation, x, y)
  if anywhere ~= nil then
    return anywhere
  end

  -- Nowhere new or already-open to go: scan to reveal more of the map.
  return "scan"
end
"#;

    #[test]
    fn a_large_representative_controller_renders_correctly_at_every_supported_size() {
        use super::super::state::Msg;

        for (width, height) in [(120, 40), (150, 50)] {
            let mut state = AppState::new();
            state.apply(Msg::Activate);
            state.apply(Msg::Activate);
            state.apply(Msg::EditController(super::super::editor::EditOp::SelectAll));
            state.apply(Msg::PasteController(
                LARGE_REPRESENTATIVE_CONTROLLER.to_string(),
            ));
            state.apply(Msg::ValidateController);
            assert_eq!(
                state.validation(),
                &Validation::Valid,
                "the large representative fixture itself must be valid Lua"
            );

            let terminal = render(width, height, &state);
            assert!(
                buffer_contains(&terminal, "CAPTURED CONTROLLER"),
                "failed at {width}x{height}"
            );
            // Paste leaves the cursor at the end of the document, so the
            // viewport follows it there — assert on content near the tail,
            // not the (now likely scrolled-off) top of the program.
            assert!(
                buffer_contains(&terminal, "return \"scan\""),
                "failed at {width}x{height}"
            );
        }
    }

    #[test]
    fn vertical_scrolling_follows_the_cursor_through_a_tall_document() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::SelectAll));
        state.apply(Msg::PasteController(
            LARGE_REPRESENTATIVE_CONTROLLER.to_string(),
        ));
        // Paste leaves the cursor at the end of the pasted text, i.e. the
        // last line of the program.

        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            !buffer_contains(&terminal, "A more complete resistance controller"),
            "the top of a document this much taller than the pane must have \
             scrolled off screen once the cursor is at the end"
        );
        assert!(
            buffer_contains(&terminal, "return \"scan\""),
            "the cursor's line, near the end of the document, must be visible"
        );

        // Move back to the very top and confirm the viewport follows the
        // cursor back up again.
        for _ in 0..LARGE_REPRESENTATIVE_CONTROLLER.lines().count() {
            state.apply(Msg::EditController(super::super::editor::EditOp::MoveUp(
                false,
            )));
        }
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            buffer_contains(&terminal, "A more complete resistance controller"),
            "moving the cursor back to the top must scroll the viewport back up"
        );
    }

    #[test]
    fn a_banner_shrinking_the_pane_does_not_strand_the_cursor() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::SelectAll));
        state.apply(Msg::PasteController(
            LARGE_REPRESENTATIVE_CONTROLLER.to_string(),
        ));
        // Corrupt the tail with an unbalanced syntax error, appended at the
        // cursor (already at the end of the pasted program from the paste
        // above), so the document stays exactly as tall as the large
        // fixture and the cursor stays on its very last, deeply scrolled
        // line.
        state.apply(Msg::PasteController("\nfunction broken(\n".to_string()));

        // Before any banner exists, the cursor is already at the bottom of
        // a document far taller than the pane, so the viewport has already
        // had to scroll all the way down to keep it visible.
        let before_banner = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            before_banner.backend().cursor_visible(),
            "the cursor must be visible before any banner exists"
        );
        let cursor_before = before_banner.backend().cursor_position();

        state.apply(Msg::ValidateController);
        assert!(matches!(state.validation(), Validation::Invalid(_)));

        // At the supported minimum, a wrapped invalid-syntax banner can
        // claim several rows out of the source pane's height — rows taken
        // directly from where this already-scrolled-to-the-bottom cursor
        // was sitting.
        let after_banner = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            buffer_contains(&after_banner, "INVALID"),
            "the validation banner must be shown"
        );
        assert!(
            after_banner.backend().cursor_visible(),
            "the cursor must remain reachable even though the banner has \
             shrunk the editor's available height"
        );
        let cursor_after = after_banner.backend().cursor_position();
        assert!(
            cursor_after.y < MIN_ROWS,
            "the reported cursor position must fall inside the frame, not \
             behind the banner: cursor.y = {}",
            cursor_after.y
        );
        assert!(
            cursor_after.y < cursor_before.y,
            "the banner claiming rows must have actually shrunk the \
             content area the cursor was refocused against — otherwise the \
             cursor's row would be unchanged, not moved up: before = {}, \
             after = {}",
            cursor_before.y,
            cursor_after.y
        );
    }

    #[test]
    fn a_top_level_error_with_an_embedded_newline_does_not_crash_the_banner() {
        // Regression test for issue #122: `load_controller` executes the
        // player's top-level source, so a bare top-level `error(...)` call
        // produces an `mlua::Error` whose message embeds a literal newline
        // (and, via its stack traceback, tabs) — text `controller_banner`
        // used to embed verbatim into a single `Line`, which crashed
        // `word_wrapped_row_count`'s `cell_width()` call the next time the
        // Controller view rendered. Not panicking here is the point of
        // this test; the buffer assertions confirm the banner still shows
        // something useful.
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::SelectAll));
        state.apply(Msg::PasteController(
            "error(\"line one\\nline two\")\nfunction on_tick(observation) return \"wait\" end"
                .to_string(),
        ));
        state.apply(Msg::ValidateController);
        assert!(matches!(state.validation(), Validation::Invalid(_)));

        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(
            buffer_contains(&terminal, "INVALID"),
            "the validation banner must still be shown"
        );
        assert!(
            buffer_contains(&terminal, "controller.lua"),
            "the :line: location on the message's first line must remain visible"
        );
    }

    #[test]
    fn a_runtime_error_containing_a_tab_does_not_crash_the_report_pane() {
        // Regression test for issue #122's second reproduction: the
        // AfterAction/report pane's `bounded_detail_lines` shares the same
        // `error.to_string()` text (via `controller_error_detail`) and the
        // same `wrapped_row_count` measurement, but only split on `\n` —
        // a literal tab (as a real Lua stack traceback would contain) hit
        // the identical `cell_width()` panic there. Not panicking is the
        // point of this test.
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::PasteController(
            "function on_tick(observation)\n  error(\"bad state:\\tmore info\", 0)\nend\n"
                .to_string(),
        ));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();

        assert_eq!(state.current_view(), View::AfterAction);
        assert!(state.operation().unwrap().finished);

        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(buffer_contains(&terminal, "OPERATION FAILED"));
        assert!(buffer_contains(&terminal, "bad state:"));
    }

    #[test]
    fn the_cursor_stays_visible_while_editing_at_the_minimum_geometry() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        // At the console's minimum width the source pane's bordered editor
        // content area is well under 120 columns wide (it shares the frame
        // with the Lua reference pane and loses 2 more to its own border) —
        // 120 `Z`s is comfortably longer than that, so the line can only
        // fit by actually scrolling horizontally, not merely by shrinking
        // to fit.
        let long_marker = "Z".repeat(120);
        for c in long_marker.chars() {
            state.apply(Msg::EditController(EditOp::Insert(c)));
        }

        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(terminal.backend().cursor_visible());
        assert!(
            !buffer_contains(&terminal, &long_marker),
            "the full 120-character line must not fit unscrolled in the \
             minimum-geometry source pane"
        );
        assert!(
            buffer_contains(&terminal, "ZZZZZZZZZZ"),
            "the tail of the long line, where the cursor now is, must still \
             be on screen at the minimum geometry"
        );
    }

    #[test]
    fn unicode_wide_glyphs_and_combining_marks_render_with_a_visible_cursor() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(EditOp::SelectAll));
        // "-- 你好 café" — a double-width CJK run and an "é" built from a
        // combining acute accent, kept short enough to stay within one
        // viewport width per docs/TUI_DESIGN.md and the accepted upstream
        // limitation pinned by
        // `known_limitation_wide_glyph_line_can_leave_cursor_offscreen_after_focus`
        // in tests/editor_foundation_contract.rs.
        state.apply(Msg::PasteController(
            "-- \u{4f60}\u{597d} cafe\u{0301}\nfunction on_tick(observation)\n  return \"wait\"\nend\n".to_string(),
        ));
        // The trailing newline leaves the cursor on the blank line after
        // "end", nowhere near the Unicode content. Walk it back up onto
        // the Unicode line and rightward to land it between the two wide
        // CJK glyphs — precisely where a display-width-to-column
        // miscalculation would misplace it.
        for _ in 0..4 {
            state.apply(Msg::EditController(EditOp::MoveUp(false)));
        }
        for _ in 0..4 {
            state.apply(Msg::EditController(EditOp::MoveRight(false)));
        }
        assert_eq!(
            state.controller_source().unwrap().lines().next(),
            Some("-- \u{4f60}\u{597d} cafe\u{0301}"),
            "the cursor walk above must land back on the Unicode line \
             without having altered it"
        );

        for (width, height) in [(120, 40), (MIN_COLUMNS, MIN_ROWS)] {
            let terminal = render(width, height, &state);
            // Checked individually, not as one contiguous substring: a
            // double-width glyph's trailing cell renders as a padding
            // placeholder, not an empty string, so two adjacent wide
            // glyphs are not adjacent in the flattened buffer text.
            assert!(
                buffer_contains(&terminal, "\u{4f60}"),
                "the first wide CJK glyph must render at {width}x{height}"
            );
            assert!(
                buffer_contains(&terminal, "\u{597d}"),
                "the second wide CJK glyph must render at {width}x{height}"
            );
            assert!(
                buffer_contains(&terminal, "cafe\u{0301}"),
                "the combining-mark grapheme must render at {width}x{height}"
            );
            assert!(
                terminal.backend().cursor_visible(),
                "the cursor, positioned between the two wide CJK glyphs, \
                 must be visible at {width}x{height}"
            );
        }

        // Move onto the combining-mark grapheme itself (the end of the
        // line, right after "café") and confirm the cursor is still
        // correctly placed and visible there too.
        state.apply(Msg::EditController(EditOp::MoveLineEnd(false)));
        for (width, height) in [(120, 40), (MIN_COLUMNS, MIN_ROWS)] {
            let terminal = render(width, height, &state);
            assert!(
                terminal.backend().cursor_visible(),
                "the cursor, positioned after the combining-mark grapheme, \
                 must be visible at {width}x{height}"
            );
        }
    }

    #[test]
    fn word_wrapped_row_count_matches_greedy_word_wrapping_not_total_width_division() {
        // Three 40-column words in a 78-column line: a naive
        // total-width/pane-width division sees 120/78 = 2 rows, but real
        // word wrapping can't fit a second 40-column word on the first
        // word's row (40 + 1 + 40 = 81 > 78), so it actually takes 3.
        let word = "x".repeat(40);
        let text = format!("{word} {word} {word}");
        assert_eq!(word_wrapped_row_count(&text, 78), 3);
    }

    #[test]
    fn word_wrapped_row_count_preserves_whitespace_runs_instead_of_collapsing_them() {
        // A 70-column word, ten literal spaces, and a short recovery word
        // in a 78-column pane: `Wrap { trim: false }` preserves the space
        // run, so the trailing "recover" (10 + 7 = 17 columns) can't share
        // a row with the 70-column word (70 + 17 = 87 > 78) and wraps to a
        // second row — collapsing the run to a single space would instead
        // predict everything fits on one 78-column row (70 + 1 + 7 = 78).
        let text = format!("{}{}recover", "x".repeat(70), " ".repeat(10));
        assert_eq!(word_wrapped_row_count(&text, 78), 2);
    }

    #[test]
    fn word_wrapped_row_count_measures_a_halfwidth_katakana_dakuten_like_ratatui() {
        // Same underlying divergence as `display_width_matches_ratatui_
        // for_a_halfwidth_katakana_dakuten`, but exercised through the
        // word-wrap row counter a banner/Help actually uses: a per-
        // character `unicode-width` sum treats U+FF9E as zero-width, while
        // ratatui's own `CellWidth` counts it as an extra occupied cell.
        let word = "\u{FF76}\u{FF9E}".repeat(40); // 40x (halfwidth カ + dakuten) = 80 cells
        assert_eq!(word_wrapped_row_count(&word, 78), 80_usize.div_ceil(78));
    }

    #[test]
    fn word_wrapped_row_count_hard_wraps_a_single_word_wider_than_the_line() {
        let word = "x".repeat(200);
        assert_eq!(word_wrapped_row_count(&word, 78), 200_usize.div_ceil(78));
    }

    #[test]
    fn word_wrapped_row_count_treats_an_empty_line_as_one_row() {
        assert_eq!(word_wrapped_row_count("", 78), 1);
    }

    #[test]
    fn help_marks_gated_navigation_keys_as_unavailable() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 60, &state);

        assert!(buffer_contains(
            &terminal,
            "F3 Target        (unavailable: inspect the [OPEN] signal first)"
        ));
        assert!(buffer_contains(
            &terminal,
            "F4 Controller    (unavailable: work an opportunity from Target first)"
        ));
        assert!(buffer_contains(
            &terminal,
            "F5 Operation     (unavailable: work an opportunity from Target first)"
        ));
    }

    #[test]
    fn help_marks_target_available_once_inspected() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 60, &state);

        assert!(buffer_contains(
            &terminal,
            "F3 Target        dossier for the current opportunity"
        ));
    }

    fn working_state() -> AppState {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::Navigate(View::Operation));
        state
    }

    #[test]
    fn operation_view_before_any_deploy_shows_the_placeholder() {
        let state = working_state();
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "No operation is deployed yet."));
    }

    #[test]
    fn operation_placeholder_shows_no_marker_and_no_f8_hint() {
        let state = working_state();

        for (width, height) in [(MIN_COLUMNS, MIN_ROWS), (120, 40)] {
            let terminal = render(width, height, &state);
            assert!(!buffer_contains(&terminal, ">"));
            assert!(!buffer_contains(&terminal, "F8 Next Pane"));
            assert!(!buffer_contains(&terminal, "F8 Pane"));
        }
    }

    #[test]
    fn f8_does_not_move_remembered_operation_focus_from_the_placeholder() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::FocusNextPane);
        state.apply(Msg::RequestDeploy);

        // A stray F8 press on the undeployed placeholder must not have
        // corrupted the remembered focus: the first real deployment still
        // opens on the documented default (the satellite feed).
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "> COMPROMISED SATELLITE FEED"));
    }

    #[test]
    fn after_action_placeholder_shows_no_marker_and_no_f8_hint() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::Navigate(View::AfterAction));

        for (width, height) in [(MIN_COLUMNS, MIN_ROWS), (120, 40)] {
            let terminal = render(width, height, &state);
            assert!(buffer_contains(
                &terminal,
                "No operation has concluded yet."
            ));
            assert!(!buffer_contains(&terminal, ">"));
            assert!(!buffer_contains(&terminal, "F8 Next Pane"));
            assert!(!buffer_contains(&terminal, "F8 Pane"));
        }
    }

    #[test]
    fn f8_does_not_move_hidden_after_action_focus_from_the_placeholder() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::Navigate(View::AfterAction));
        state.apply(Msg::FocusNextPane);

        assert_eq!(
            state.focused_pane(View::AfterAction),
            PaneId::Report,
            "F8 must stay inert on the no-conclusion placeholder"
        );
    }

    #[test]
    fn deploying_shows_the_satellite_feed_and_running_status() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "COMPROMISED SATELLITE FEED"));
        assert!(buffer_contains(&terminal, "OPERATION TELEMETRY"));
        assert!(buffer_contains(&terminal, "controller    running"));
        assert!(buffer_contains(&terminal, "STATUS: RUNNING"));
        // The starter controller's drone hasn't moved yet at tick 0.
        assert!(buffer_contains(&terminal, "tick          00"));
    }

    #[test]
    fn running_the_starter_controller_to_completion_reaches_an_unambiguous_result() {
        let mut state = working_state();
        state.apply(super::super::state::Msg::RequestDeploy);

        // The starter controller scans once, then waits forever, so it
        // always ends in budget exhaustion on the fixed 15-budget scenario.
        for _ in 0..20 {
            if state.operation().is_some_and(|op| op.finished) {
                break;
            }
            state.advance_running_operation();
        }
        assert!(
            state.operation().unwrap().finished,
            "should have finished well within 20 ticks"
        );

        // Finishing hands the view off from Operation to After Action
        // (`docs/TUI_DESIGN.md` §5) — the failure headline, status, and a
        // redeploy hint all now live there.
        assert_eq!(state.current_view(), View::AfterAction);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: budget exhausted"
        ));
        // The outcome hierarchy's trigger/meaning and completion lines
        // (`docs/TUI_DESIGN.md` §5 "Failure and controller error") — the
        // failure counterpart to the success closure in issue #68. The full
        // sentence wraps across several rows at this pane width, so — like
        // the success test above — assert on short substrings known to
        // land intact on a single row.
        assert!(buffer_contains(&terminal, "reached the uplink"));
        assert!(buffer_contains(&terminal, "foothold"));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
        assert!(buffer_contains(&terminal, "ticks executed"));
        assert!(buffer_contains(&terminal, "tiles discovered"));
        assert!(buffer_contains(&terminal, "hazards entered"));
        assert!(buffer_contains(&terminal, "remaining budget"));
        assert!(buffer_contains(&terminal, "deployed rev"));
        assert!(buffer_contains(&terminal, "STATUS: FAILED"));
        assert!(buffer_contains(&terminal, "F6 Redeploy"));
        assert!(buffer_contains(&terminal, "F4  revise the controller"));

        // Review Run (`F5`/`Navigate(Operation)`) still shows the finished
        // run's own telemetry pane, unchanged from before this view split —
        // plus the frozen source and revision that produced this result, so
        // the player can tell which code ran even after editing Controller
        // (`docs/TUI_DESIGN.md` §5, "Review Run").
        state.apply(super::super::state::Msg::Navigate(View::Operation));
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "F6  redeploy"));
        assert!(buffer_contains(&terminal, "DEPLOYED SOURCE"));
        assert!(buffer_contains(&terminal, "deployed rev  run-01"));
    }

    #[test]
    fn a_deploy_time_script_error_is_shown_in_operation_without_a_live_run() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        // Clears the seeded starter controller down to an empty document,
        // then types a single unbalanced paren — invalid Lua — the same
        // way `console::state`'s own `validating_an_invalid_controller_...`
        // test builds a broken source without needing direct field access.
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));
        state.apply(Msg::RequestDeploy);

        // A synchronous load failure has no live run to show, so it lands
        // directly on After Action rather than an empty Operation view.
        assert_eq!(state.current_view(), View::AfterAction);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: controller script error"
        ));
        // Controller/script failures state that execution stopped and First
        // Contact remains incomplete, while preserving the existing
        // diagnostic detail (`docs/TUI_DESIGN.md` §5).
        assert!(buffer_contains(&terminal, "execution stopped"));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
        assert!(state.operation().unwrap().finished);
    }

    #[test]
    fn a_zero_tick_deploy_failure_shows_no_recorded_satellite_state_in_review_run() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));
        state.apply(Msg::RequestDeploy);
        assert_eq!(state.current_view(), View::AfterAction);
        assert!(state.operation().unwrap().review_points.is_empty());

        // Review Run (`F5`/`Navigate(Operation)`) must never manufacture a
        // satellite frame from authoritative scenario state just because
        // deploy itself failed before any tick ever executed
        // (`docs/TUI_DESIGN.md` §5, "Review Run").
        state.apply(Msg::Navigate(View::Operation));
        for (width, height) in [(120, 40), (150, 50)] {
            let terminal = render(width, height, &state);
            assert!(buffer_contains(
                &terminal,
                "NO RECORDED SATELLITE EXECUTION STATE"
            ));
            assert!(!buffer_contains(&terminal, "legend: D drone"));
            assert!(buffer_contains(
                &terminal,
                "OPERATION FAILED: controller script error"
            ));
            assert!(buffer_contains(&terminal, "F4  revise the controller"));
            assert!(buffer_contains(&terminal, "F6  redeploy"));
        }
    }

    /// Types `source` into a working, undeployed state, deploys it, and
    /// steps every tick to whatever conclusion it deterministically reaches —
    /// the shared plumbing behind [`succeeded_state`]/[`budget_exhausted_state`]
    /// and this module's mid-run-controller-failure fixture.
    fn deployed_and_run_to_completion(source: &str) -> AppState {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::PasteController(source.to_string()));
        state.apply(Msg::RequestDeploy);

        for _ in 0..20 {
            if state.operation().is_some_and(|op| op.finished) {
                break;
            }
            state.advance_running_operation();
        }
        assert!(
            state.operation().unwrap().finished,
            "should have finished well within 20 ticks"
        );
        state
    }

    /// Two valid moves onto floor tiles adjacent to the fixed First Contact
    /// drone start, then an unconditional error — mirrors `console::state`'s
    /// own `FAILS_AFTER_TWO_TICKS` fixture, exercising a controller failure
    /// that happens after some ticks have already completed.
    const FAILS_AFTER_TWO_TICKS: &str = r#"
        local step = 0
        function on_tick(observation)
            step = step + 1
            if step > 2 then error('boom') end
            return "north"
        end
    "#;

    #[test]
    fn a_mid_run_controller_failure_shows_a_distinct_failure_boundary_in_review_run() {
        let mut state = deployed_and_run_to_completion(FAILS_AFTER_TWO_TICKS);
        assert_eq!(
            state.operation().unwrap().records.len(),
            2,
            "exactly two ticks should have completed before the failure"
        );

        state.apply(super::super::state::Msg::Navigate(View::Operation));
        for (width, height) in [(120, 40), (150, 50)] {
            let terminal = render(width, height, &state);
            // The failure boundary is clearly distinguished from the last
            // completed tick, names which tick preceded it, and carries no
            // fabricated action of its own (`docs/TUI_DESIGN.md` §5, "A
            // runtime/controller failure boundary must clearly distinguish
            // the last completed tick from the failure that stopped
            // execution").
            assert!(buffer_contains(&terminal, "FAILURE (after tick 02)"));
            assert!(buffer_contains(
                &terminal,
                "action        (none — execution stopped)"
            ));
            assert!(buffer_contains(
                &terminal,
                "OPERATION FAILED: controller runtime error"
            ));
            assert!(buffer_contains(&terminal, "boom"));
        }
    }

    #[test]
    fn a_synchronous_deploy_failure_defaults_to_the_report_pane() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));
        state.apply(Msg::RequestDeploy);
        assert_eq!(state.current_view(), View::AfterAction);

        // A deploy that never started a live run has nothing to show in the
        // satellite pane, so the Report pane must carry the focus marker by
        // default — not the final satellite frame — even though both panes
        // render.
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(buffer_contains(&terminal, "> AFTER-ACTION REPORT"));
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: controller script error"
        ));
    }

    #[test]
    fn after_action_focus_survives_a_resize() {
        use super::super::state::Msg;

        // `budget_exhausted_state` is defined below, alongside the rest of
        // the After Action test helpers; this test only needs a finished
        // operation, not a specific outcome.
        let mut state = budget_exhausted_state();
        assert_eq!(state.focused_pane(View::AfterAction), PaneId::Report);

        state.apply(Msg::FocusNextPane); // move focus to the final satellite frame

        let small = render(120, 40, &state);
        assert!(buffer_contains(&small, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(&small, "FINAL SATELLITE FRAME"));
        assert!(buffer_contains(&small, "> FINAL SATELLITE FRAME"));
        assert!(!buffer_contains(&small, "> AFTER-ACTION REPORT"));

        let large = render(150, 50, &state);
        assert!(buffer_contains(&large, "FINAL SATELLITE FRAME"));
        assert!(buffer_contains(&large, "> FINAL SATELLITE FRAME"));

        let small_again = render(120, 40, &state);
        assert!(buffer_contains(&small_again, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(&small_again, "FINAL SATELLITE FRAME"));
        assert_eq!(state.focused_pane(View::AfterAction), PaneId::FinalFrame);
    }

    #[test]
    fn the_recovery_hint_stays_visible_at_the_minimum_geometry() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));
        state.apply(Msg::RequestDeploy);
        assert_eq!(state.current_view(), View::AfterAction);

        // The two-pane report pane is only 40% wide at the console's
        // supported minimum geometry — narrow enough that both the headline
        // and the diagnostic detail wrap across several lines. Before the
        // recovery hint was pinned to a fixed last row
        // (`draw_pane_with_pinned_action` via `draw_after_action_report_pane`),
        // that wrapped content could push it below the pane's visible rows.
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        // The full headline text wraps across rows at this width, and a
        // wrapped line is no longer contiguous once the buffer is flattened
        // row-major across both panes — assert on a short substring known
        // to land intact on a single row instead.
        assert!(buffer_contains(&terminal, "OPERATION FAILED"));
        assert!(buffer_contains(&terminal, "F4  revise the controller"));
    }

    /// Deploys the seeded starter controller and steps it to its
    /// deterministic budget-exhaustion conclusion (it scans once, then
    /// waits forever, on the fixed 15-budget scenario) — the failure
    /// counterpart to [`succeeded_state`].
    fn budget_exhausted_state() -> AppState {
        let mut state = working_state();
        state.apply(super::super::state::Msg::RequestDeploy);

        for _ in 0..20 {
            if state.operation().is_some_and(|op| op.finished) {
                break;
            }
            state.advance_running_operation();
        }
        assert!(
            state.operation().unwrap().finished,
            "should have exhausted its budget well within 20 ticks"
        );
        state
    }

    #[test]
    fn a_budget_exhaustion_failure_defaults_to_the_report_pane_at_the_minimum_geometry() {
        let state = budget_exhausted_state();
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        // At the console's supported minimum geometry, the outcome and
        // incompletion must be visible in the Report pane by default,
        // without pressing `F8` (`docs/TUI_DESIGN.md` §5).
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: budget exhausted"
        ));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
    }

    #[test]
    fn the_full_failure_closure_fits_at_the_minimum_geometry() {
        let state = budget_exhausted_state();

        // At the console's supported minimum geometry, the 40%-wide report
        // pane has room for the full closure — outcome, completion,
        // evidence, the closing paragraph, and the separately-pinned `F4`
        // recovery hint — same as success.
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: budget exhausted"
        ));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
        assert!(buffer_contains(&terminal, "deployed rev"));
        // The full closing paragraph, including its last word, must
        // survive — `Paragraph` doesn't scroll, so content taller than the
        // pane's inner height is silently clipped from the bottom.
        assert!(buffer_contains(&terminal, "Signals."));
        assert!(buffer_contains(&terminal, "F4  revise the controller"));
    }

    const ROUTE_TO_UPLINK: &str = r#"
        local route = { "north", "east", "east", "east", "east", "north", "north", "north" }
        local step = 0
        function on_tick(observation)
            step = step + 1
            return route[step]
        end
    "#;

    /// Types [`ROUTE_TO_UPLINK`] into a working, undeployed state, deploys
    /// it, and steps every tick to a deterministic successful conclusion —
    /// the UI-level analog of `console::state`'s own success fixture, built
    /// through the same `EditController`/`RequestDeploy` events the other
    /// `ui.rs` tests already use rather than reaching into private fields.
    fn succeeded_state() -> AppState {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::PasteController(ROUTE_TO_UPLINK.to_string()));
        state.apply(Msg::RequestDeploy);

        for _ in 0..20 {
            if state.operation().is_some_and(|op| op.finished) {
                break;
            }
            state.advance_running_operation();
        }
        assert!(
            state.operation().unwrap().finished,
            "should have reached the uplink well within 20 ticks"
        );
        state
    }

    #[test]
    fn a_successful_operation_shows_the_foothold_report() {
        let state = succeeded_state();
        assert_eq!(state.current_view(), View::AfterAction);
        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "FOOTHOLD ESTABLISHED"));
        assert!(buffer_contains(&terminal, "reached the facility uplink"));
        // The meaning sentence must reach its actual end ("closed.") rather
        // than being hard-truncated with an ellipsis by
        // `bounded_detail_lines`'s `MAX_DETAIL_LINE_CHARS` cap, which this
        // fixed, trusted copy must not be routed through.
        assert!(buffer_contains(&terminal, "closed."));
        assert!(!buffer_contains(&terminal, "…"));
        assert!(buffer_contains(&terminal, "FIRST CONTACT COMPLETE"));
        assert!(buffer_contains(&terminal, "ticks executed"));
        assert!(buffer_contains(&terminal, "tiles discovered"));
        assert!(buffer_contains(&terminal, "hazards entered"));
        assert!(buffer_contains(&terminal, "remaining budget"));
        assert!(buffer_contains(&terminal, "deployed rev"));
        assert!(buffer_contains(
            &terminal,
            "No further operation is available"
        ));
        assert!(buffer_contains(&terminal, "FINAL SATELLITE FRAME"));
    }

    #[test]
    fn the_success_report_blank_line_spacing_matches_the_failure_report() {
        let state = succeeded_state();
        let op = state.operation().unwrap();
        let lines: Vec<String> = after_action_report_lines(&op)
            .into_iter()
            .map(String::from)
            .collect();

        // Same two separator positions `after_action_failure_lines` uses: a
        // blank line before the completion line, and another before the
        // availability line — without these the success report renders as
        // one dense block instead of matching the failure report's spacing.
        let completion_index = lines
            .iter()
            .position(|line| line == FIRST_CONTACT_COMPLETE)
            .expect("completion line should be present");
        assert_eq!(
            lines[completion_index - 1],
            "",
            "expected a blank line before {FIRST_CONTACT_COMPLETE:?}, got {:?}",
            lines
        );

        let availability_index = lines
            .iter()
            .position(|line| line == NO_FURTHER_OPERATION)
            .expect("availability line should be present");
        assert_eq!(
            lines[availability_index - 1],
            "",
            "expected a blank line before the availability line, got {:?}",
            lines
        );
    }

    #[test]
    fn a_successful_operation_defaults_to_the_report_pane_at_the_minimum_geometry() {
        let state = succeeded_state();
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        // At the console's supported minimum geometry, the outcome must be
        // visible without pressing `F8` to swap panes (`docs/TUI_DESIGN.md`
        // §5, "Responsive behavior").
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(&terminal, "FOOTHOLD ESTABLISHED"));
        assert!(buffer_contains(&terminal, "FIRST CONTACT COMPLETE"));
    }

    #[test]
    fn the_full_success_closure_reaches_the_bottom_at_the_minimum_geometry() {
        use super::super::state::Msg;

        let mut state = succeeded_state();

        // `MIN_COLUMNS` is the narrowest supported width, giving the report
        // pane's 40%-width inner area the tightest fit the full success
        // report has to survive. A wider width (150) leaves the same inner
        // *height* but a wider pane, so it's checked too.
        for width in [MIN_COLUMNS, 150] {
            let terminal = render(width, MIN_ROWS, &state);
            assert!(
                buffer_contains(&terminal, "FOOTHOLD ESTABLISHED"),
                "headline clipped at {width}x{MIN_ROWS}"
            );
            assert!(
                buffer_contains(&terminal, "FIRST CONTACT COMPLETE"),
                "completion line clipped at {width}x{MIN_ROWS}"
            );
            assert!(
                buffer_contains(&terminal, "deployed rev"),
                "evidence clipped at {width}x{MIN_ROWS}"
            );
            // The blank-line-separated report (#76) no longer has to fit
            // without scrolling at this geometry — but the closing
            // paragraph, including its last word, must still be reachable
            // by scrolling all the way down (#77's pane-scroll mechanism),
            // rather than being unrecoverably cut off.
            let max_scroll = after_action_max_scroll(&state, width, MIN_ROWS);
            for _ in 0..max_scroll {
                state.apply(Msg::ScrollDown);
            }
            let scrolled = render(width, MIN_ROWS, &state);
            assert!(
                buffer_contains(&scrolled, "network."),
                "closing paragraph unreachable even scrolled to the bottom at {width}x{MIN_ROWS}"
            );
            for _ in 0..max_scroll {
                state.apply(Msg::ScrollUp);
            }
        }
    }

    #[test]
    fn reviewing_a_successful_run_shows_the_same_foothold_headline() {
        let mut state = succeeded_state();
        state.apply(super::super::state::Msg::Navigate(View::Operation));
        let terminal = render(120, 40, &state);

        // Review Run must describe the same recorded run consistently with
        // After Action, while still carrying its own frozen provenance
        // (`docs/TUI_DESIGN.md` §5, "Review Run").
        assert!(buffer_contains(&terminal, "FOOTHOLD ESTABLISHED"));
        assert!(buffer_contains(&terminal, "DEPLOYED SOURCE"));
        assert!(buffer_contains(&terminal, "deployed rev  run-01"));
    }

    #[test]
    fn strip_control_characters_replaces_control_characters_with_spaces() {
        assert_eq!(
            strip_control_characters("bad state:\tmore info"),
            "bad state: more info"
        );
        assert_eq!(
            strip_control_characters("no control chars here"),
            "no control chars here"
        );
        // Ordinary text and Unicode (including a combining-mark grapheme,
        // which is not a control character) must pass through untouched.
        assert_eq!(
            strip_control_characters("caf\u{e9} \u{4f60}\u{597d}"),
            "caf\u{e9} \u{4f60}\u{597d}"
        );
    }

    #[test]
    fn bounded_detail_lines_strips_control_characters_from_each_line() {
        // Issue #122: a Lua stack traceback's tab-indented lines must not
        // reach `wrapped_row_count`'s `cell_width()` call unsanitized.
        let text = "first line\nsecond\tline\twith\ttabs";
        let lines = bounded_detail_lines(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].to_string(), "first line");
        assert_eq!(lines[1].to_string(), "second line with tabs");
    }

    #[test]
    fn bounded_detail_lines_truncates_long_text_with_a_marker() {
        let long_line = "x".repeat(MAX_DETAIL_LINE_CHARS + 50);
        let many_lines = std::iter::repeat_n("line", MAX_DETAIL_LINES + 5)
            .collect::<Vec<_>>()
            .join("\n");

        let truncated_line = bounded_detail_lines(&long_line);
        assert_eq!(truncated_line.len(), 1);
        let rendered = truncated_line[0].to_string();
        assert!(rendered.ends_with('…'));
        assert!(rendered.chars().count() <= MAX_DETAIL_LINE_CHARS + 1);

        let truncated_lines = bounded_detail_lines(&many_lines);
        assert_eq!(truncated_lines.len(), MAX_DETAIL_LINES + 1);
    }

    #[test]
    fn a_top_level_execution_limit_reads_as_execution_limit_not_a_script_error() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        for c in "while true do pcall(function() while true do end end) end\nfunction on_tick(observation) return \"wait\" end".chars() {
            let op = if c == '\n' {
                EditOp::Newline
            } else {
                EditOp::Insert(c)
            };
            state.apply(Msg::EditController(op));
        }
        state.apply(Msg::RequestDeploy);

        assert_eq!(state.current_view(), View::AfterAction);
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: controller execution limit"
        ));
    }

    #[test]
    fn a_long_multiline_controller_error_does_not_clip_completion_at_minimum_geometry() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        // A worst-case runtime error: several long lines, each well past
        // `MAX_DETAIL_LINE_CHARS`, that `bounded_detail_lines` caps by
        // logical line/character count but not by the rows `Wrap` needs to
        // reflow them at the report pane's narrow (~40%) width. Before
        // `FIRST CONTACT INCOMPLETE` was moved ahead of the diagnostic
        // detail, this could push it off the unscrollable pane entirely.
        state.apply(Msg::PasteController(
            r#"
                function on_tick(observation)
                    local segment = string.rep("x", 130)
                    local message = segment
                    for i = 1, 9 do
                        message = message .. "\n" .. segment
                    end
                    error(message, 0)
                end
            "#
            .to_string(),
        ));
        state.apply(Msg::RequestDeploy);
        state.advance_running_operation();

        assert_eq!(state.current_view(), View::AfterAction);
        assert!(state.operation().unwrap().finished);

        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);
        assert!(buffer_contains(&terminal, "OPERATION FAILED"));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
        assert!(buffer_contains(&terminal, "F4  revise the controller"));
    }

    #[test]
    fn redeploying_an_active_run_shows_a_confirmation_prompt() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        state.apply(Msg::RequestDeploy); // pending confirmation

        let terminal = render(120, 40, &state);

        assert!(buffer_contains(&terminal, "REDEPLOY?"));
        assert!(buffer_contains(
            &terminal,
            "Enter / y  confirm and redeploy"
        ));
        assert!(!buffer_contains(&terminal, "F8 Next Pane"));
        assert!(!buffer_contains(&terminal, "F8 Pane"));
    }

    #[test]
    fn operation_view_shows_both_panes_and_moves_the_focus_marker_with_f8() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "COMPROMISED SATELLITE FEED"));
        assert!(buffer_contains(&terminal, "OPERATION TELEMETRY"));
        assert!(buffer_contains(&terminal, "> COMPROMISED SATELLITE FEED"));
        assert!(!buffer_contains(&terminal, "> OPERATION TELEMETRY"));

        state.apply(Msg::FocusNextPane);
        let terminal = render(120, 40, &state);
        assert!(buffer_contains(&terminal, "OPERATION TELEMETRY"));
        assert!(buffer_contains(&terminal, "COMPROMISED SATELLITE FEED"));
        assert!(buffer_contains(&terminal, "> OPERATION TELEMETRY"));
    }

    #[test]
    fn operation_focus_survives_a_resize() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        state.apply(Msg::FocusNextPane); // move focus to telemetry

        let small = render(120, 40, &state);
        assert!(buffer_contains(&small, "COMPROMISED SATELLITE FEED"));
        assert!(buffer_contains(&small, "OPERATION TELEMETRY"));
        assert!(buffer_contains(&small, "> OPERATION TELEMETRY"));
        assert!(!buffer_contains(&small, "> COMPROMISED SATELLITE FEED"));

        let large = render(150, 50, &state);
        assert!(buffer_contains(&large, "OPERATION TELEMETRY"));
        assert!(buffer_contains(&large, "> OPERATION TELEMETRY"));

        let small_again = render(120, 40, &state);
        assert!(buffer_contains(&small_again, "COMPROMISED SATELLITE FEED"));
        assert!(buffer_contains(&small_again, "OPERATION TELEMETRY"));
        assert_eq!(
            state.focused_pane(View::Operation),
            PaneId::OperationTelemetry
        );
    }

    #[test]
    fn quitting_with_only_an_active_run_explains_the_run_will_be_abandoned() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        assert!(!state.controller_modified());
        state.apply(Msg::RequestQuit);

        let terminal = render(120, 40, &state);

        assert!(buffer_contains(
            &terminal,
            "The active run will be abandoned."
        ));
    }

    /// A minimal but representative [`OperationView`] for exercising
    /// [`review_point_evidence_lines`] directly against hand-built
    /// [`ReviewPoint`] values — including [`ReviewPointKind`]s the player
    /// cannot yet navigate to via any `Msg` (chronology navigation is out
    /// of this issue's scope, so `review_selected` is always the run's
    /// terminal point in practice).
    fn test_operation_view<'a>(records: &'a [TickRecord]) -> OperationView<'a> {
        OperationView {
            deployed_source: "function on_tick(observation) return \"wait\" end",
            run_id: 1,
            records,
            paused: false,
            finished: true,
            error: None,
            starting_budget: 15,
            current: OperationSnapshot {
                drone_position: crate::simulation::Position { x: 0, y: 0 },
                map_width: 5,
                map_height: 5,
                discovered: Vec::new(),
                tick: 0,
                budget_remaining: 15,
            },
            conclusion: None,
            review_points: Vec::new(),
            review_selected: None,
        }
    }

    fn test_snapshot(tick: u32, budget_remaining: u32) -> OperationSnapshot {
        OperationSnapshot {
            drone_position: crate::simulation::Position { x: 1, y: 2 },
            map_width: 5,
            map_height: 5,
            discovered: Vec::new(),
            tick,
            budget_remaining,
        }
    }

    fn lines_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn review_point_evidence_lines_for_initial_shows_no_invented_action() {
        let op = test_operation_view(&[]);
        let point = ReviewPoint {
            kind: ReviewPointKind::Initial,
            snapshot: test_snapshot(0, 15),
            newly_discovered: Vec::new(),
        };
        let text = lines_text(&review_point_evidence_lines(&op, &point));

        assert!(text.contains("INITIAL"));
        assert!(text.contains("action        (none — pre-tick observation)"));
        assert!(text.contains("position      (1, 2)"));
        assert!(text.contains("budget        15 / 15"));
        assert!(text.contains("discovered    0 new tile(s)"));
        // No terminal outcome/failure evidence belongs on a pre-tick point.
        assert!(!text.contains("FOOTHOLD ESTABLISHED"));
        assert!(!text.contains("OPERATION FAILED"));
    }

    #[test]
    fn review_point_evidence_lines_for_a_non_terminal_tick_shows_its_action_and_events_but_no_outcome()
     {
        let record = TickRecord {
            tick: 1,
            drone_position: crate::simulation::Position { x: 1, y: 2 },
            action: Action::Scan,
            budget_remaining: 14,
            outcome: TickOutcome::Running,
            events: vec![SimEvent::ActionCost {
                action: Action::Scan,
                amount: 1,
            }],
            map_width: 5,
            map_height: 5,
            discovered: Vec::new(),
        };
        let op = test_operation_view(&[]);
        let point = ReviewPoint {
            kind: ReviewPointKind::Tick(&record),
            snapshot: test_snapshot(1, 14),
            newly_discovered: Vec::new(),
        };
        let text = lines_text(&review_point_evidence_lines(&op, &point));

        assert!(text.contains("TICK 01"));
        assert!(text.contains("action        scanned"));
        assert!(text.contains("scanned — cost 1"));
        // A tick still in progress is not the run's terminal boundary, so
        // no outcome headline belongs here.
        assert!(!text.contains("FOOTHOLD ESTABLISHED"));
        assert!(!text.contains("OPERATION FAILED"));
    }

    #[test]
    fn review_point_evidence_lines_for_the_terminal_tick_shows_the_outcome_headline() {
        let record = TickRecord {
            tick: 3,
            drone_position: crate::simulation::Position { x: 4, y: 4 },
            action: Action::MoveNorth,
            budget_remaining: 10,
            outcome: TickOutcome::Succeeded,
            events: vec![SimEvent::OperationSucceeded],
            map_width: 5,
            map_height: 5,
            discovered: Vec::new(),
        };
        let op = test_operation_view(&[]);
        let point = ReviewPoint {
            kind: ReviewPointKind::Tick(&record),
            snapshot: test_snapshot(3, 10),
            newly_discovered: Vec::new(),
        };
        let text = lines_text(&review_point_evidence_lines(&op, &point));

        assert!(text.contains("TICK 03"));
        assert!(text.contains("action        moved north"));
        assert!(text.contains("FOOTHOLD ESTABLISHED"));
        // `OperationSucceeded` is represented by the headline, not
        // duplicated as its own structured event line.
        assert!(!text.contains("EVENTS"));
    }

    #[test]
    fn review_point_evidence_lines_for_a_terminal_failure_after_ticks_names_the_last_tick_without_a_fabricated_action()
     {
        let completed = [TickRecord {
            tick: 2,
            drone_position: crate::simulation::Position { x: 1, y: 1 },
            action: Action::MoveEast,
            budget_remaining: 13,
            outcome: TickOutcome::Running,
            events: Vec::new(),
            map_width: 5,
            map_height: 5,
            discovered: Vec::new(),
        }];
        let op = test_operation_view(&completed);
        let point = ReviewPoint {
            kind: ReviewPointKind::TerminalFailure(&ControllerError::MissingCallback),
            snapshot: test_snapshot(2, 13),
            newly_discovered: Vec::new(),
        };
        let text = lines_text(&review_point_evidence_lines(&op, &point));

        assert!(text.contains("FAILURE (after tick 02)"));
        assert!(text.contains("action        (none — execution stopped)"));
        assert!(text.contains("OPERATION FAILED: controller script error"));
    }

    #[test]
    fn review_point_evidence_lines_for_a_first_tick_failure_names_no_preceding_tick() {
        let op = test_operation_view(&[]);
        let point = ReviewPoint {
            kind: ReviewPointKind::TerminalFailure(&ControllerError::MissingCallback),
            snapshot: test_snapshot(0, 15),
            newly_discovered: Vec::new(),
        };
        let text = lines_text(&review_point_evidence_lines(&op, &point));

        assert!(text.contains("FAILURE (before any tick completed)"));
        assert!(text.contains("action        (none — execution stopped)"));
    }
}
