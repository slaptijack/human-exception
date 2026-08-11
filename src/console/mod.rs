//! The persistent, full-screen resistance-console session.
//!
//! This module hosts the interactive shell described by
//! `docs/TUI_DESIGN.md`: a session/state model ([`state`]), key-to-intent
//! mapping ([`event`]), and rendering ([`ui`]). Signals, Target, and
//! Controller are populated; Operation and After Action still show
//! placeholder content for later issues (#45-#46) to populate.

pub mod editor;
pub mod event;
pub mod intel;
pub mod state;
pub mod ui;

use std::io;
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::Show;
use crossterm::event::{
    self as term_event, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use state::{AppState, Msg};

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

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    if KEYBOARD_ENHANCEMENT_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    Ok(())
}

/// Drives the session against a real terminal, reading real `crossterm`
/// events. The pure dispatch/redraw logic lives in [`should_redraw`] and
/// [`ui::draw`] so it can also be driven against `TestBackend` in tests.
fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut state = AppState::new();
    let mut frame_size = (0u16, 0u16);

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
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    // `REPORT_EVENT_TYPES` alone only covers keys already
                    // sent as CSI-u sequences; plain-text keys like `Enter`
                    // and `y`/`n` — exactly the transition keys that guard
                    // cares about — need this too to report `Repeat`/`Release`
                    // instead of only ever `Press`.
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        )
        .is_ok()
    {
        KEYBOARD_ENHANCEMENT_PUSHED.store(true, Ordering::SeqCst);
    }

    while !state.should_quit() {
        let event = term_event::read()?;
        if should_redraw(&mut state, event, frame_size) {
            terminal.draw(|frame| {
                let area = frame.area();
                frame_size = (area.width, area.height);
                ui::draw(frame, &state);
            })?;
        }
    }

    Ok(())
}

