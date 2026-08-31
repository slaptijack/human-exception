//! The shared, focus-aware navigation vocabulary for non-editor console
//! surfaces (`docs/TUI_DESIGN.md`, "Console-wide navigation").
//!
//! `Up`/`Down`/`PageUp`/`PageDown`/`Home`/`End` mean the same *kind* of
//! thing everywhere — previous/next, page backward/forward, first/last —
//! but each focused surface interprets that meaning according to its own
//! semantics (a list moves a selection, a scroll surface moves an offset,
//! a chronology steps through review points). This module is the small,
//! shared vocabulary for that: which physical key means which
//! [`NavIntent`], which surface currently owns it (replacing what used to
//! be three independent `(View, PaneId) -> bool` gates, one grown per
//! view), and what existing [`Msg`] that surface already produces for a
//! given intent. It intentionally does not become a general command
//! framework: the Controller editor's own movement/selection model
//! (`super::editor::EditOp`) and Operation's `Space`/`Enter` pacing
//! controls are untouched and out of scope here.

use crossterm::event::KeyCode;

use super::state::{Msg, PaneId, View};

/// A physical navigation key's meaning, independent of which surface
/// interprets it. Mirrors `docs/TUI_DESIGN.md`'s console-wide navigation
/// contract. `PageBackward`/`PageForward` carry a page size — `0` as a
/// placeholder until real rendered viewport geometry is known, the same
/// placeholder-and-rewrite convention `Msg::SelectReviewPointPageBackward`
/// already uses (filled in by `console::mod::should_redraw`, which is the
/// only place with both the message and the current frame size).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavIntent {
    Previous,
    Next,
    PageBackward(usize),
    PageForward(usize),
    First,
    Last,
}

/// Which pane-local surface, if any, currently owns focus-aware navigation
/// input. Determined purely from `(View, PaneId)` — the same snapshot
/// `event::map` already receives — so a pane that isn't the one actually
/// focused is `None` here regardless of what it contains, matching
/// "Focus ownership" in `docs/TUI_DESIGN.md`'s console-wide navigation
/// contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavSurface {
    SignalsList,
    /// A read-only scroll surface. Help and After Action's Report pane
    /// share identical semantics today (`Up`/`Down` scroll by one row), so
    /// one variant serves both rather than a helper apiece.
    Scroll,
    ReviewRun,
}

/// Which surface, if any, currently owns navigation input for `view`
/// focused on `focused_pane`. Replaces three independent gates that used
/// to live in `event.rs`: a Signals-specific inline check, a Help/After
/// Action `scroll_focus_matches`, and a Review Run `review_run_focus_matches`.
pub(crate) fn focused_nav_surface(view: View, focused_pane: PaneId) -> Option<NavSurface> {
    match (view, focused_pane) {
        (View::Signals, PaneId::SignalsList) => Some(NavSurface::SignalsList),
        (View::Help, _) => Some(NavSurface::Scroll),
        (View::AfterAction, PaneId::Report) => Some(NavSurface::Scroll),
        (View::Operation, PaneId::OperationTelemetry) => Some(NavSurface::ReviewRun),
        _ => None,
    }
}

/// Maps a physical key to the [`NavIntent`] it represents, or `None` if
/// `code` isn't one of the console-wide navigation keys at all.
pub(crate) fn intent_for_key(code: KeyCode) -> Option<NavIntent> {
    match code {
        KeyCode::Up => Some(NavIntent::Previous),
        KeyCode::Down => Some(NavIntent::Next),
        KeyCode::PageUp => Some(NavIntent::PageBackward(0)),
        KeyCode::PageDown => Some(NavIntent::PageForward(0)),
        KeyCode::Home => Some(NavIntent::First),
        KeyCode::End => Some(NavIntent::Last),
        _ => None,
    }
}

