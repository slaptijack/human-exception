//! Renders the resistance console's persistent frame and per-view content.
//!
//! Every function here is a pure `(state, frame area) -> drawn widgets`
//! operation so it can be exercised against `ratatui`'s `TestBackend`
//! without a real terminal.

use super::intel::{Signal, TargetDossier, authored_signals, first_contact_dossier};
use super::state::{
    AppState, ConclusionKind, OperationSnapshot, OperationView, Validation, View, WorkingSet,
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
use unicode_segmentation::UnicodeSegmentation;

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

    // Checked before the undersized-geometry return, and drawn without the
    // header/body/footer layout that return skips past: `Ctrl+Q` is global
    // and the confirmation must stay reachable at any size (`docs/
    // TUI_DESIGN.md`, "Below 80 columns" — "Quitting remains available,
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

/// Whether Controller's source pane (rather than the Lua reference pane
/// `F8` can swap in at 80-99 columns) is what's actually on screen for
/// `frame_width`. Exposed so the event mapper can gate editing keys on it —
/// typing while the reference pane is showing must not silently edit an
/// invisible source (see `event::map`).
pub(crate) fn controller_source_visible(state: &AppState, frame_width: u16) -> bool {
    frame_width >= TWO_PANE_MIN_COLUMNS
        || !state.narrow_secondary_visible()
        || state.reset_confirmation_pending()
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
    } else if state.narrow_secondary_visible() && !state.reset_confirmation_pending() {
        // Unlike the wide two-pane layout above (where the source pane and
        // its banner are always visible alongside the reference), this is
        // the only place the reference pane can be shown *instead of* the
        // source, so validation/reset feedback needs its own copy of the
        // banner here too — otherwise pressing Ctrl+Enter/Ctrl+V while
        // looking at the reference would appear to do nothing.
        draw_controller_reference(frame, area, state);
    } else {
        // A pending reset confirmation always wins the narrow-layout toggle:
        // its banner only ever renders inside the source pane, so showing
        // the reference pane instead while it's pending would leave the
        // prompt (and the `Enter`/`Esc` it's waiting on) invisible.
        draw_controller_source(frame, area, state);
    }
}

/// The narrow-layout (80-99 column) stand-in for the source pane when `F8`
/// has swapped the Lua reference in instead. Renders the same validation
/// banner `draw_controller_source` would, since that pane isn't on screen
/// to show it.
fn draw_controller_reference(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("LUA FIELD REFERENCE");
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

    frame.render_widget(
        Paragraph::new(lua_field_reference_lines()).wrap(Wrap { trim: false }),
        content_area,
    );

    if let (Some(banner_area), Some(banner)) = (banner_area, banner) {
        frame.render_widget(
            Paragraph::new(banner).wrap(Wrap { trim: false }),
            banner_area,
        );
    }
}

/// An upper bound on how many rows [`controller_banner`] can claim, so one
/// long message can't swallow the whole pane.
const MAX_BANNER_ROWS: u16 = 4;

/// How many rows to reserve for `banner` at `width`, wrapping instead of
/// clipping a message that runs past one row — a Lua syntax error easily
/// exceeds the supported 80-column pane's width, and clipping it can lose
/// the `:line:` location `docs/TUI_DESIGN.md` requires stay visible.
fn banner_height(banner: &Line<'static>, width: u16) -> u16 {
    (wrapped_row_count(std::slice::from_ref(banner), width) as u16).clamp(1, MAX_BANNER_ROWS)
}

