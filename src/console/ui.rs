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
use super::state::{AppState, View, WorkingSet};

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

    let extra = match state.current_view() {
        View::Signals => format!("SIGNALS: {:02}", authored_signals().len()),
        View::Target => {
            let dossier = first_contact_dossier();
            format!(
                "TARGET: {}   CONFIDENCE: {}",
                dossier.title, dossier.confidence_summary
            )
        }
        _ => "SATLINK: COMPROMISED".to_string(),
    };

    let status = Line::from(format!(
        "MESH: DEGRADED   {extra}   WORKING SET: {working_set}"
    ));

    let block = Block::default().borders(Borders::ALL).title(TITLE);
    frame.render_widget(Paragraph::new(status).block(block), area);
}

fn draw_body(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.current_view() {
        View::Signals => draw_signals(frame, area, state),
        View::Target => draw_target(frame, area, state),
        View::Help => draw_help(frame, area, state),
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
        View::Controller => vec![
            Line::from("No controller is loaded yet."),
            Line::from("The captured-controller Lua editor will appear here (see #44)."),
        ],
        View::Operation => vec![
            Line::from("No operation is deployed yet."),
            Line::from("The compromised satellite feed and telemetry will appear here (see #45)."),
        ],
        View::AfterAction => vec![
            Line::from("No operation has concluded yet."),
            Line::from("The after-action report will appear here (see #46)."),
        ],
        View::Signals | View::Target | View::Help => Vec::new(),
    }
}

fn draw_pane(frame: &mut Frame, area: Rect, title: &'static str, lines: Vec<Line<'static>>) {
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_signals(frame: &mut Frame, area: Rect, state: &AppState) {
    let signal = &authored_signals()[state.selected_signal()];

    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);
        draw_pane(frame, left, "SIGNALS", signal_list_lines(state));
        draw_pane(frame, right, "SELECTED SIGNAL", signal_detail_lines(signal));
    } else if state.narrow_secondary_visible() {
        draw_pane(frame, area, "SELECTED SIGNAL", signal_detail_lines(signal));
    } else {
        draw_pane(frame, area, "SIGNALS", signal_list_lines(state));
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
        lines.push(Line::from("Enter  inspect opportunity"));
    }
    lines
}

fn draw_target(frame: &mut Frame, area: Rect, state: &AppState) {
    let dossier = first_contact_dossier();

    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(area);
        draw_pane(
            frame,
            left,
            "TARGET INTELLIGENCE",
            target_intel_lines(&dossier),
        );
        draw_pane(
            frame,
            right,
            "PROVENANCE / ACCESS",
            target_provenance_lines(&dossier),
        );
    } else if state.narrow_secondary_visible() {
        draw_pane(
            frame,
            area,
            "PROVENANCE / ACCESS",
            target_provenance_lines(&dossier),
        );
    } else {
        draw_pane(
            frame,
            area,
            "TARGET INTELLIGENCE",
            target_intel_lines(&dossier),
        );
    }
}

fn target_intel_lines(dossier: &TargetDossier) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            dossier.title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(dossier.location),
        Line::from(""),
        Line::from(Span::styled(
            "KNOWN",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    for fact in dossier.known {
        lines.push(Line::from(format!("- {fact}")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "UNKNOWN",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for fact in dossier.unknown {
        lines.push(Line::from(format!("- {fact}")));
    }
    lines.push(Line::from(""));
    for line in dossier.opportunity_text {
        lines.push(Line::from(*line));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Enter  work this opportunity"));
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
    lines.push(Line::from(""));
    lines.push(Line::from("Esc  back to signals"));
    lines
}

fn draw_help(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(view_title(View::Help));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let paragraph = Paragraph::new(help_lines(state))
        .wrap(Wrap { trim: false })
        .scroll((state.help_scroll(), 0));
    frame.render_widget(paragraph, inner);
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
        "Global controls",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from("F1 Help          toggle this overlay"));
    lines.push(Line::from("F2 Signals       the intelligence stream"));
    lines.push(Line::from(
        "F3 Target        dossier for the current opportunity",
    ));
    lines.push(Line::from(
        "F4 Controller    the Lua editor for the working set",
    ));
    lines.push(Line::from(
        "F5 Operation     the live satellite/telemetry view",
    ));
    lines.push(Line::from(
        "F6 Deploy        run the current controller (unavailable, see #44/#45)",
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
    lines.push(Line::from(""));
    lines.push(Line::from(
        "valid return values: north south east west wait scan",
    ));
    lines.push(Line::from(format!(
        "each action costs {} budget; entering a hazard tile costs {} more",
        crate::simulation::ACTION_COST,
        crate::simulation::HAZARD_ENTRY_COST
    )));

    lines
}

fn view_specific_help(view: View) -> Vec<Line<'static>> {
    match view {
        View::Signals => vec![
            Line::from("Up/Down  move between signals"),
            Line::from("Enter    inspect the selected signal (opens Target if actionable)"),
        ],
        View::Target => vec![
            Line::from("Enter  work this opportunity"),
            Line::from("Esc    back to Signals"),
        ],
        View::Controller => vec![Line::from("The Lua editor arrives in a later build (#44).")],
        View::Operation => vec![Line::from("Live telemetry arrives in a later build (#45).")],
        View::AfterAction => vec![Line::from(
            "After-action reporting arrives in a later build (#46).",
        )],
        View::Help => Vec::new(),
    }
}

fn footer_hint_items(state: &AppState) -> Vec<(&'static str, bool)> {
    vec![
        ("F1 Help", true),
        ("F2 Signals", true),
        ("F3 Target", state.view_available(View::Target)),
        ("F4 Controller", state.view_available(View::Controller)),
        ("F5 Operation", state.view_available(View::Operation)),
        // F6 has no prerequisite controller to deploy yet (see #44/#45).
        ("F6 Deploy", false),
    ]
}

const FOOTER_RIGHT_HINT: &str = "Ctrl+Q Quit";

fn draw_footer(frame: &mut Frame, area: Rect, state: &AppState, full_width: u16) {
    let mut items = footer_hint_items(state);
    if full_width < TWO_PANE_MIN_COLUMNS
        && matches!(state.current_view(), View::Signals | View::Target)
    {
        items.push(("F8 Toggle Pane", true));
    }

    let mut spans = Vec::with_capacity(items.len() * 2 + 2);
    let mut left_len = 0usize;
    for (index, (label, enabled)) in items.iter().enumerate() {
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

    let inner_width = area.width.saturating_sub(2) as usize;
    let used = left_len + 1 + FOOTER_RIGHT_HINT.len();
    let padding = " ".repeat(inner_width.saturating_sub(used).max(1));

    spans.push(Span::raw(padding));
    spans.push(Span::raw(FOOTER_RIGHT_HINT));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL)),
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
        assert!(buffer_contains(&terminal, "inspect the selected signal"));
        assert!(buffer_contains(&terminal, "Global controls"));
        assert!(buffer_contains(&terminal, "on_tick(observation)"));
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
}