/// Interprets `intent` for `surface`, returning the existing [`Msg`] that
/// surface already used for that action, or `None` when this surface
/// doesn't (yet) support that intent. Signals and the read-only scroll
/// surfaces (Help, After Action's Report pane) only support `Previous`/
/// `Next` today — `PageBackward`/`PageForward`/`First`/`Last` bindings for
/// those are the job of later issues (#148, #149), not this one, so this
/// function deliberately returns `None` for those combinations rather than
/// inventing new behavior.
pub(crate) fn route(surface: NavSurface, intent: NavIntent) -> Option<Msg> {
    match (surface, intent) {
        (NavSurface::SignalsList, NavIntent::Previous) => Some(Msg::SelectPreviousSignal),
        (NavSurface::SignalsList, NavIntent::Next) => Some(Msg::SelectNextSignal),
        (NavSurface::Scroll, NavIntent::Previous) => Some(Msg::ScrollUp),
        (NavSurface::Scroll, NavIntent::Next) => Some(Msg::ScrollDown),
        (NavSurface::ReviewRun, NavIntent::Previous) => Some(Msg::SelectPreviousReviewPoint),
        (NavSurface::ReviewRun, NavIntent::Next) => Some(Msg::SelectNextReviewPoint),
        (NavSurface::ReviewRun, NavIntent::PageBackward(page)) => {
            Some(Msg::SelectReviewPointPageBackward(page))
        }
        (NavSurface::ReviewRun, NavIntent::PageForward(page)) => {
            Some(Msg::SelectReviewPointPageForward(page))
        }
        (NavSurface::ReviewRun, NavIntent::First) => Some(Msg::SelectFirstReviewPoint),
        (NavSurface::ReviewRun, NavIntent::Last) => Some(Msg::SelectLastReviewPoint),
        (NavSurface::SignalsList, _) | (NavSurface::Scroll, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_list_owns_navigation_only_when_focused() {
        assert_eq!(
            focused_nav_surface(View::Signals, PaneId::SignalsList),
            Some(NavSurface::SignalsList)
        );
        assert_eq!(
            focused_nav_surface(View::Signals, PaneId::SelectedSignal),
            None
        );
    }

    #[test]
    fn help_always_owns_navigation_since_it_has_one_pane() {
        assert_eq!(
            focused_nav_surface(View::Help, PaneId::Help),
            Some(NavSurface::Scroll)
        );
    }

    #[test]
    fn after_action_report_owns_navigation_only_when_focused() {
        assert_eq!(
            focused_nav_surface(View::AfterAction, PaneId::Report),
            Some(NavSurface::Scroll)
        );
        assert_eq!(
            focused_nav_surface(View::AfterAction, PaneId::FinalFrame),
            None
        );
    }

    #[test]
    fn review_run_owns_navigation_only_when_telemetry_pane_focused() {
        assert_eq!(
            focused_nav_surface(View::Operation, PaneId::OperationTelemetry),
            Some(NavSurface::ReviewRun)
        );
        assert_eq!(
            focused_nav_surface(View::Operation, PaneId::Satellite),
            None
        );
    }

    #[test]
    fn no_other_view_or_pane_owns_navigation() {
        assert_eq!(
            focused_nav_surface(View::Target, PaneId::TargetIntelligence),
            None
        );
        assert_eq!(focused_nav_surface(View::Target, PaneId::Provenance), None);
        assert_eq!(
            focused_nav_surface(View::Controller, PaneId::ControllerSource),
            None
        );
        assert_eq!(
            focused_nav_surface(View::Controller, PaneId::LuaFieldReference),
            None
        );
    }

    #[test]
    fn intent_for_key_covers_the_six_navigation_keys() {
        assert_eq!(intent_for_key(KeyCode::Up), Some(NavIntent::Previous));
        assert_eq!(intent_for_key(KeyCode::Down), Some(NavIntent::Next));
        assert_eq!(
            intent_for_key(KeyCode::PageUp),
            Some(NavIntent::PageBackward(0))
        );
        assert_eq!(
            intent_for_key(KeyCode::PageDown),
            Some(NavIntent::PageForward(0))
        );
        assert_eq!(intent_for_key(KeyCode::Home), Some(NavIntent::First));
        assert_eq!(intent_for_key(KeyCode::End), Some(NavIntent::Last));
        assert_eq!(intent_for_key(KeyCode::Char('a')), None);
    }

    #[test]
    fn signals_list_only_supports_previous_and_next_today() {
        assert_eq!(
            route(NavSurface::SignalsList, NavIntent::Previous),
            Some(Msg::SelectPreviousSignal)
        );
        assert_eq!(
            route(NavSurface::SignalsList, NavIntent::Next),
            Some(Msg::SelectNextSignal)
        );
        assert_eq!(
            route(NavSurface::SignalsList, NavIntent::PageBackward(3)),
            None
        );
        assert_eq!(
            route(NavSurface::SignalsList, NavIntent::PageForward(3)),
            None
        );
        assert_eq!(route(NavSurface::SignalsList, NavIntent::First), None);
        assert_eq!(route(NavSurface::SignalsList, NavIntent::Last), None);
    }

    #[test]
    fn scroll_surface_only_supports_previous_and_next_today() {
        assert_eq!(
            route(NavSurface::Scroll, NavIntent::Previous),
            Some(Msg::ScrollUp)
        );
        assert_eq!(
            route(NavSurface::Scroll, NavIntent::Next),
            Some(Msg::ScrollDown)
        );
        assert_eq!(route(NavSurface::Scroll, NavIntent::PageBackward(3)), None);
        assert_eq!(route(NavSurface::Scroll, NavIntent::PageForward(3)), None);
        assert_eq!(route(NavSurface::Scroll, NavIntent::First), None);
        assert_eq!(route(NavSurface::Scroll, NavIntent::Last), None);
    }

    #[test]
    fn review_run_supports_the_full_navigation_vocabulary() {
        assert_eq!(
            route(NavSurface::ReviewRun, NavIntent::Previous),
            Some(Msg::SelectPreviousReviewPoint)
        );
        assert_eq!(
            route(NavSurface::ReviewRun, NavIntent::Next),
            Some(Msg::SelectNextReviewPoint)
        );
        assert_eq!(
            route(NavSurface::ReviewRun, NavIntent::PageBackward(7)),
            Some(Msg::SelectReviewPointPageBackward(7))
        );
        assert_eq!(
            route(NavSurface::ReviewRun, NavIntent::PageForward(7)),
            Some(Msg::SelectReviewPointPageForward(7))
        );
        assert_eq!(
            route(NavSurface::ReviewRun, NavIntent::First),
            Some(Msg::SelectFirstReviewPoint)
        );
        assert_eq!(
            route(NavSurface::ReviewRun, NavIntent::Last),
            Some(Msg::SelectLastReviewPoint)
        );
    }
}