fn draw_controller_source(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("CAPTURED CONTROLLER // controller.lua");
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

    let source = state.controller_source().unwrap_or_default();
    let (cursor_line, cursor_col) = state.controller_cursor().unwrap_or((0, 0));
    let total_lines = source.split('\n').count();
    let gutter_width = source_gutter_width(total_lines);
    let text_width = (content_area.width as usize).saturating_sub(gutter_width);
    // Scrolling is computed in terminal display cells, not `char`s: a
    // double-width character (e.g. CJK text in a comment or string) still
    // costs two columns on screen even though it's one `char`, so indexing
    // by `char` count alone can leave the cursor's true screen column
    // outside a narrow pane while this thinks it's still visible.
    let cursor_line_text = source.split('\n').nth(cursor_line).unwrap_or("");
    let cursor_line_chars: Vec<char> = cursor_line_text.chars().collect();
    // The cursor's highlighted unit is a whole grapheme cluster (see
    // `cursor_grapheme_char_range`), not necessarily the single `char` at
    // `cursor_col` — `cursor_col` can land on a zero-width character (a
    // combining mark, or a variation selector like U+FE0F) whose own
    // `unicode-width` value says nothing about the *combined* cell width
    // ratatui's `CellWidth` actually renders for the pair. Measuring only
    // that one `char` could under-reserve scroll room for a wider unit,
    // clipping it off the pane's edge entirely.
    let (cursor_start_col, cursor_glyph_width) = if cursor_col < cursor_line_chars.len() {
        let (at_start, at_end) = cursor_grapheme_char_range(cursor_line_text, cursor_col);
        let glyph: String = cursor_line_chars[at_start..at_end].iter().collect();
        (at_start, (glyph.as_str().cell_width() as usize).max(1))
    } else {
        (cursor_col, 1usize) // past the last character: the synthetic trailing cursor cell
    };
    let cursor_display_col = display_width_of_prefix(cursor_line_text, cursor_start_col);
    // Scroll far enough to fit the cursor glyph's *far* edge, not just its
    // start column: reserving only one cell for a double-width character
    // (e.g. CJK) leaves it straddling the pane's right edge, and ratatui
    // won't render a two-cell glyph that doesn't fully fit, making the
    // cursor disappear.
    let cursor_display_end = cursor_display_col + cursor_glyph_width - 1;
    let first_visible_cell = first_visible_offset(cursor_display_end, text_width);
    let lines = controller_editor_lines(source, cursor_line, cursor_col, first_visible_cell);
    let viewport_height = content_area.height as usize;
    let first_visible_row = first_visible_offset(cursor_line, viewport_height);
    // Clamp is separate from `first_visible_offset` itself so a document
    // shorter than the viewport never scrolls past its own last line, which
    // matters vertically (there's a fixed document length to respect) but
    // not horizontally (each line has its own length, so there's no single
    // "last column" to clamp against).
    let max_first_row = total_lines.saturating_sub(viewport_height);
    let first_visible_row = first_visible_row.min(max_first_row);
    let visible_lines: Vec<Line<'static>> = lines
        .into_iter()
        .skip(first_visible_row)
        .take(viewport_height)
        .collect();
    frame.render_widget(Paragraph::new(visible_lines), content_area);

    if let (Some(banner_area), Some(banner)) = (banner_area, banner) {
        frame.render_widget(
            Paragraph::new(banner).wrap(Wrap { trim: false }),
            banner_area,
        );
    }
}

/// The confirmation prompt or validation result shown as a fixed last row
/// under the source, or `None` when there's nothing to say (an unmodified,
/// unchecked controller keeps the full pane for source).
fn controller_banner(state: &AppState) -> Option<Line<'static>> {
    // The quit-confirmation prompt is drawn globally by `draw` (it can be
    // triggered from any view), so it isn't handled here even though it's
    // Controller-adjacent state.
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

/// The gutter column width (digits plus one trailing space) for a document
/// of `total_lines` lines. Shared between the line-rendering and the
/// horizontal-viewport-width calculation so they can't drift apart.
fn source_gutter_width(total_lines: usize) -> usize {
    total_lines.to_string().len().max(2) + 1
}

/// Renders `source` as line-numbered rows with the cursor shown as a
/// reversed-style character (or a reversed space at end-of-line/on an empty
/// line), one [`Line`] per source line. `first_visible_col` horizontally
/// scrolls every line together (the same offset for all of them, as
/// ordinary editors do), so a line longer than the pane doesn't leave the
/// cursor invisible off the right edge once it moves or types past it.
fn controller_editor_lines(
    source: &str,
    cursor_line: usize,
    cursor_col: usize,
    first_visible_cell: usize,
) -> Vec<Line<'static>> {
    let raw_lines: Vec<&str> = source.split('\n').collect();
    let gutter_width = source_gutter_width(raw_lines.len()) - 1;
    raw_lines
        .iter()
        .enumerate()
        .map(|(idx, text)| {
            let number = Span::styled(
                format!("{:>gutter_width$} ", idx + 1),
                Style::default().add_modifier(Modifier::DIM),
            );
            let mut spans = vec![number];
            let skip_chars = chars_to_skip_for_cell_offset(text, first_visible_cell);
            let visible: String = text.chars().skip(skip_chars).collect();
            if idx == cursor_line {
                let visible_cursor_col = cursor_col.saturating_sub(skip_chars);
                spans.extend(cursor_line_spans(&visible, visible_cursor_col));
            } else {
                spans.push(Span::raw(visible));
            }
            Line::from(spans)
        })
        .collect()
}

