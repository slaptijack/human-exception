//! The persistent, full-screen resistance-console session.
//!
//! This module hosts the interactive shell described by
//! `docs/TUI_DESIGN.md`: a session/state model (`state`), key-to-intent
//! mapping (`event`), and rendering (`ui`). Signals, Target, Controller,
//! Operation, and After Action are all populated, completing the
//! edit-deploy-observe-retry loop.
//!
//! [`run`] is the only supported public API: these submodules are
//! implementation details of the console session, not a supported
//! embedding surface.

pub(crate) mod document;
pub(crate) mod editor;
pub(crate) mod event;
pub(crate) mod intel;
pub(crate) mod state;
pub(crate) mod ui;

use std::io;
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{
    self as term_event, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use state::{AppState, Msg, PaneId};

type PanicHook = dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static;

/// Whether [`event_loop`] successfully pushed the Kitty keyboard-protocol
/// flags, so [`restore_terminal`] (called from several places: normal
/// exit, an early `Terminal::new` failure, and the panic hook) only pops
/// them when there's actually a matching push to undo. Popping
/// unconditionally would remove one level from the terminal's own
/// keyboard-protocol stack even when nothing was ever pushed — e.g. the
/// query timed out, the push itself failed, or startup never reached that
/// point at all — which can disturb a mode a parent terminal or
/// multiplexer had already established.
static KEYBOARD_ENHANCEMENT_PUSHED: AtomicBool = AtomicBool::new(false);

/// Enters the interactive resistance-console session against a real
/// terminal, and restores the terminal on every exit path, including a
/// panic.
pub fn run() -> io::Result<()> {
    enable_raw_mode()?;
    if let Err(err) = execute!(io::stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(err);
    }
    if let Err(err) = execute!(io::stdout(), EnableBracketedPaste) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        return Err(err);
    }

    let previous_hook = install_panic_hook();

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => {
            restore_panic_hook(previous_hook);
            let _ = restore_terminal();
            return Err(err);
        }
    };

    let result = event_loop(&mut terminal);

    restore_panic_hook(previous_hook);
    restore_terminal()?;

    result
}

/// Installs a panic hook that restores the terminal before delegating to
/// whatever hook the host application had configured, and returns that
/// original hook so it can be reinstated once the session ends normally.
fn install_panic_hook() -> Arc<PanicHook> {
    let previous: Arc<PanicHook> = Arc::from(panic::take_hook());
    let for_hook = Arc::clone(&previous);
    panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        for_hook(info);
    }));
    previous
}

fn restore_panic_hook(previous: Arc<PanicHook>) {
    panic::set_hook(Box::new(move |info| previous(info)));
}

/// Restores every terminal mode this session enables, attempting each step
/// even if an earlier one fails — a terminal mode like bracketed paste is
/// owned by the terminal emulator, not this process, so it must not be
/// left enabled just because e.g. `disable_raw_mode` errored first. Any
/// error from `disable_raw_mode` or leaving the alternate screen is still
/// surfaced, favoring the first one.
fn restore_terminal() -> io::Result<()> {
    let raw_mode_result = disable_raw_mode();
    if KEYBOARD_ENHANCEMENT_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    let leave_screen_result = execute!(io::stdout(), LeaveAlternateScreen, Show);
    raw_mode_result?;
    leave_screen_result?;
    Ok(())
}

/// Drives the session against a real terminal, reading real `crossterm`
/// events. The pure dispatch/redraw logic lives in [`should_redraw`] and
/// [`ui::draw`] so it can also be driven against `TestBackend` in tests.
fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut state = AppState::new();
    let mut frame_size = (0u16, 0u16);
    let mut last_transition_press: TransitionKeyDebounce = None;

    terminal.draw(|frame| {
        let area = frame.area();
        frame_size = (area.width, area.height);
        ui::draw(frame, &state);
    })?;

    // Deliberately probed only after the first draw, not before: querying
    // terminal support blocks on a reply crossterm waits up to ~2s for, and
    // on a terminal that never answers, probing first would leave the
    // player staring at a blank, unresponsive screen for that whole window
    // instead of the console they just launched. Best-effort either way —
    // without it, most terminals report Ctrl+Enter identically to plain
    // Enter, so Controller's Ctrl+Enter "validate" shortcut would silently
    // insert a newline instead; `Ctrl+V` (an ordinary control character
    // every terminal sends correctly) is the guaranteed fallback binding
    // for validation regardless, so this is just a nicer default, not the
    // only path to it.
    if supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    // Required (on Unix; Windows always reports it) for
                    // `KeyEvent.kind` to ever be `Repeat`/`Release` at all —
                    // without it every autorepeated or released key still
                    // arrives as plain `Press`, silently defeating the
                    // held-key cascade guard in `should_redraw` below.
                    //
                    // Deliberately NOT also requesting
                    // `REPORT_ALL_KEYS_AS_ESCAPE_CODES`: crossterm's own docs
                    // say it's needed for plain-text keys (`Enter`, `y`/`n`)
                    // to report `Repeat`/`Release` too, but it also forces
                    // *every* key — including ordinary shifted symbol keys
                    // like `_` and `(`/`)` — through the more complex CSI-u
                    // encoding, and at least one real terminal/keyboard-
                    // layout combination stopped reporting those characters
                    // correctly once it was enabled (confirmed by testing:
                    // Shift-derived characters silently failed to type in
                    // Controller's editor). A working editor is far more
                    // important than `Enter`/`y`/`n` specifically joining
                    // the held-key cascade guard already applied to
                    // everything else Kitty can report a `Repeat` kind for.
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )
        .is_ok()
    {
        KEYBOARD_ENHANCEMENT_PUSHED.store(true, Ordering::SeqCst);
    }

    while !state.should_quit() {
        // Presentation paces a running, unpaused deployment (`Space`/`Enter`
        // control it, not wall-clock timing — see `docs/TUI_DESIGN.md`,
        // "Pacing controls"): while `operation_auto_advancing` holds, poll
        // with a short timeout instead of blocking on `read` forever, so a
        // tick can advance on its own between key presses. Otherwise this
        // is the exact same blocking wait as before — an idle session never
        // wakes up on a timer it doesn't need.
        let redraw = if state.operation_auto_advancing() {
            if term_event::poll(OPERATION_TICK_INTERVAL)? {
                let event = term_event::read()?;
                should_redraw(&mut state, event, frame_size, &mut last_transition_press)
            } else {
                state.advance_running_operation()
            }
        } else {
            let event = term_event::read()?;
            should_redraw(&mut state, event, frame_size, &mut last_transition_press)
        };

        if redraw {
            terminal.draw(|frame| {
                let area = frame.area();
                frame_size = (area.width, area.height);
                ui::draw(frame, &state);
            })?;
        }
    }

    Ok(())
}