/// Applies a single terminal event to `state`, returning whether the frame
/// needs to be redrawn as a result. `frame_size` is the frame's size as of
/// the most recent draw — slightly stale for the resize event that changes
/// it, but accurate again by the time the next key event arrives — and is
/// used both to bound `help_scroll` against the viewport it will actually
/// render into, and to tell whether only the geometry warning is showing.
///
/// A resize always needs a redraw, since the geometry warning (or the shell
/// it replaces) depends on the frame size, not on any key event. It also
/// clears layout-specific state (the narrow-mode secondary-pane toggle) so
/// it can't leak across a change in available width.
fn should_redraw(state: &mut AppState, event: Event, frame_size: (u16, u16)) -> bool {
    if matches!(event, Event::Resize(_, _)) {
        state.handle_resize();
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
    // `Enter`, `Esc`, `y`/`n`, and the function keys are excluded from
    // `Repeat` entirely, Press-only, because they trigger state
    // *transitions* (Activate, Confirm/Cancel a dialog, Dismiss Help,
    // navigate) rather than a repeatable edit: a terminal's repeat events
    // fire on a timer independent of how fast the app already processed
    // the initial press, so a held key can otherwise cascade through
    // several unrelated meanings in a row — e.g. Activate in Signals opens
    // Target, a later repeat Activates again there and opens Controller,
    // and a further repeat inserts a newline into its now-visible source,
    // all from one keypress the player thought they'd already released.
    // `y`/`n` specifically confirm/cancel a dialog; a held one whose
    // repeat arrives just after the dialog closes would otherwise type
    // that letter straight into the controller it just reset or the quit
    // it just cancelled. The cost is that holding `y`/`n` in Controller
    // to type several of that letter no longer auto-repeats — a minor,
    // easily-worked-around loss (press it again) next to that risk.
    //
    // `Ctrl+V` (the fallback validation binding, see `event::map`) is
    // excluded the same way, but only that exact chord — plain `v` (no
    // Ctrl) is an ordinary printable character that must keep repeating
    // for typing. Each `Ctrl+V` repeat starts another `validate` worker
    // (see `lua_controller::validate`'s `MAX_CONCURRENT_VALIDATIONS`
    // cap), so a terminal that reports a held `Ctrl+V` as a stream of
    // `Repeat` events could otherwise exhaust every validation slot from
    // one keypress, the same class of risk transition keys exist for
    // above.
    let is_ctrl_v = key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL);
    let kind_allowed = match key.kind {
        KeyEventKind::Press => true,
        KeyEventKind::Repeat => {
            !is_ctrl_v
                && !matches!(
                    key.code,
                    KeyCode::Enter
                        | KeyCode::Esc
                        | KeyCode::F(_)
                        | KeyCode::Char('y')
                        | KeyCode::Char('n')
                )
        }
        KeyEventKind::Release => false,
    };
    if !kind_allowed {
        return false;
    }

    // Below the supported minimum, only the geometry warning and `Ctrl+Q`
    // are shown; every other intent must stay inert instead of silently
    // mutating state (e.g. committing an opportunity) the player can't see.
    let undersized = frame_size.0 < ui::MIN_COLUMNS || frame_size.1 < ui::MIN_ROWS;

    match event::map(
        key,
        state.current_view(),
        state.reset_confirmation_pending(),
        state.quit_confirmation_pending(),
        ui::controller_source_visible(state, frame_size.0),
    ) {
        Some(msg) => {
            let is_quit_related =
                matches!(msg, Msg::RequestQuit | Msg::ConfirmQuit | Msg::CancelQuit);
            if undersized && !is_quit_related {
                return false;
            }
            state.apply(msg);
            if matches!(msg, Msg::ScrollHelpUp | Msg::ScrollHelpDown) {
                let max = ui::help_max_scroll(state, frame_size.0, frame_size.1);
                state.clamp_help_scroll(max);
            }
            true
        }
        None => false,
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

    fn render(width: u16, height: u16, events: &[Event]) -> (AppState, Terminal<TestBackend>) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut state = AppState::new();

        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");

        for event in events {
            if should_redraw(&mut state, event.clone(), (width, height)) {
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
        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");
        while !state.should_quit() {
            let event = events
                .next()
                .expect("script should quit before running out");
            should_redraw(&mut state, event, (120, 40));
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
        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");
        assert!(buffer_contains(&terminal, "Terminal link degraded."));

        terminal.backend_mut().resize(120, 40);
        if should_redraw(&mut state, Event::Resize(120, 40), (120, 40)) {
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
            Some(format!("{}xx", intel::STARTER_CONTROLLER).as_str()),
            "held-key Repeat events should insert just like Press, not be ignored"
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

        assert_eq!(state.controller_source(), Some(intel::STARTER_CONTROLLER));
    }

    #[test]
    fn typing_while_the_narrow_reference_pane_is_shown_does_not_edit_the_hidden_source() {
        let (state, _) = render(
            90,
            30,
            &[
                press(KeyCode::Enter), // inspect the actionable signal, opening Target
                press(KeyCode::Enter), // commit, opening Controller
                press(KeyCode::F(8)),  // swap to the Lua reference pane
                press(KeyCode::Char('x')),
                press(KeyCode::Backspace),
            ],
        );

        assert!(state.narrow_secondary_visible());
        assert_eq!(state.controller_source(), Some(intel::STARTER_CONTROLLER));
    }

    #[test]
    fn a_resize_clears_the_narrow_secondary_pane_toggle() {
        let (mut state, _) = render(90, 30, &[press(KeyCode::F(8))]);
        assert!(state.narrow_secondary_visible());

        should_redraw(&mut state, Event::Resize(90, 30), (90, 30));

        assert!(!state.narrow_secondary_visible());
    }

    #[test]
    fn help_scroll_offset_stays_bounded_to_the_viewport_not_just_the_render() {
        let (mut state, _) = render(120, 40, &[press(KeyCode::F(1))]);
        for _ in 0..100 {
            should_redraw(&mut state, press(KeyCode::Down), (120, 40));
        }
        let bound = state.help_scroll();
        assert!(
            bound < 100,
            "offset should be clamped to the real content at this viewport, \
             not just the coarse MAX_HELP_SCROLL constant"
        );

        should_redraw(&mut state, press(KeyCode::Up), (120, 40));

        assert_eq!(
            state.help_scroll(),
            bound - 1,
            "Up should immediately move the stored offset, not appear stuck"
        );
    }
}