/// The display-cell width of the first `char_count` characters of `text`,
/// computed via ratatui's own [`CellWidth`] — the exact same calculation
/// `Buffer`/`Paragraph` use to lay out and clip rendered text — rather than
/// summing each character's individual `unicode-width` value in isolation.
/// Those can disagree: `CellWidth` applies a terminal-compatibility
/// adjustment for a few grapheme-forming character combinations (e.g.
/// halfwidth katakana dakuten/handakuten marks, which `unicode-width`
/// reports as zero-width on their own but which terminals render as an
/// extra occupied cell) that a naive per-character sum misses entirely,
/// under- or over-counting exactly the columns this function's callers use
/// to decide what's on screen and where the cursor cell actually falls.
fn display_width_of_prefix(text: &str, char_count: usize) -> usize {
    let prefix: String = text.chars().take(char_count).collect();
    prefix.cell_width() as usize
}

/// How many leading characters of `text` to skip so that they, combined,
/// consume at least `cells` display-cell columns — the character-count
/// equivalent of a cell-based horizontal scroll offset, since `chars()`
/// iterates by character, not display width. Uses the same [`CellWidth`]
/// calculation as [`display_width_of_prefix`], for the same reason: it
/// must agree with ratatui's own rendering, not an independent per-character
/// unicode-width sum that can diverge from it.
fn chars_to_skip_for_cell_offset(text: &str, cells: usize) -> usize {
    let mut consumed = 0usize;
    let mut skip = 0usize;
    // Advances by whole extended grapheme clusters, not individual
    // `char`s: `cells` can land partway through a multi-scalar cluster
    // (e.g. a ZWJ sequence like "👩‍💻"), and skipping only the leading
    // scalars of one would leave the visible line starting mid-cluster —
    // a split glyph ratatui can't render as the single unit it actually
    // is, at a horizontal-scroll offset the cursor's own grapheme-aware
    // positioning (`cursor_grapheme_char_range`) assumes lines always
    // start on a real boundary.
    for grapheme in text.graphemes(true) {
        if consumed >= cells {
            break;
        }
        consumed += grapheme.cell_width() as usize;
        skip += grapheme.chars().count();
    }
    skip
}

/// The char-index range `[at_start, at_end)` of the extended grapheme
/// cluster (per Unicode's own segmentation rules, via `unicode-segmentation`
/// — the same algorithm ratatui uses internally to lay out and measure
/// text) containing `cursor_col` within `text`. Real terminals — and
/// ratatui's own `CellWidth` — render a whole cluster as one visual unit,
/// which is not always "a character plus its immediately adjacent
/// zero-width neighbors": a zero-width joiner sequence (`👩‍💻`, where
/// neither the woman nor the computer emoji is itself zero-width — only
/// the joiner between them is) or a regional-indicator pair forming a flag
/// (`🇺🇸`, where *neither* component is zero-width) both need real
/// grapheme-boundary rules to group correctly. Treating each `char` as
/// independent, or only merging zero-width neighbors, can split a cluster
/// across a style boundary or under-measure its true rendered width.
/// Shared by [`cursor_line_spans`] (which highlighting group to render as
/// one reversed unit) and `draw_controller_source` (how many display cells
/// that unit actually costs when deciding whether it fits the visible
/// viewport).
fn cursor_grapheme_char_range(text: &str, cursor_col: usize) -> (usize, usize) {
    let mut char_idx = 0usize;
    for grapheme in text.graphemes(true) {
        let end = char_idx + grapheme.chars().count();
        if cursor_col < end {
            return (char_idx, end);
        }
        char_idx = end;
    }
    // `cursor_col` is at or past the end of `text`; callers handle the
    // "past the last character" case themselves, but fall back to a
    // single-position range just in case.
    (cursor_col, cursor_col + 1)
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
        return spans;
    }

    let (at_start, at_end) = cursor_grapheme_char_range(text, cursor_col);

    let before: String = chars[..at_start].iter().collect();
    let at: String = chars[at_start..at_end].iter().collect();
    let after: String = chars[at_end..].iter().collect();
    vec![
        Span::raw(before),
        Span::styled(at, cursor_style),
        Span::raw(after),
    ]
}

