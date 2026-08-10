//! Renders the resistance console's persistent frame and per-view content.
//!
//! Every function here is a pure `(state, frame area) -> drawn widgets`
//! operation so it can be exercised against `ratatui`'s `TestBackend`
//! without a real terminal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::intel::{Signal, TargetDossier, authored_signals, first_contact_dossier};
use super::state::{AppState, Validation, View, WorkingSet};

pub const MIN_COLUMNS: u16 = 80;
pub const MIN_ROWS: u16 = 24;

/// Below this width, Signals and Target fall back to a single primary pane
/// with `F8` toggling to the secondary one, per `docs/TUI_DESIGN.md`
/// ("Responsive behavior").
const TWO_PANE_MIN_COLUMNS: u16 = 100;

const TITLE: &str = "HUMAN EXCEPTION // RESISTANCE CONSOLE";

/// Draws the full console frame for the current session state.
pub fn draw(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    if area.width < MIN_COLUMNS || area.height < MIN_ROWS {
        draw_geometry_warning(frame, area);
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);

    draw_header(frame, header, state);
    draw_body(frame, body, state);
    draw_footer(frame, footer, state, area.width);
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
    let candidates: Vec<String> = match state.current_view() {
        View::Signals => {
            let signals = format!("SIGNALS: {:02}", authored_signals().len());
            vec![
                format!("MESH: DEGRADED   {signals}   {working}"),
                format!("{signals}   {working}"),
            ]
        }
        View::Target => {
            let dossier = first_contact_dossier();
            let target = format!("TARGET: {}", dossier.title);
            let confidence = format!("CONFIDENCE: {}", dossier.confidence_summary);
            vec![
                format!("MESH: DEGRADED   {target}   {confidence}   {working}"),
                // Confidence is the lowest-priority field here — drop it,
                // not MESH, so Target doesn't lose link condition while
                // every other view keeps it at the same widths.
                format!("MESH: DEGRADED   {target}   {working}"),
                format!("{target}   {confidence}   {working}"),
                format!("{target}   {working}"),
            ]
        }
        _ => match state.controller_source() {
            Some(_) => {
                let status = if matches!(state.validation(), Validation::Invalid(_)) {
                    "invalid"
                } else if state.controller_modified() {
                    "modified"
                } else {
                    "starter"
                };
                vec![
                    format!(
                        "MESH: DEGRADED   SATLINK: COMPROMISED   CONTROLLER: {status}   {working}"
                    ),
                    format!("SATLINK: COMPROMISED   CONTROLLER: {status}   {working}"),
                    format!("CONTROLLER: {status}   {working}"),
                    working.clone(),
                ]
            }
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

fn draw_body(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.current_view() {
        View::Signals => draw_signals(frame, area, state),
        View::Target => draw_target(frame, area, state),
        View::Help => draw_help(frame, area, state),
        View::Controller => draw_controller(frame, area, state),
        view => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(view_title(view));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(Paragraph::new(view_body(view)), inner);
        }
    }
}

fn draw_controller(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)])
                .areas(area);
        draw_controller_source(frame, left, state);
        draw_pane(
            frame,
            right,
            "LUA FIELD REFERENCE",
            lua_field_reference_lines(),
        );
    } else if state.narrow_secondary_visible() {
        draw_pane(
            frame,
            area,
            "LUA FIELD REFERENCE",
            lua_field_reference_lines(),
        );
    } else {
        draw_controller_source(frame, area, state);
    }
}

fn draw_controller_source(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("CAPTURED CONTROLLER // controller.lua");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let banner = controller_banner(state);
    let (content_area, banner_area) = if banner.is_some() {
        let [content, banner_row] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        (content, Some(banner_row))
    } else {
        (inner, None)
    };

    let source = state.controller_source().unwrap_or_default();
    let (cursor_line, cursor_col) = state.controller_cursor().unwrap_or((0, 0));
    let lines = controller_editor_lines(source, cursor_line, cursor_col);
    let viewport_height = content_area.height as usize;
    let first_visible = first_visible_line(cursor_line, lines.len(), viewport_height);
    let visible_lines: Vec<Line<'static>> = lines
        .into_iter()
        .skip(first_visible)
        .take(viewport_height)
        .collect();
    frame.render_widget(Paragraph::new(visible_lines), content_area);

    if let (Some(banner_area), Some(banner)) = (banner_area, banner) {
        frame.render_widget(Paragraph::new(banner), banner_area);
    }
}