/// How often a running, unpaused deployment advances a tick on its own
/// while the player isn't pressing anything (`docs/TUI_DESIGN.md`'s
/// "Pacing controls": `Space` pause/resume, `Enter` single-step). Purely a
/// presentation cadence — [`crate::lua_controller::LiveOperation::step`]'s
/// result never depends on when it's called, so this has no effect on
/// simulation outcomes, only on how quickly the player watches them unfold.
const OPERATION_TICK_INTERVAL: Duration = Duration::from_millis(350);

/// The last (code, modifiers, timestamp) of an accepted-or-debounced press
/// of one of [`is_repeat_untrustworthy`]'s keys — see
/// [`TRANSITION_KEY_DEBOUNCE`]. Threaded through every [`should_redraw`]
/// call site (the real event loop and each test that drives it) rather
/// than stored on [`AppState`], since it's a terminal-input-timing detail,
/// not session state.
type TransitionKeyDebounce = Option<(KeyCode, KeyModifiers, Instant)>;

/// How long after a `Repeat`-untrustworthy transition key is accepted
/// before another press of the exact *same* key (same code and modifiers)
/// is treated as a suspected terminal auto-repeat and dropped, rather than
/// a second deliberate press. Chosen comfortably above a terminal's
/// typical auto-repeat interval (often tens of milliseconds) but well
/// under any gap a player would leave between two genuinely separate
/// presses of the same key.
const TRANSITION_KEY_DEBOUNCE: Duration = Duration::from_millis(150);