/// The first line/column to render so that `cursor` stays within a
/// `viewport_len`-cell window, scrolling only as far as needed. Used both
/// vertically (rows) and horizontally (columns); the vertical caller
/// additionally clamps against the document's total length (see
/// `draw_controller_source`) so it never scrolls past a short document's
/// last line — there's no equivalent "last column" to clamp against
/// horizontally, since each line has its own length.
fn first_visible_offset(cursor: usize, viewport_len: usize) -> usize {
    if viewport_len == 0 {
        return 0;
    }
    cursor.saturating_sub(viewport_len.saturating_sub(1))
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
/// secondary (`docs/TUI_DESIGN.md` §4, "Operation"). Two panes at 100+
/// columns; below that, one primary pane with `F8` swapping to the other,
/// reusing the same `narrow_secondary_visible` toggle Controller and
/// Signals already use.
fn draw_operation(frame: &mut Frame, area: Rect, state: &AppState) {
    // Wins over the normal layout regardless of width, the same way a
    // pending reset confirmation always wins Controller's narrow-layout
    // toggle: the prompt (and the `Enter`/`Esc` it's waiting on) must never
    // end up on the pane the player currently isn't looking at.
    if state.redeploy_confirmation_pending() {
        draw_pane(
            frame,
            area,
            "REDEPLOY?",
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
            "OPERATION",
            vec![
                Line::from("No operation is deployed yet."),
                Line::from("F6 deploys the current controller."),
            ],
        );
        return;
    };

    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);
        draw_pane(
            frame,
            left,
            "COMPROMISED SATELLITE FEED",
            satellite_lines(&op.current),
        );
        draw_pane(frame, right, "OPERATION TELEMETRY", telemetry_lines(&op));
    } else if state.narrow_secondary_visible() {
        draw_pane(frame, area, "OPERATION TELEMETRY", telemetry_lines(&op));
    } else {
        draw_pane(
            frame,
            area,
            "COMPROMISED SATELLITE FEED",
            satellite_lines(&op.current),
        );
    }
}

/// The reflective, run-concluded view: the same final satellite frame
/// Operation was last showing, alongside a concise mechanical outcome and
/// summary stats (`docs/TUI_DESIGN.md` §5, "After Action is an operation
/// state, not a disconnected popup"). Reuses `satellite_lines`/`draw_pane`
/// and the same two-pane/narrow-layout structure as `draw_operation`.
fn draw_after_action(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(op) = state.operation() else {
        draw_pane(
            frame,
            area,
            "AFTER-ACTION REPORT",
            vec![
                Line::from("No operation has concluded yet."),
                Line::from("F4 revises the controller, F6 deploys it."),
            ],
        );
        return;
    };

    if area.width >= TWO_PANE_MIN_COLUMNS {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);
        draw_pane(
            frame,
            left,
            "FINAL SATELLITE FRAME",
            satellite_lines(&op.current),
        );
        draw_after_action_report_pane(frame, right, &op);
    } else {
        // The report pane carries hierarchy items 1-4 (outcome, trigger,
        // meaning, completion) that the player must see before anything
        // else, for every finished operation — not just a synchronous load
        // failure with no discovered tiles (`docs/TUI_DESIGN.md` §5,
        // "Responsive behavior": "the report subview defaults to primary").
        // `narrow_secondary_visible` still flips which pane is showing from
        // there, same as every other narrow-layout view.
        let defaults_to_report = op.finished;
        if state.narrow_secondary_visible() ^ defaults_to_report {
            draw_after_action_report_pane(frame, area, &op);
        } else {
            draw_pane(
                frame,
                area,
                "FINAL SATELLITE FRAME",
                satellite_lines(&op.current),
            );
        }
    }
}

