//! The persistent, full-screen resistance-console session.
//!
//! This module hosts the interactive shell described by
//! `docs/TUI_DESIGN.md`: a session/state model ([`state`]), key-to-intent
//! mapping ([`event`]), and rendering ([`ui`]). It intentionally contains no
//! gameplay yet — every major view shows placeholder content for later
//! issues (#43-#46) to populate.

pub mod event;
pub mod intel;
pub mod state;
pub mod ui;

use std::io;
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;

use crossterm::cursor::Show;
use crossterm::event::{self as term_event, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use state::AppState;

type PanicHook = dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static;

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
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    Ok(())
}

/// Drives the session against a real terminal, reading real `crossterm`
/// events. The pure dispatch/redraw logic lives in [`should_redraw`] and
/// [`ui::draw`] so it can also be driven against `TestBackend` in tests.
fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut state = AppState::new();

    terminal.draw(|frame| ui::draw(frame, &state))?;

    while !state.should_quit() {
        let event = term_event::read()?;
        if should_redraw(&mut state, event) {
            terminal.draw(|frame| ui::draw(frame, &state))?;
        }
    }

    Ok(())
}

/// Applies a single terminal event to `state`, returning whether the frame
/// needs to be redrawn as a result.
///
/// A resize never changes session state but always needs a redraw: the
/// geometry warning (or the shell it replaces) depends on the frame size,
/// not on any key event.
fn should_redraw(state: &mut AppState, event: Event) -> bool {
    if matches!(event, Event::Resize(_, _)) {
        return true;
    }

    let Event::Key(key) = event else {
        return false;
    };
    if key.kind != KeyEventKind::Press {
        return false;
    }

    match event::map(key, state.current_view()) {
        Some(msg) => {
            state.apply(msg);
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

    fn render(width: u16, height: u16, events: &[Event]) -> (AppState, Terminal<TestBackend>) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut state = AppState::new();

        terminal
            .draw(|frame| ui::draw(frame, &state))
            .expect("initial draw should succeed");

        for event in events {
            if should_redraw(&mut state, event.clone()) {
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
            should_redraw(&mut state, event);
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
        if should_redraw(&mut state, Event::Resize(120, 40)) {
            terminal
                .draw(|frame| ui::draw(frame, &state))
                .expect("redraw should succeed");
        }

        assert!(buffer_contains(&terminal, "SIGNALS"));
    }
}