/// The confirmation prompt or validation result shown as a fixed last row
/// under the source, or `None` when there's nothing to say (an unmodified,
/// unchecked controller keeps the full pane for source).
fn controller_banner(state: &AppState) -> Option<Line<'static>> {
    if state.quit_confirmation_pending() {
        return Some(Line::from(Span::styled(
            "Quit? Modified controller source will be lost. Enter/y confirm  Esc/n cancel",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    if state.reset_confirmation_pending() {
        return Some(Line::from(Span::styled(
            "Reset controller? Edits will be lost. Enter/y confirm  Esc/n cancel",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    match state.validation() {
        Validation::Unchecked => None,
        Validation::Valid => Some(Line::from(Span::styled(
            "READY: controller loads and defines on_tick",
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        Validation::Invalid(message) => Some(Line::from(Span::styled(
            format!("INVALID: {message}"),
            Style::default().add_modifier(Modifier::BOLD),
        ))),
    }
}

/// Renders `source` as line-numbered rows with the cursor shown as a
/// reversed-style character (or a reversed space at end-of-line/on an empty
/// line), one [`Line`] per source line.
fn controller_editor_lines(
    source: &str,
    cursor_line: usize,
    cursor_col: usize,
) -> Vec<Line<'static>> {
    let raw_lines: Vec<&str> = source.split('\n').collect();
    let gutter_width = raw_lines.len().to_string().len().max(2);
    raw_lines
        .iter()
        .enumerate()
        .map(|(idx, text)| {
            let number = Span::styled(
                format!("{:>gutter_width$} ", idx + 1),
                Style::default().add_modifier(Modifier::DIM),
            );
            let mut spans = vec![number];
            if idx == cursor_line {
                spans.extend(cursor_line_spans(text, cursor_col));
            } else {
                spans.push(Span::raw(text.to_string()));
            }
            Line::from(spans)
        })
        .collect()
}

fn cursor_line_spans(text: &str, cursor_col: usize) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let cursor_style = Style::default().add_modifier(Modifier::REVERSED);
    if cursor_col >= chars.len() {
        let mut spans = Vec::new();
        if !chars.is_empty() {
            spans.push(Span::raw(chars.iter().collect::<String>()));
        }
        spans.push(Span::styled(" ", cursor_style));
        spans
    } else {
        let before: String = chars[..cursor_col].iter().collect();
        let at: String = chars[cursor_col].to_string();
        let after: String = chars[cursor_col + 1..].iter().collect();
        vec![
            Span::raw(before),
            Span::styled(at, cursor_style),
            Span::raw(after),
        ]
    }
}

/// The first source line to render so that `cursor_line` stays within a
/// `viewport_height`-row window, without scrolling past the end of a
/// document shorter than the viewport.
fn first_visible_line(cursor_line: usize, total_lines: usize, viewport_height: usize) -> usize {
    if viewport_height == 0 {
        return 0;
    }
    let max_first = total_lines.saturating_sub(viewport_height);
    let wanted_first = cursor_line.saturating_sub(viewport_height.saturating_sub(1));
    wanted_first.min(max_first)
}

/// A short, representative subset of the Lua contract shown as a cheat
/// sheet next to (or, at narrow widths, instead of) the editor. See
/// `help_lines`'s "Lua contract" section for the complete reference; the
/// two are checked for consistency in tests so they can't silently drift.
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
        Line::from("F1 opens the complete reference"),
    ]
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

/// Placeholder body content for views this issue doesn't populate.
fn view_body(view: View) -> Vec<Line<'static>> {
    match view {
        View::Operation => vec![
            Line::from("No operation is deployed yet."),
            Line::from("The compromised satellite feed and telemetry will appear here (see #45)."),
        ],
        View::AfterAction => vec![
            Line::from("No operation has concluded yet."),
            Line::from("The after-action report will appear here (see #46)."),
        ],
        View::Signals | View::Target | View::Controller | View::Help => Vec::new(),
    }
}

fn draw_pane(frame: &mut Frame, area: Rect, title: &'static str, lines: Vec<Line<'static>>) {
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
    title: &'static str,
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

    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);
        draw_pane(frame, left, "SIGNALS", signal_list_lines(state));
        draw_signal_detail_pane(frame, right, signal);
    } else if state.narrow_secondary_visible() {
        draw_signal_detail_pane(frame, area, signal);
    } else {
        draw_pane(frame, area, "SIGNALS", signal_list_lines(state));
    }
}

fn draw_signal_detail_pane(frame: &mut Frame, area: Rect, signal: &Signal) {
    if signal.is_actionable() {
        draw_pane_with_pinned_action(
            frame,
            area,
            "SELECTED SIGNAL",
            signal_detail_lines(signal),
            "Enter  inspect opportunity",
        );
    } else {
        draw_pane(frame, area, "SELECTED SIGNAL", signal_detail_lines(signal));
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

    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(area);
        draw_pane_with_pinned_action(
            frame,
            left,
            "TARGET INTELLIGENCE",
            target_intel_lines(&dossier),
            "Enter  work this opportunity",
        );
        draw_pane_with_pinned_action(
            frame,
            right,
            "PROVENANCE / ACCESS",
            target_provenance_lines(&dossier),
            "Esc  back to signals",
        );
    } else if state.narrow_secondary_visible() {
        draw_pane_with_pinned_action(
            frame,
            area,
            "PROVENANCE / ACCESS",
            target_provenance_lines(&dossier),
            "Esc  back to signals",
        );
    } else {
        draw_pane_with_pinned_action(
            frame,
            area,
            "TARGET INTELLIGENCE",
            target_intel_lines(&dossier),
            "Enter  work this opportunity",
        );
    }
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

fn draw_help(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(view_title(View::Help));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = help_lines(state);
    // The event loop already clamps the stored offset via `help_max_scroll`
    // as each scroll key arrives; this recomputes against the exact `inner`
    // Rect as a second, render-time backstop (e.g. for the very first draw,
    // or a caller that renders without going through that event loop).
    let content_rows = wrapped_row_count(&lines, inner.width);
    let max_scroll = content_rows.saturating_sub(inner.height as usize) as u16;
    let scroll = state.help_scroll().min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);
}

/// How many terminal rows `lines` occupies once wrapped to `width`, matching
/// the `Wrap { trim: false }` behavior used to render Help.
fn wrapped_row_count(lines: &[Line<'static>], width: u16) -> usize {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            let line_width = line.width();
            if line_width == 0 {
                1
            } else {
                line_width.div_ceil(width)
            }
        })
        .sum()
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
    lines.push(Line::from(
        "F6 Deploy        run the current controller (unavailable, see #45)",
    ));
    lines.push(Line::from(if state.view_available(View::Controller) {
        "F7 Reset         (Controller) restore the starter controller"
    } else {
        "F7 Reset         (Controller, unavailable: work an opportunity first)"
    }));
    lines.push(Line::from(
        "Ctrl+Enter       (Controller) check the source loads, without running it",
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
            Line::from("at 100+ columns its detail shows alongside the list automatically;"),
            Line::from("at 80-99 columns, F8 switches to that detail"),
        ],
        View::Target => vec![
            Line::from("Enter  work this opportunity"),
            Line::from("Esc    back to Signals"),
            Line::from("F8     (80-99 columns) switch between intel and provenance"),
        ],
        View::Controller => vec![
            Line::from("Type to edit; arrows/Home/End/PageUp/PageDown move the cursor"),
            Line::from("F7          reset to the starter controller (confirms if modified)"),
            Line::from("Ctrl+Enter  check whether the source loads, without running it"),
            Line::from("F8          (80-99 columns) switch between source and reference"),
        ],
        View::Operation => vec![Line::from("Live telemetry arrives in a later build (#45).")],
        View::AfterAction => vec![Line::from(
            "After-action reporting arrives in a later build (#46).",
        )],
        View::Help => Vec::new(),
    }
}

/// `(full label, compact label, enabled)` for each footer hint. The compact
/// form is used whenever the full labels would crowd out the `Ctrl+Q Quit`
/// hint, which must always stay visible.
fn footer_hint_items(state: &AppState, show_f8: bool) -> Vec<(&'static str, &'static str, bool)> {
    let mut items = vec![
        ("F1 Help", "F1 Help", true),
        ("F2 Signals", "F2 Sig", true),
        ("F3 Target", "F3 Tgt", state.view_available(View::Target)),
        (
            "F4 Controller",
            "F4 Ctl",
            state.view_available(View::Controller),
        ),
        (
            "F5 Operation",
            "F5 Op",
            state.view_available(View::Operation),
        ),
        // F6 has no operation to deploy yet (see #45).
        ("F6 Deploy", "F6 Dep", false),
    ];
    if state.current_view() == View::Controller {
        let has_controller = state.controller_source().is_some();
        items.push(("F7 Reset", "F7 Rst", has_controller));
        items.push(("Ctrl+Enter Validate", "^Enter Val", has_controller));
    }
    if show_f8 {
        items.push(("F8 Toggle Pane", "F8 Pane", true));
    }
    items
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

fn draw_footer(frame: &mut Frame, area: Rect, state: &AppState, full_width: u16) {
    let show_f8 = full_width < TWO_PANE_MIN_COLUMNS
        && matches!(
            state.current_view(),
            View::Signals | View::Target | View::Controller
        );
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
    fn narrow_signals_view_shows_one_pane_until_f8_toggles_it() {
        let state = AppState::new();
        let terminal = render(90, 30, &state);

        assert!(buffer_contains(&terminal, "SIGNALS"));
        assert!(!buffer_contains(&terminal, "SELECTED SIGNAL"));
    }

    #[test]
    fn narrow_signals_view_shows_detail_pane_once_toggled() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::ToggleSecondaryPane);
        let terminal = render(90, 30, &state);

        assert!(buffer_contains(&terminal, "SELECTED SIGNAL"));
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
    fn target_view_at_the_two_pane_threshold_still_shows_the_pinned_actions() {
        use super::super::state::Msg;

        // 100x24: the narrowest width that switches Target into the
        // two-pane layout, where a 55% intelligence pane wraps enough known
        // facts and the opportunity blurb that the action row would be the
        // first thing pushed off-screen if it weren't pinned separately.
        let mut state = AppState::new();
        state.apply(Msg::Activate);
        let terminal = render(TWO_PANE_MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "work this opportunity"));
        assert!(buffer_contains(&terminal, "back to signals"));
    }

    #[test]
    fn signal_detail_pane_at_the_two_pane_threshold_still_shows_the_pinned_action() {
        let state = AppState::new();
        let terminal = render(TWO_PANE_MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "inspect opportunity"));
    }

    #[test]
    fn footer_keeps_the_quit_hint_visible_at_minimum_width() {
        let state = AppState::new();
        let terminal = render(MIN_COLUMNS, MIN_ROWS, &state);

        assert!(buffer_contains(&terminal, "Ctrl+Q Quit"));
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
        let terminal = render(120, 60, &state);

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
    fn signals_help_describes_the_narrow_layout_toggle_accurately() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 40, &state);

        assert!(!buffer_contains(&terminal, "follows automatically"));
        assert!(buffer_contains(&terminal, "at 80-99 columns, F8 switches"));
    }

    #[test]
    fn help_lists_signal_terminology_and_satellite_symbols() {
        use super::super::state::Msg;

        // Tall enough that the full two-level help content (view controls,
        // global controls, Lua contract, terminology, symbol legend) fits
        // without needing to scroll.
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        let terminal = render(120, 70, &state);

        assert!(buffer_contains(&terminal, "Terminology"));
        assert!(buffer_contains(&terminal, "MACHINE INTERCEPT"));
        assert!(buffer_contains(&terminal, "Satellite symbols"));
        assert!(buffer_contains(&terminal, "D drone"));
    }

    #[test]
    fn help_at_a_tall_viewport_does_not_scroll_past_its_own_content() {
        use super::super::state::Msg;

        // At this height the full Help document already fits without
        // scrolling; MAX_HELP_SCROLL alone would still let repeated Down
        // presses blank the pane, so the render-time clamp must catch it.
        let mut state = AppState::new();
        state.apply(Msg::OpenHelp);
        for _ in 0..60 {
            state.apply(Msg::ScrollHelpDown);
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
}