/// Draws the AFTER-ACTION REPORT pane, pinning the `F4` recovery hint to a
/// fixed last row on failure so a long diagnostic or deployed-source excerpt
/// filling [`MAX_DETAIL_LINES`] can never push it off the bottom of the
/// pane at the console's supported minimum geometry (`draw_pane_with_pinned_action`).
fn draw_after_action_report_pane(frame: &mut Frame, area: Rect, op: &OperationView<'_>) {
    let lines = after_action_report_lines(op);
    if after_action_succeeded(op) {
        draw_pane(frame, area, "AFTER-ACTION REPORT", lines);
    } else {
        draw_pane_with_pinned_action(
            frame,
            area,
            "AFTER-ACTION REPORT",
            lines,
            "F4  revise the controller",
        );
    }
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

/// Renders `text` capped to [`MAX_DETAIL_LINES`]/[`MAX_DETAIL_LINE_CHARS`],
/// with a trailing `…` marker whenever something was cut off, so the rest
/// of whatever pane called this always has room for its own content.
fn bounded_detail_lines(text: &str) -> Vec<Line<'static>> {
    let total_lines = text.lines().count();
    let mut lines: Vec<Line<'static>> = text
        .lines()
        .take(MAX_DETAIL_LINES)
        .map(|line| {
            let char_count = line.chars().count();
            if char_count > MAX_DETAIL_LINE_CHARS {
                let truncated: String = line.chars().take(MAX_DETAIL_LINE_CHARS).collect();
                Line::from(format!("{truncated}…"))
            } else {
                Line::from(line.to_string())
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

    match conclusion.kind {
        ConclusionKind::BudgetExhausted => {
            lines.push(Line::from(BUDGET_EXHAUSTED_MEANING));
        }
        ConclusionKind::ControllerError(error) => {
            lines.push(Line::from(CONTROLLER_EXECUTION_STOPPED));
            lines.extend(bounded_detail_lines(&controller_error_detail(error)));
        }
        ConclusionKind::Success => {
            unreachable!("after_action_failure_lines only runs on a non-success conclusion")
        }
    }
    // A blank line separates outcome/trigger/meaning from completion,
    // matching `docs/TUI_DESIGN.md` §5's failure mockup, without adding
    // enough height to push evidence/recovery out of the pane at the
    // console's minimum supported geometry — the `F4` recovery hint already
    // has its own dedicated pinned row (`draw_pane_with_pinned_action`), so
    // this pane's remaining budget is tighter here than on the success path.
    lines.push(Line::from(""));
    lines.push(Line::from(FIRST_CONTACT_INCOMPLETE));

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

    // Once the run is over, this pane is functioning as "Review Run"
    // (reachable via `F5` from After Action) rather than a live view — show
    // the immutable source and revision that produced this result, not
    // whatever the editor currently holds (`docs/TUI_DESIGN.md` §5,
    // "Review Run displays the immutable source revision and telemetry
    // associated with that recorded run").
    if op.finished {
        lines.push(Line::from(format!("deployed rev  run-{:02}", op.run_id)));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "DEPLOYED SOURCE",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(bounded_detail_lines(op.deployed_source));
        lines.push(Line::from(""));
    }

    if let Some(error) = op.error {
        lines.push(Line::from(Span::styled(
            controller_error_headline(error),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(bounded_detail_lines(&controller_error_detail(error)));
        lines.push(Line::from(""));
        lines.push(Line::from("F4  revise the controller"));
        lines.push(Line::from("F6  redeploy"));
        return lines;
    }

    if let Some(outcome) = op.records.last().map(|record| record.outcome)
        && outcome != TickOutcome::Running
    {
        lines.push(Line::from(Span::styled(
            outcome_headline(outcome),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from("F4  revise the controller"));
        lines.push(Line::from("F6  redeploy"));
        return lines;
    }

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
            Line::from("Ctrl+V      load the source and check for on_tick, without calling it"),
            Line::from("F8          (80-99 columns) switch between source and reference"),
        ],
        View::Operation => vec![
            Line::from("F6     deploy the current controller (confirms if a run is active)"),
            Line::from("Space  pause/resume the run"),
            Line::from("Enter  advance exactly one tick while paused"),
            Line::from("F8     (80-99 columns) switch between satellite feed and telemetry"),
            Line::from("Leaving via F2/F3/F4 pauses the run; F5 returns to it as you left it."),
        ],
        View::AfterAction => vec![
            Line::from("F2     back to Signals"),
            Line::from("F4     edit the controller (your edits are preserved)"),
            Line::from("F5     review this run's frozen source and telemetry (Review Run)"),
            Line::from("F6     redeploy from a clean scenario state"),
            Line::from("F8     (80-99 columns) switch between satellite frame and report"),
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
        items.push(("F8 Toggle Pane", "F8 Pane", true));
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

fn draw_footer(frame: &mut Frame, area: Rect, state: &AppState, full_width: u16) {
    let show_f8 = full_width < TWO_PANE_MIN_COLUMNS
        && matches!(
            state.current_view(),
            View::Signals | View::Target | View::Controller | View::Operation | View::AfterAction
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
    fn controller_status_stays_visible_from_signals_at_eighty_columns() {
        use super::super::state::Msg;
        use super::super::state::View;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::Navigate(View::Signals));
        let terminal = render(80, 30, &state);

        assert!(
            buffer_contains(&terminal, "CONTROLLER: modified"),
            "at the narrow 80-column width, a candidate that drops the \
             signals count instead of controller status should be offered \
             before one that drops controller status entirely"
        );
    }

    #[test]
    fn controller_status_stays_visible_from_target_at_eighty_columns() {
        use super::super::state::Msg;
        use super::super::state::View;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::Navigate(View::Target));
        let terminal = render(80, 30, &state);

        assert!(
            buffer_contains(&terminal, "CONTROLLER: modified"),
            "at the narrow 80-column width, a candidate that drops the \
             Target field instead of controller status should be offered \
             before one that drops controller status entirely"
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
    fn reset_confirmation_is_visible_even_when_the_narrow_reference_pane_was_toggled_on() {
        use super::super::state::Msg;

        let mut state = AppState::new();
        state.apply(Msg::Activate);
        state.apply(Msg::Activate);
        state.apply(Msg::EditController(super::super::editor::EditOp::Insert(
            'x',
        )));
        state.apply(Msg::ToggleSecondaryPane); // swap to the Lua reference pane
        state.apply(Msg::RequestResetController);
        let terminal = render(90, 30, &state);

        assert!(state.narrow_secondary_visible());
        assert!(state.reset_confirmation_pending());
        assert!(buffer_contains(
            &terminal,
            "Reset controller? Edits will be lost."
        ));
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
    fn display_width_of_prefix_counts_double_width_characters_as_two_cells() {
        // "中" (CJK) occupies two terminal columns despite being one `char`;
        // a scroll calculation based on `char` count alone would place the
        // cursor two columns short of its true screen position.
        assert_eq!(display_width_of_prefix("中文", 1), 2);
        assert_eq!(display_width_of_prefix("中文", 2), 4);
        assert_eq!(display_width_of_prefix("ab", 2), 2);
    }

    #[test]
    fn cursor_on_a_combining_mark_is_highlighted_together_with_its_base_character() {
        // "e" + U+0301 (combining acute accent) is a decomposed grapheme:
        // the mark alone is zero-width, so highlighting it by itself would
        // render no visible cursor cell at all.
        let text = "e\u{0301}bc";
        let cursor_span = |cursor_col: usize| {
            cursor_line_spans(text, cursor_col)
                .into_iter()
                .find(|span| span.style.add_modifier.contains(Modifier::REVERSED))
                .expect("one span should carry the cursor style")
        };

        // Cursor on the mark itself (col 1): the highlighted unit must
        // include the base character before it.
        assert_eq!(cursor_span(1).content, "e\u{0301}");
        // Cursor on the base character (col 0): the highlighted unit must
        // include the mark trailing it.
        assert_eq!(cursor_span(0).content, "e\u{0301}");
    }

    #[test]
    fn cursor_grapheme_range_spans_a_heart_and_its_variation_selector() {
        // U+2764 (heavy black heart) + U+FE0F (variation selector-16,
        // requesting emoji presentation) is a two-codepoint grapheme
        // cluster ratatui renders as one unit — measuring only the `char`
        // at the cursor (whichever of the two it lands on) misses the
        // other codepoint's contribution to the unit's true display width.
        let text = "a\u{2764}\u{fe0f}b";
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(cursor_grapheme_char_range(text, 1), (1, 3));
        assert_eq!(cursor_grapheme_char_range(text, 2), (1, 3));
        let glyph: String = chars[1..3].iter().collect();
        assert_eq!(
            glyph.as_str().cell_width(),
            2,
            "ratatui renders heart+VS16 as 2 cells, not 1"
        );
    }

    #[test]
    fn cursor_grapheme_range_spans_a_zero_width_joiner_sequence() {
        // "👩‍💻" is WOMAN (U+1F469) + ZWJ (U+200D) + COMPUTER (U+1F4BB) —
        // one extended grapheme cluster, but neither the woman nor the
        // computer emoji is itself zero-width (only the joiner between
        // them is), so a heuristic that only merges *zero-width* neighbors
        // stops right after the joiner and never reaches the trailing
        // emoji.
        let text = "a\u{1F469}\u{200D}\u{1F4BB}b";
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars.len(), 5, "a, woman, zwj, computer, b");
        assert_eq!(cursor_grapheme_char_range(text, 1), (1, 4));
        assert_eq!(cursor_grapheme_char_range(text, 2), (1, 4));
        assert_eq!(cursor_grapheme_char_range(text, 3), (1, 4));
    }

    #[test]
    fn cursor_grapheme_range_spans_a_regional_indicator_flag_pair() {
        // "🇺🇸" is REGIONAL INDICATOR SYMBOL LETTER U (U+1F1FA) + ... S
        // (U+1F1F8) — two individually non-zero-width characters that form
        // one flag grapheme together; nothing about either one alone marks
        // them as needing to be grouped.
        let text = "a\u{1F1FA}\u{1F1F8}b";
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars.len(), 4, "a, U indicator, S indicator, b");
        assert_eq!(cursor_grapheme_char_range(text, 1), (1, 3));
        assert_eq!(cursor_grapheme_char_range(text, 2), (1, 3));
    }

    #[test]
    fn chars_to_skip_for_cell_offset_accounts_for_double_width_characters() {
        // Skipping 2 display cells must skip exactly one CJK character, not
        // zero (which char-count-only scrolling would do, since 2 chars
        // would be requested but only 1 exists at that cell offset).
        assert_eq!(chars_to_skip_for_cell_offset("中文abc", 2), 1);
        assert_eq!(chars_to_skip_for_cell_offset("中文abc", 4), 2);
        assert_eq!(chars_to_skip_for_cell_offset("abc", 2), 2);
    }

    #[test]
    fn chars_to_skip_for_cell_offset_treats_zero_width_marks_as_zero_cells() {
        // Previously this function clamped every character's contribution
        // to at least one cell, so three zero-width combining marks were
        // (wrongly) treated as consuming three whole display columns —
        // disagreeing with `display_width_of_prefix`'s accounting of the
        // very same text and potentially leaving the cursor's true screen
        // column outside the scrolled viewport.
        let text = "\u{0301}\u{0301}\u{0301}X"; // three combining marks, then "X"
        assert_eq!(
            chars_to_skip_for_cell_offset(text, 0),
            0,
            "nothing needs skipping to reach the very first cell"
        );
        assert_eq!(
            chars_to_skip_for_cell_offset(text, 1),
            4,
            "reaching cell 1 (where X starts) must skip past all three \
             zero-width marks and X itself, not stop after the first mark"
        );
    }

    #[test]
    fn chars_to_skip_for_cell_offset_never_stops_mid_grapheme_cluster() {
        // "a👩‍💻b": a (1 cell), then the woman-technologist ZWJ sequence
        // (2 cells — see `cursor_grapheme_range_spans_a_zero_width_joiner_
        // sequence`), then b (1 cell). Requesting a cell offset that lands
        // *inside* that cluster (offset 2, one cell past "a") must still
        // skip the whole cluster, not stop partway through it and leave a
        // scrolled line starting on the ZWJ or the trailing emoji alone.
        let text = "a\u{1F469}\u{200D}\u{1F4BB}b";
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars.len(), 5, "a, woman, zwj, computer, b");
        let skip = chars_to_skip_for_cell_offset(text, 2);
        assert!(
            skip == 1 || skip == 4,
            "must land on a grapheme boundary (before or after the \
             cluster), got skip={skip}"
        );
    }

    #[test]
    fn display_width_matches_ratatui_for_a_halfwidth_katakana_dakuten() {
        // `unicode-width` alone reports U+FF9E (a halfwidth katakana
        // voiced-sound mark) as zero-width, but ratatui's own `CellWidth`
        // adds a terminal-compatibility +1 for it (real terminals render it
        // as its own occupied cell) — a naive per-character sum disagrees
        // with what actually gets rendered, exactly the class of mismatch
        // that could clip the cursor off the edge of a scrolled line.
        let text = "\u{FF76}\u{FF9E}"; // halfwidth カ + dakuten
        assert_eq!(
            display_width_of_prefix(text, 2),
            2,
            "ratatui renders the halfwidth katakana + dakuten pair as 2 cells"
        );
        assert_eq!(
            chars_to_skip_for_cell_offset(text, 2),
            2,
            "reaching cell 2 must skip past both characters, not treat the \
             dakuten as contributing zero cells"
        );
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
    fn a_synchronous_deploy_failure_defaults_to_the_report_pane_at_narrow_widths() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));
        state.apply(Msg::RequestDeploy);
        assert_eq!(state.current_view(), View::AfterAction);

        // Below the two-pane threshold, with no `F8` toggle pressed yet, a
        // deploy that never started a live run has nothing to show in the
        // satellite pane — the compact failure report must be what's
        // visible by default, not an empty grid the player has to know to
        // toggle away from.
        let terminal = render(90, 30, &state);
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: controller script error"
        ));
        assert!(!buffer_contains(&terminal, "FINAL SATELLITE FRAME"));
    }

    #[test]
    fn the_recovery_hint_stays_visible_at_the_two_pane_minimum_geometry() {
        use super::super::editor::EditOp;
        use super::super::state::Msg;

        let mut state = working_state();
        for _ in 0..500 {
            state.apply(Msg::EditController(EditOp::Backspace));
        }
        state.apply(Msg::EditController(EditOp::Insert('(')));
        state.apply(Msg::RequestDeploy);
        assert_eq!(state.current_view(), View::AfterAction);

        // The two-pane report pane is only 40% wide at the narrowest
        // two-pane geometry the console supports (`TWO_PANE_MIN_COLUMNS` x
        // `MIN_ROWS`) — narrow enough that both the headline and the
        // diagnostic detail wrap across several lines. Before the recovery
        // hint was pinned to a fixed last row (`draw_pane_with_pinned_action`
        // via `draw_after_action_report_pane`), that wrapped content could
        // push it below the pane's visible rows.
        let terminal = render(TWO_PANE_MIN_COLUMNS, MIN_ROWS, &state);
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
        // incompletion must be visible without pressing `F8` to swap panes
        // (`docs/TUI_DESIGN.md` §5, "Responsive behavior").
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: budget exhausted"
        ));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
    }

    #[test]
    fn the_full_failure_closure_fits_at_the_two_pane_minimum_geometry() {
        let state = budget_exhausted_state();

        // At `TWO_PANE_MIN_COLUMNS` x `MIN_ROWS` — a 40%-wide, unscrollable
        // report pane — the outcome hierarchy's higher-priority items
        // (outcome, completion) and the separately-pinned `F4` recovery hint
        // must survive even if lower-priority evidence/closing-paragraph
        // body text is clipped from the bottom of the unscrollable
        // `Paragraph` (`docs/TUI_DESIGN.md` §5: "Higher items must never be
        // sacrificed for lower ones when space is constrained"). At a wider
        // two-pane width (120), there's room for the full closure —
        // evidence and the closing paragraph too — same as success.
        let terminal = render(TWO_PANE_MIN_COLUMNS, MIN_ROWS, &state);
        assert!(buffer_contains(
            &terminal,
            "OPERATION FAILED: budget exhausted"
        ));
        assert!(buffer_contains(&terminal, "FIRST CONTACT INCOMPLETE"));
        assert!(buffer_contains(&terminal, "F4  revise the controller"));

        let terminal = render(120, MIN_ROWS, &state);
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
    fn the_full_success_closure_fits_at_the_two_pane_minimum_geometry() {
        let state = succeeded_state();

        // `TWO_PANE_MIN_COLUMNS` x `MIN_ROWS` is the narrowest geometry that
        // still takes the two-pane layout — the report pane's 40%-width,
        // unscrollable inner area is the tightest fit the full success
        // report has to survive. A wider two-pane width (120) leaves the
        // same inner *height* but a wider pane, so it's checked too.
        for width in [TWO_PANE_MIN_COLUMNS, 120] {
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
            // The full closing paragraph, including its last word, must
            // survive — `Paragraph` doesn't scroll, so content taller than
            // the pane's inner height is silently clipped from the bottom.
            assert!(
                buffer_contains(&terminal, "network."),
                "closing paragraph clipped at {width}x{MIN_ROWS}"
            );
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
    }

    #[test]
    fn narrow_operation_view_shows_one_pane_until_f8_toggles_it() {
        use super::super::state::Msg;

        let mut state = working_state();
        state.apply(Msg::RequestDeploy);
        let terminal = render(90, 30, &state);
        assert!(buffer_contains(&terminal, "COMPROMISED SATELLITE FEED"));
        assert!(!buffer_contains(&terminal, "OPERATION TELEMETRY"));

        state.apply(Msg::ToggleSecondaryPane);
        let terminal = render(90, 30, &state);
        assert!(buffer_contains(&terminal, "OPERATION TELEMETRY"));
        assert!(!buffer_contains(&terminal, "COMPROMISED SATELLITE FEED"));
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
}