/// Whether `code`'s `Repeat`/`Release` `KeyEventKind` can't be trusted to
/// distinguish a held key from a fresh press, and so needs
/// [`TRANSITION_KEY_DEBOUNCE`] instead. Per crossterm's own documentation,
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES` is required for a *plain-text* key
/// (no modifier, not a function key — those are already sent as escape
/// sequences either way) to report anything but `Press`; `event_loop`
/// deliberately doesn't request that flag (see its doc comment — it broke
/// typing shifted symbol keys like `_`/`(`/`)` on a real terminal), so
/// plain `y`/`n` can no longer rely on `key.kind` at all.
///
/// `Enter` and `Esc` are deliberately *not* included here despite having
/// the same untrustworthy-`kind` problem: unlike `y`/`n` (single-shot
/// dialog responses nothing legitimately needs to press twice in quick
/// succession), a fast player pressing `Enter` twice in a row on purpose —
/// e.g. drilling straight from Signals through Target into Controller — is
/// a normal, encouraged interaction this console's own flow is built
/// around, and a debounce can't tell that apart from a held key's
/// auto-repeat. Between "occasionally lets a genuinely held `Enter`/`Esc`
/// cascade through an extra transition" and "makes rapid deliberate
/// double-`Enter` navigation randomly drop the second press," the former
/// is the smaller cost — and is also the pre-existing baseline on any
/// terminal that never reported a trustworthy `Repeat` kind for these keys
/// in the first place, not a new regression.
fn is_repeat_untrustworthy(code: KeyCode) -> bool {
    matches!(code, KeyCode::Char('y') | KeyCode::Char('n'))
}

/// Applies a single terminal event to `state`, returning whether the frame
/// needs to be redrawn as a result. `frame_size` is the frame's size as of
/// the most recent draw — slightly stale for the resize event that changes
/// it, but accurate again by the time the next key event arrives — and is
/// used both to bound the current view's scroll offset against the viewport
/// it will actually render into, and to tell whether only the geometry
/// warning is showing.
///
/// A resize always needs a redraw, since the geometry warning (or the shell
/// it replaces) depends on the frame size, not on any key event. Pane focus
/// itself is untouched by a resize — narrow-layout visibility is derived
/// from `AppState::focused_pane`, so the already-focused pane is simply
/// what narrow mode shows, with no separate state to reset here (`docs/
/// TUI_DESIGN.md`, "Focus persistence").
fn should_redraw(
    state: &mut AppState,
    event: Event,
    frame_size: (u16, u16),
    last_transition_press: &mut TransitionKeyDebounce,
) -> bool {
    if matches!(event, Event::Resize(_, _)) {
        return true;
    }

    // Below the supported minimum, only the geometry warning and `Ctrl+Q`
    // are shown; every other intent must stay inert instead of silently
    // mutating state (e.g. committing an opportunity) the player can't see.
    let undersized = frame_size.0 < ui::MIN_COLUMNS || frame_size.1 < ui::MIN_ROWS;

    if let Event::Paste(text) = event {
        let dialog_pending = state.reset_confirmation_pending()
            || state.quit_confirmation_pending()
            || state.redeploy_confirmation_pending();
        let accepted = !undersized
            && !dialog_pending
            && state.current_view() == state::View::Controller
            && state.focused_pane(state::View::Controller) == PaneId::ControllerSource;
        if !accepted || text.is_empty() {
            return false;
        }
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        state.apply(Msg::PasteController(normalized));
        return true;
    }

    let Event::Key(key) = event else {
        return false;
    };
    // `Repeat` (a terminal reporting a key still held down) is treated the
    // same as `Press` for keys whose meaning can't change out from under a
    // held key: typing, deleting, and moving the cursor or a selection.
    // Without that, holding an arrow key, Backspace, or a printable
    // character in Controller only ever applies once, making ordinary
    // editing through more than a token of source impractical.
    //
    // `Enter`, `Esc`, the function keys, and `Ctrl+V` are excluded from
    // `Repeat` entirely, Press-only, because they trigger state
    // *transitions* (Activate, navigate, dismiss Help, or — for `Ctrl+V` —
    // starting another `validate` worker, see `lua_controller::validate`'s
    // `MAX_CONCURRENT_VALIDATIONS` cap) rather than a repeatable edit: a
    // terminal's repeat events fire on a timer independent of how fast the
    // app already processed the initial press, so a held key can otherwise
    // cascade through several unrelated meanings in a row, or exhaust
    // every validation slot from one keypress. This only helps on a
    // terminal that actually reports `Repeat` for these — `event_loop`
    // deliberately doesn't request `REPORT_ALL_KEYS_AS_ESCAPE_CODES` (see
    // its doc comment: it broke typing shifted symbols like `_`/`(`/`)` on
    // a real terminal), which crossterm's own docs say is required for a
    // plain-text key like `Enter`/`Esc` to report anything but `Press` —
    // but it's free protection when the terminal does provide it (e.g.
    // Windows, which crossterm says always reports `kind`), so it stays.
    let is_ctrl_v = key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL);
    let kind_allowed = match key.kind {
        KeyEventKind::Press => true,
        KeyEventKind::Repeat => {
            !is_ctrl_v && !matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::F(_))
        }
        KeyEventKind::Release => false,
    };
    if !kind_allowed {
        return false;
    }

    // Plain `y`/`n` get a second, stronger layer on top of the `kind`
    // check above: a time-based debounce (see `is_repeat_untrustworthy`
    // for why `key.kind` alone isn't enough for them specifically). A
    // second press of the exact same key within `TRANSITION_KEY_DEBOUNCE`
    // of one that *actually closed a dialog* (accepted *or* itself
    // debounced — the window keeps sliding forward for as long as presses
    // keep arriving faster than that, modeling "still held") is treated as
    // a suspected leaked auto-repeat and dropped — closing the risk of a
    // held `y`/`n` confirming or cancelling a dialog and then typing
    // straight into the controller it just reset or the quit it just
    // cancelled.
    //
    // Deliberately scoped to only *record* a press when a dialog is
    // actually open (i.e. this exact press is what's about to confirm or
    // cancel it), not every `y`/`n` press unconditionally: `y` and `n` are
    // also ordinary printable characters, and debouncing them globally
    // would drop the second character of a legitimate `nn`/`yy` typed
    // quickly into Controller's source (e.g. mid-identifier), the same way
    // any other repeated printable character must keep working. A `y`/`n`
    // press while no dialog is open never updates or consults this record,
    // so ordinary editing is completely unaffected; only a leaked repeat
    // arriving soon after an actual dialog-closing press is caught.
    //
    // `Enter`/`Esc` don't get this same treatment at all: unlike `y`/`n`,
    // a fast player pressing `Enter` twice on purpose (e.g. drilling
    // straight from Signals through Target into Controller) is normal,
    // expected use a debounce can't tell apart from a held key's
    // auto-repeat, so for those two the `kind`-based check above —
    // best-effort, inert on a terminal that doesn't report it — is the
    // only protection.
    if is_repeat_untrustworthy(key.code) {
        let dialog_pending = state.reset_confirmation_pending()
            || state.quit_confirmation_pending()
            || state.redeploy_confirmation_pending();
        let now = Instant::now();
        let debounced = matches!(
            *last_transition_press,
            Some((last_code, last_mods, last_time))
                if last_code == key.code
                    && last_mods == key.modifiers
                    && now.duration_since(last_time) < TRANSITION_KEY_DEBOUNCE
        );
        if dialog_pending {
            *last_transition_press = Some((key.code, key.modifiers, now));
        } else if debounced {
            return false;
        } else {
            // Ordinary typing unrelated to any dialog: clear any stale
            // record so it can't debounce a much-later, purely
            // coincidental same-key press.
            *last_transition_press = None;
        }
    }

    match event::map(
        key,
        state.current_view(),
        state.reset_confirmation_pending(),
        state.quit_confirmation_pending(),
        state.redeploy_confirmation_pending(),
        state.focused_pane(state.current_view()),
        state.focus_movement_available(state.current_view()),
    ) {
        Some(msg) => {
            let is_quit_related =
                matches!(msg, Msg::RequestQuit | Msg::ConfirmQuit | Msg::CancelQuit);
            if undersized && !is_quit_related {
                return false;
            }
            let is_pane_scroll = matches!(msg, Msg::ScrollUp | Msg::ScrollDown);
            state.apply(msg);
            if is_pane_scroll {
                let pane = state.focused_pane(state.current_view());
                if let Some(max) = pane_max_scroll(pane, state, frame_size.0, frame_size.1) {
                    state.clamp_scroll(pane, max);
                }
            }
            true
        }
        None => false,
    }
}

