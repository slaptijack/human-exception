//! Renders the resistance console's persistent frame and per-view content.
//!
//! Every function here is a pure `(state, frame area) -> drawn widgets`
//! operation so it can be exercised against `ratatui`'s `TestBackend`
//! without a real terminal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::{AppState, View, WorkingSet};

pub const MIN_COLUMNS: u16 = 80;
pub const MIN_ROWS: u16 = 24;

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
    draw_footer(frame, footer);
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
    let status = Line::from(format!(
        "MESH: DEGRADED   SATLINK: COMPROMISED   WORKING SET: {working_set}"
    ));

    let block = Block::default().borders(Borders::ALL).title(TITLE);
    frame.render_widget(Paragraph::new(status).block(block), area);
}

fn draw_body(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(view_title(state.current_view()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(view_body(state.current_view())), inner);
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

fn view_body(view: View) -> Vec<Line<'static>> {
    match view {
        View::Signals => vec![
            Line::from("No signals have reached this console yet."),
            Line::from("Intercepted traffic and shared intel will appear here (see #43)."),
        ],
        View::Target => vec![
            Line::from("No target dossier is available yet."),
            Line::from("Provenance, knowns, unknowns, and opportunity will appear here (see #43)."),
        ],
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
        View::Help => vec![
            Line::from(Span::styled(
                "Global controls",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from("F1 Help          toggle this overlay"),
            Line::from("F2 Signals       the intelligence stream"),
            Line::from("F3 Target        dossier for the current opportunity"),
            Line::from("F4 Controller    the Lua editor for the working set"),
            Line::from("F5 Operation     the live satellite/telemetry view"),
            Line::from("F6 Deploy        run the current controller"),
            Line::from("Ctrl+Q Quit      exit and restore the terminal"),
        ],
    }
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let hints = Line::from(
        "F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy                Ctrl+Q Quit",
    );
    frame.render_widget(
        Paragraph::new(hints).block(Block::default().borders(Borders::ALL)),
        area,
    );
}