/// The content- and frame-size-aware maximum scroll offset for `pane`, or
/// `None` if `pane` isn't scrollable at all. The single source of truth for
/// which panes are scrollable at render time, kept in sync with
/// `state::pane_is_scrollable`, which makes the same decision for dispatch
/// (`Msg::ScrollUp`/`Msg::ScrollDown` themselves must never accumulate an
/// offset for a pane this function would return `None` for).
fn pane_max_scroll(
    pane: PaneId,
    state: &AppState,
    frame_width: u16,
    frame_height: u16,
) -> Option<u16> {
    match pane {
        PaneId::Help => Some(ui::help_max_scroll(state, frame_width, frame_height)),
        PaneId::Report => Some(ui::after_action_max_scroll(
            state,
            frame_width,
            frame_height,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use state::View;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press_ctrl(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
    }

    fn repeat(code: KeyCode) -> Event {
        let mut key = KeyEvent::new(code, KeyModifiers::NONE);
        key.kind = KeyEventKind::Repeat;
        Event::Key(key)
    }

    fn repeat_ctrl(code: KeyCode) -> Event {
        let mut key = KeyEvent::new(code, KeyModifiers::CONTROL);
        key.kind = KeyEventKind::Repeat;
        Event::Key(key)
    }

    fn release(code: KeyCode) -> Event {
        let mut key = KeyEvent::new(code, KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        Event::Key(key)
    }

    fn paste(text: &str) -> Event {
        Event::Paste(text.to_string())
    }

    fn render(width: u16, height: u16, events: &[Event]) -> (AppState, Terminal<TestBackend>) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut state = AppState::new();
        let mut last_transition_press: TransitionKeyDebounce = None;

        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");

        for event in events {
            if should_redraw(
                &mut state,
                event.clone(),
                (width, height),
                &mut last_transition_press,
            ) {
                terminal
                    .draw(|frame| ui::draw(frame, &state))
                    .expect("redraw should succeed");
            }
        }

        (state, terminal)
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
    fn signals_is_the_default_view() {
        let (_, terminal) = render(120, 40, &[]);
        assert!(buffer_contains(&terminal, "SIGNALS"));
    }

    #[test]
    fn signals_selection_and_activation_are_inert_while_the_detail_pane_is_focused() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::F(8)), // move focus to the selected-signal detail pane
                press(KeyCode::Down),
                press(KeyCode::Up),
                press(KeyCode::Enter),
            ],
        );

        assert_eq!(state.current_view(), View::Signals);
        assert_eq!(state.selected_signal(), 0);
    }

    #[test]
    fn navigating_and_opening_help_then_dismissing_returns_to_target() {
        let (state, terminal) = render(
            120,
            40,
            &[
                press(KeyCode::Enter), // inspect the actionable signal, opening Target
                press(KeyCode::F(1)),
                press(KeyCode::Esc),
            ],
        );

        assert_eq!(state.current_view(), View::Target);
        assert!(buffer_contains(&terminal, "TARGET"));
    }

    #[test]
    fn help_overlay_renders_when_opened() {
        let (_, terminal) = render(120, 40, &[press(KeyCode::F(1))]);
        assert!(buffer_contains(&terminal, "HELP"));
        assert!(buffer_contains(&terminal, "Global controls"));
    }

    #[test]
    fn ctrl_q_sets_should_quit() {
        let (state, _) = render(120, 40, &[press_ctrl(KeyCode::Char('q'))]);
        assert!(state.should_quit());
    }

    #[test]
    fn full_event_loop_exits_after_ctrl_q() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut events = vec![
            press(KeyCode::F(3)),
            press(KeyCode::F(4)),
            press_ctrl(KeyCode::Char('q')),
        ]
        .into_iter();

        let mut state = AppState::new();
        let mut last_transition_press: TransitionKeyDebounce = None;
        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");
        while !state.should_quit() {
            let event = events
                .next()
                .expect("script should quit before running out");
            should_redraw(&mut state, event, (120, 40), &mut last_transition_press);
        }

        assert!(state.should_quit());
    }

    #[test]
    fn undersized_terminal_shows_the_geometry_warning_instead_of_the_shell() {
        let (_, terminal) = render(60, 20, &[]);
        assert!(buffer_contains(&terminal, "Terminal link degraded."));
        assert!(buffer_contains(
            &terminal,
            "Minimum console geometry: 80x24"
        ));
        assert!(!buffer_contains(&terminal, "SIGNALS"));
    }

    #[test]
    fn local_intents_are_ignored_while_the_geometry_warning_is_showing() {
        let (state, _) = render(
            60,
            20,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(2)),
            ],
        );

        assert_eq!(state.current_view(), View::Signals);
        assert_eq!(state.working_set(), None);
        assert!(state.controller_source().is_none());
    }

    #[test]
    fn ctrl_q_still_quits_while_the_geometry_warning_is_showing() {
        let (state, _) = render(60, 20, &[press_ctrl(KeyCode::Char('q'))]);

        assert!(state.should_quit());
    }

    #[test]
    fn f3_is_inert_before_a_signal_has_been_inspected() {
        let (state, terminal) = render(120, 40, &[press(KeyCode::F(3))]);

        assert_eq!(state.current_view(), View::Signals);
        assert!(buffer_contains(&terminal, "SIGNALS"));
    }

    #[test]
    fn inspecting_and_working_the_opportunity_reaches_controller_with_a_working_set() {
        let (state, terminal) = render(120, 40, &[press(KeyCode::Enter), press(KeyCode::Enter)]);

        assert_eq!(state.current_view(), View::Controller);
        assert_eq!(state.working_set(), Some(state::WorkingSet::FirstContact));
        assert!(state.controller_source().is_some());
        assert!(buffer_contains(&terminal, "FIRST CONTACT"));
    }

    #[test]
    fn f4_becomes_available_once_a_working_set_exists() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(2)),
                press(KeyCode::F(4)),
            ],
        );

        assert_eq!(state.current_view(), View::Controller);
    }

    #[test]
    fn a_resize_event_redraws_without_a_key_press() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut state = AppState::new();
        let mut last_transition_press: TransitionKeyDebounce = None;
        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");
        assert!(buffer_contains(&terminal, "Terminal link degraded."));

        terminal.backend_mut().resize(120, 40);
        if should_redraw(
            &mut state,
            Event::Resize(120, 40),
            (120, 40),
            &mut last_transition_press,
        ) {
            terminal
                .draw(|frame| ui::draw(frame, &state))
                .expect("redraw should succeed");
        }

        assert!(buffer_contains(&terminal, "SIGNALS"));
    }

    #[test]
    fn a_held_key_repeat_event_edits_the_controller_same_as_a_press() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter), // inspect the actionable signal, opening Target
                press(KeyCode::Enter), // commit, opening Controller
                repeat(KeyCode::Char('x')),
                repeat(KeyCode::Char('x')),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(format!("{}xx", intel::STARTER_CONTROLLER)),
            "held-key Repeat events should insert just like Press, not be ignored"
        );
    }

    #[test]
    fn pasting_multiline_text_inserts_it_verbatim_into_the_controller() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                paste("function on_tick()\n  return 1\nend\n"),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(format!(
                "{}function on_tick()\n  return 1\nend\n",
                intel::STARTER_CONTROLLER
            ))
        );
    }

    #[test]
    fn pasted_crlf_and_cr_line_endings_are_normalized_to_lf() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                paste("a\r\nb\rc\n"),
            ],
        );

        let source = state.controller_source().expect("controller loaded");
        assert!(source.ends_with("a\nb\nc\n"));
        assert!(!source.contains('\r'));
    }

    #[test]
    fn paste_cursor_lands_immediately_after_inserted_text() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                paste("abc"),
                press(KeyCode::Char('Z')),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(format!("{}abcZ", intel::STARTER_CONTROLLER))
        );
    }

    #[test]
    fn empty_paste_does_not_modify_controller_source() {
        let (state, _) = render(
            120,
            40,
            &[press(KeyCode::Enter), press(KeyCode::Enter), paste("")],
        );

        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn paste_outside_controller_view_is_ignored() {
        let (state, _) = render(120, 40, &[paste("stolen text")]);

        assert_eq!(state.current_view(), View::Signals);
        assert!(state.controller_source().is_none());
    }

    #[test]
    fn paste_while_a_confirmation_dialog_is_open_is_ignored() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::Char('x')),
                press(KeyCode::F(7)), // open the reset-controller confirmation
                paste("sneaked in"),
            ],
        );

        assert!(state.reset_confirmation_pending());
        assert_eq!(
            state.controller_source(),
            Some(format!("{}x", intel::STARTER_CONTROLLER))
        );
    }

    #[test]
    fn paste_while_reference_pane_visible_in_narrow_layout_is_ignored() {
        let (state, _) = render(
            90,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(8)), // swap the narrow secondary pane to the Lua reference
                paste("sneaked in"),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn paste_while_reference_pane_focused_in_wide_layout_is_ignored() {
        // At 100+ columns both panes are visible, so screen visibility alone
        // can't tell paste apart from real focus — this is the wide-width
        // counterpart to `paste_while_reference_pane_visible_in_narrow_layout_is_ignored`
        // above, proving routing follows focus rather than layout.
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(8)), // move focus to the Lua reference pane
                paste("sneaked in"),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn typed_characters_while_reference_pane_focused_in_wide_layout_are_ignored() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(8)), // move focus to the Lua reference pane
                press(KeyCode::Char('x')),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn paste_below_minimum_geometry_is_ignored() {
        let (state, _) = render(60, 20, &[paste("sneaked in")]);

        assert!(state.controller_source().is_none());
    }

    #[test]
    fn pasting_over_a_selection_replaces_it() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press_ctrl(KeyCode::Char('a')), // select the entire starter source
                paste("function on_tick()\nend\n"),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some("function on_tick()\nend\n".to_string()),
            "paste should replace the selected starter source, not insert alongside it"
        );
    }

    #[test]
    fn undo_after_paste_reverses_the_whole_paste_and_redo_reapplies_it() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                paste("multi\nline\ntext\n"),
                press_ctrl(KeyCode::Char('z')), // undo
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string()),
            "one undo should reverse the entire paste in a single step, \
             not partially"
        );

        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                paste("multi\nline\ntext\n"),
                press_ctrl(KeyCode::Char('z')), // undo
                press_ctrl(KeyCode::Char('y')), // redo
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(format!("{}multi\nline\ntext\n", intel::STARTER_CONTROLLER))
        );
    }

    #[test]
    fn a_held_enter_key_does_not_cascade_through_activate_and_newline() {
        // Reproduces the exact scenario a terminal-generated Repeat stream
        // can produce: a Press that opens Target, then Repeat events that
        // arrive after the app already moved on, which must not go on to
        // Activate again (committing and opening Controller) or insert a
        // newline into the now-visible source.
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),  // inspect the signal, opening Target
                repeat(KeyCode::Enter), // must NOT Activate again
                repeat(KeyCode::Enter), // must NOT insert a newline either
            ],
        );

        assert_eq!(state.current_view(), View::Target);
        assert_eq!(state.working_set(), None);
    }

    #[test]
    fn a_held_ctrl_v_does_not_repeatedly_start_new_validations() {
        // Each `Ctrl+V` triggers a background `validate` worker (see
        // `lua_controller::validate`'s `MAX_CONCURRENT_VALIDATIONS` cap); a
        // terminal reporting a held `Ctrl+V` as a stream of `Repeat` events
        // must not start one worker per repeat, the same held-key cascade
        // risk the Enter/Esc/F-key/y/n guard exists for above.
        let (_, terminal) = render(
            120,
            40,
            &[
                press(KeyCode::Enter), // open Target
                press(KeyCode::Enter), // commit, opening Controller
                press_ctrl(KeyCode::Char('v')),
                repeat_ctrl(KeyCode::Char('v')),
                repeat_ctrl(KeyCode::Char('v')),
            ],
        );

        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains("STATUS: READY"),
            "the initial Press should still validate; only the Repeats \
             that follow it must be suppressed"
        );
    }

    #[test]
    fn a_held_n_does_not_leak_into_the_controller_after_cancelling_a_dialog() {
        // A terminal reporting a held `n` as a stream of ordinary `Press`
        // events (indistinguishable from separate presses once `key.kind`
        // can't be trusted — see `is_repeat_untrustworthy`) must not let a
        // second `n` arriving just after the reset dialog closes (F7,
        // since the source is modified) get typed straight into the
        // now-visible source.
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),     // open Target
                press(KeyCode::Enter),     // commit, opening Controller
                press(KeyCode::Char('x')), // modify the controller
                press(KeyCode::F(7)),      // open the reset confirmation
                press(KeyCode::Char('n')), // cancel it
                press(KeyCode::Char('n')), // must be debounced, not typed
            ],
        );

        assert!(!state.reset_confirmation_pending());
        assert_eq!(
            state.controller_source(),
            Some(format!("{}x", intel::STARTER_CONTROLLER)),
            "the second, debounced n must not have been typed into the source"
        );
    }

    #[test]
    fn typing_two_ns_in_a_row_in_the_controller_inserts_both_when_no_dialog_is_open() {
        // The y/n debounce must only apply to a leaked repeat right after
        // a dialog closes (see `a_held_n_does_not_leak_into_the_controller_
        // after_cancelling_a_dialog`), not to `y`/`n` unconditionally —
        // `n` and `y` are also ordinary printable characters, and two
        // typed quickly in a row (e.g. mid-identifier) must both insert,
        // the same as any other repeated character would.
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter), // open Target
                press(KeyCode::Enter), // commit, opening Controller
                press(KeyCode::Char('n')),
                press(KeyCode::Char('n')),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(format!("{}nn", intel::STARTER_CONTROLLER)),
            "both n presses should have inserted; no dialog was ever open"
        );
    }

    #[test]
    fn a_key_release_event_does_not_edit_the_controller() {
        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                release(KeyCode::Char('x')),
            ],
        );

        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn typing_while_the_narrow_reference_pane_is_shown_does_not_edit_the_hidden_source() {
        use state::PaneId;

        let (state, _) = render(
            90,
            30,
            &[
                press(KeyCode::Enter), // inspect the actionable signal, opening Target
                press(KeyCode::Enter), // commit, opening Controller
                press(KeyCode::F(8)),  // move focus to the Lua reference pane
                press(KeyCode::Char('x')),
                press(KeyCode::Backspace),
            ],
        );

        assert_eq!(
            state.focused_pane(View::Controller),
            PaneId::LuaFieldReference
        );
        assert_eq!(
            state.controller_source(),
            Some(intel::STARTER_CONTROLLER.to_string())
        );
    }

    #[test]
    fn a_resize_preserves_focus_moved_by_f8() {
        use state::PaneId;

        let (mut state, _) = render(90, 30, &[press(KeyCode::F(8))]);
        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);

        let mut last_transition_press: TransitionKeyDebounce = None;
        should_redraw(
            &mut state,
            Event::Resize(90, 30),
            (90, 30),
            &mut last_transition_press,
        );

        assert_eq!(state.focused_pane(View::Signals), PaneId::SelectedSignal);
    }

    #[test]
    fn help_scroll_offset_stays_bounded_to_the_viewport_not_just_the_render() {
        let (mut state, _) = render(120, 40, &[press(KeyCode::F(1))]);
        let mut last_transition_press: TransitionKeyDebounce = None;
        for _ in 0..100 {
            should_redraw(
                &mut state,
                press(KeyCode::Down),
                (120, 40),
                &mut last_transition_press,
            );
        }
        let bound = state.scroll_offset(PaneId::Help);
        assert!(
            bound < 100,
            "offset should be clamped to the real content at this viewport, \
             not just the coarse MAX_PANE_SCROLL constant"
        );

        should_redraw(
            &mut state,
            press(KeyCode::Up),
            (120, 40),
            &mut last_transition_press,
        );

        assert_eq!(
            state.scroll_offset(PaneId::Help),
            bound - 1,
            "Up should immediately move the stored offset, not appear stuck"
        );
    }

    #[test]
    fn help_can_scroll_all_the_way_to_its_final_content_at_eighty_columns() {
        // At the supported 80x24 minimum, Help's full contextual + Lua
        // reference content needs more scroll than the coarse internal
        // MAX_PANE_SCROLL cap once alone allowed — that cap must stay
        // comfortably above the real content height, not below it.
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut state = AppState::new();
        let mut last_transition_press: TransitionKeyDebounce = None;
        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");
        for event in std::iter::once(press(KeyCode::F(1)))
            .chain(std::iter::repeat_n(press(KeyCode::Down), 200))
        {
            if should_redraw(&mut state, event, (80, 24), &mut last_transition_press) {
                terminal
                    .draw(|frame| ui::draw(frame, &state))
                    .expect("redraw should succeed");
            }
        }

        assert!(
            buffer_contains(&terminal, "? undiscovered"),
            "200 Down presses should be enough to reach Help's final legend \
             line at 80x24, not get stuck short of it"
        );
    }

    #[test]
    fn f6_deploys_the_starter_controller_and_shows_the_live_operation() {
        let (state, terminal) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(6)),
            ],
        );

        assert_eq!(state.current_view(), View::Operation);
        assert!(
            state
                .operation()
                .is_some_and(|op| !op.finished && !op.paused)
        );
        assert!(buffer_contains(&terminal, "COMPROMISED SATELLITE FEED"));
        assert!(buffer_contains(&terminal, "STATUS: RUNNING"));
    }

    #[test]
    fn space_pauses_and_enter_steps_exactly_one_tick_while_paused() {
        let (state, terminal) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(6)),
                press(KeyCode::Char(' ')), // pause
            ],
        );
        assert!(state.operation().unwrap().paused);
        assert!(buffer_contains(&terminal, "STATUS: PAUSED"));
        let before = state.operation().unwrap().records.len();

        let (state, _) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(6)),
                press(KeyCode::Char(' ')),
                press(KeyCode::Enter), // step once
            ],
        );

        assert!(state.operation().unwrap().paused, "Enter must not resume");
        assert_eq!(state.operation().unwrap().records.len(), before + 1);
    }

    #[test]
    fn navigating_away_and_back_preserves_the_paused_run() {
        let (state, terminal) = render(
            120,
            40,
            &[
                press(KeyCode::Enter),
                press(KeyCode::Enter),
                press(KeyCode::F(6)),
                press(KeyCode::F(4)), // leave Operation: pauses the run
                press(KeyCode::F(5)), // return to Operation
            ],
        );

        assert_eq!(state.current_view(), View::Operation);
        assert!(state.operation().unwrap().paused);
        assert!(buffer_contains(&terminal, "STATUS: PAUSED"));
    }

    const ROUTE_TO_UPLINK: &str = r#"
        local route = { "north", "east", "east", "east", "east", "north", "north", "north" }
        local step = 0
        function on_tick(observation)
            step = step + 1
            return route[step]
        end
    "#;

    /// Clears the starter controller (backspacing every character from its
    /// end-of-document starting cursor) and types `source` in its place,
    /// exactly as a player replacing the controller script would.
    fn clear_and_type(source: &str) -> Vec<Event> {
        let mut events: Vec<Event> = std::iter::repeat_n(
            press(KeyCode::Backspace),
            intel::STARTER_CONTROLLER.chars().count(),
        )
        .collect();
        events.extend(source.chars().map(|c| {
            if c == '\n' {
                press(KeyCode::Enter)
            } else {
                press(KeyCode::Char(c))
            }
        }));
        events
    }

    #[test]
    fn completing_a_scripted_route_lands_on_after_action_with_recognizable_text() {
        // The run auto-advances only while unpaused, and `Enter` only
        // steps a single tick while paused (`docs/TUI_DESIGN.md`, "Pacing
        // controls") — pause immediately after deploying, then step
        // through every remaining tick explicitly to reach completion
        // deterministically.
        let mut events = vec![press(KeyCode::Enter), press(KeyCode::Enter)];
        events.extend(clear_and_type(ROUTE_TO_UPLINK));
        events.push(press(KeyCode::F(6)));
        events.push(press(KeyCode::Char(' '))); // pause
        events.extend(std::iter::repeat_n(press(KeyCode::Enter), 8)); // step 8 ticks

        let (state, terminal) = render(120, 40, &events);

        assert_eq!(state.current_view(), View::AfterAction);
        assert!(state.operation().unwrap().finished);
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(&terminal, "FOOTHOLD ESTABLISHED"));
    }

    #[test]
    fn deploying_a_syntactically_invalid_controller_lands_directly_on_after_action() {
        let mut events = vec![press(KeyCode::Enter), press(KeyCode::Enter)];
        events.extend(clear_and_type("function on_tick("));
        events.push(press(KeyCode::F(6)));

        let (state, terminal) = render(120, 40, &events);

        assert_eq!(state.current_view(), View::AfterAction);
        assert!(state.operation().unwrap().finished);
        assert!(buffer_contains(&terminal, "AFTER-ACTION REPORT"));
        assert!(buffer_contains(&terminal, "controller script error"));
    }

    #[test]
    fn after_action_arrows_do_not_scroll_the_report_while_its_satellite_pane_is_focused() {
        let mut events = vec![press(KeyCode::Enter), press(KeyCode::Enter)];
        events.extend(clear_and_type(ROUTE_TO_UPLINK));
        events.push(press(KeyCode::F(6)));
        events.push(press(KeyCode::Char(' '))); // pause
        events.extend(std::iter::repeat_n(press(KeyCode::Enter), 8)); // step 8 ticks

        // A short viewport (`ui::MIN_ROWS`) so the report actually needs
        // scrolling — otherwise there'd be nothing to distinguish "scrolled"
        // from "already at the bottom".
        let width = 100; // the narrowest width the two-pane layout takes
        let (mut state, mut terminal) = render(width, ui::MIN_ROWS, &events);
        assert_eq!(state.current_view(), View::AfterAction);

        // With the report focused (After Action's default), Down actually
        // scrolls it.
        let mut last_transition_press: TransitionKeyDebounce = None;
        should_redraw(
            &mut state,
            press(KeyCode::Down),
            (width, ui::MIN_ROWS),
            &mut last_transition_press,
        );
        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("redraw should succeed");
        let scrolled_offset = state.scroll_offset(PaneId::Report);
        assert!(scrolled_offset > 0, "report should have scrolled");

        // Moving focus to the final-frame satellite pane must stop further
        // Down presses from scrolling the report.
        should_redraw(
            &mut state,
            press(KeyCode::F(8)),
            (width, ui::MIN_ROWS),
            &mut last_transition_press,
        );
        should_redraw(
            &mut state,
            press(KeyCode::Down),
            (width, ui::MIN_ROWS),
            &mut last_transition_press,
        );

        assert_eq!(state.scroll_offset(PaneId::Report), scrolled_offset);
    }

    #[test]
    fn editing_and_redeploying_from_after_action_completes_the_retry_loop() {
        let mut events = vec![press(KeyCode::Enter), press(KeyCode::Enter)];
        events.extend(clear_and_type("function on_tick("));
        events.push(press(KeyCode::F(6))); // lands on After Action
        events.push(press(KeyCode::F(4))); // back to Controller, edits intact
        let (state, _) = render(120, 40, &events);
        assert_eq!(state.current_view(), View::Controller);
        assert_eq!(
            state.controller_source(),
            Some("function on_tick(".to_string())
        );

        events.extend(clear_and_type(
            "function on_tick(observation) return \"wait\" end",
        ));
        events.push(press(KeyCode::F(6))); // redeploy: no confirmation, nothing active
        let (state, _) = render(120, 40, &events);

        assert_eq!(state.current_view(), View::Operation);
        assert_eq!(
            state.operation().unwrap().deployed_source,
            "function on_tick(observation) return \"wait\" end"
        );
    }

    #[test]
    fn validating_deploying_editing_reviewing_and_retrying_preserves_provenance_end_to_end() {
        // Reach the Controller with a scripted route in place of the
        // starter, and confirm it validates.
        let mut events = vec![press(KeyCode::Enter), press(KeyCode::Enter)];
        events.extend(clear_and_type(ROUTE_TO_UPLINK));
        events.push(press_ctrl(KeyCode::Char('v')));
        let (state, _) = render(120, 40, &events);
        assert_eq!(state.validation(), &state::Validation::Valid);

        // Deploy, pause, and step every tick so the run finishes
        // deterministically, exactly as
        // `completing_a_scripted_route_lands_on_after_action_with_recognizable_text`
        // does above.
        events.push(press(KeyCode::F(6)));
        events.push(press(KeyCode::Char(' '))); // pause
        events.extend(std::iter::repeat_n(press(KeyCode::Enter), 8)); // step 8 ticks
        let (state, _) = render(120, 40, &events);
        assert_eq!(state.current_view(), View::AfterAction);
        assert!(state.operation().unwrap().finished);
        let deployed_before = state.operation().unwrap().deployed_source.to_string();

        // Back to the Controller, edit further, then open Review Run
        // (`F5`, the finished operation's `View::Operation`) without
        // redeploying: the source it shows must still be the source that
        // was actually deployed, not whatever was just typed.
        events.push(press(KeyCode::F(4)));
        events.extend(clear_and_type(
            "function on_tick(observation) return \"wait\" end",
        ));
        events.push(press(KeyCode::F(5)));
        let (state, terminal) = render(120, 40, &events);
        assert_eq!(state.current_view(), View::Operation);
        assert!(buffer_contains(&terminal, "DEPLOYED SOURCE"));
        assert_eq!(
            state.operation().unwrap().deployed_source,
            deployed_before,
            "Review Run must keep showing the frozen deploy snapshot, not \
             the source the player has since typed"
        );

        // Retry: redeploying from the finished run needs no confirmation,
        // and provenance now updates to the newly typed source — it only
        // ever changes on an explicit new deploy, never implicitly.
        events.push(press(KeyCode::F(6)));
        let (state, _) = render(120, 40, &events);
        assert_eq!(state.current_view(), View::Operation);
        assert_eq!(
            state.operation().unwrap().deployed_source,
            "function on_tick(observation) return \"wait\" end"
        );

        // Quit safety still gates on the new, unfinished run after this
        // whole sequence, and cancelling leaves it running.
        events.push(press_ctrl(KeyCode::Char('q')));
        let (state, _) = render(120, 40, &events);
        assert!(state.quit_confirmation_pending());

        events.push(press(KeyCode::Esc));
        let (state, _) = render(120, 40, &events);
        assert!(!state.quit_confirmation_pending());
        assert_eq!(
            state.operation().unwrap().deployed_source,
            "function on_tick(observation) return \"wait\" end"
        );
    }
}
