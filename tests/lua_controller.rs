//! Integration tests for the Lua controller boundary, driving
//! `lua_controller::run` end to end against fixture scripts.

use std::path::{Path, PathBuf};

use human_exception::lua_controller;
use human_exception::{Action, ControllerError, FailureReason, TickOutcome};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn a_navigating_controller_reaches_the_uplink() {
    let outcome = lua_controller::run(&fixture("success.lua"), |_| {}).unwrap();
    assert_eq!(outcome, TickOutcome::Succeeded);
}

#[test]
fn a_controller_that_scans_before_navigating_still_reaches_the_uplink() {
    let mut ticks = Vec::new();
    let outcome = lua_controller::run(&fixture("scan_then_navigate.lua"), |record| {
        ticks.push(record)
    })
    .unwrap();

    assert_eq!(outcome, TickOutcome::Succeeded);
    assert_eq!(ticks[0].action, Action::Scan);
}

#[test]
fn a_waiting_controller_fails_when_the_budget_is_exhausted() {
    let outcome = lua_controller::run(&fixture("always_wait.lua"), |_| {}).unwrap();
    assert_eq!(outcome, TickOutcome::Failed(FailureReason::BudgetExhausted));
}

#[test]
fn a_controller_that_routes_through_the_hazard_reports_a_hazard_entered_event() {
    let mut ticks = Vec::new();
    let outcome =
        lua_controller::run(&fixture("hazard_route.lua"), |record| ticks.push(record)).unwrap();

    assert_eq!(outcome, TickOutcome::Succeeded);
    assert!(ticks.iter().any(|record| {
        record
            .events
            .iter()
            .any(|event| matches!(event, human_exception::SimEvent::HazardEntered { .. }))
    }));
}

#[test]
fn rerunning_the_same_script_is_deterministic() {
    let first = lua_controller::run(&fixture("success.lua"), |_| {}).unwrap();
    let second = lua_controller::run(&fixture("success.lua"), |_| {}).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, TickOutcome::Succeeded);
}

#[test]
fn a_nonexistent_script_is_a_clean_error() {
    let err = lua_controller::run(&fixture("does_not_exist.lua"), |_| {}).unwrap_err();
    assert!(matches!(err, ControllerError::ScriptUnreadable { .. }));
}

#[test]
fn invalid_lua_syntax_is_a_clean_error() {
    let err = lua_controller::run(&fixture("syntax_error.lua"), |_| {}).unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
}

#[test]
fn a_missing_callback_is_a_clean_error() {
    let err = lua_controller::run(&fixture("missing_callback.lua"), |_| {}).unwrap_err();
    assert!(matches!(err, ControllerError::MissingCallback));
}

#[test]
fn a_callback_runtime_error_is_a_clean_error() {
    let err = lua_controller::run(&fixture("callback_error.lua"), |_| {}).unwrap_err();
    assert!(matches!(err, ControllerError::CallbackFailed(_)));
}

#[test]
fn an_unrecognized_action_name_is_a_clean_error() {
    let err = lua_controller::run(&fixture("invalid_action.lua"), |_| {}).unwrap_err();
    assert!(matches!(err, ControllerError::InvalidAction(_)));
}

#[test]
fn an_out_of_bounds_move_is_a_clean_error_without_mutating_state() {
    let err = lua_controller::run(&fixture("out_of_bounds.lua"), |_| {}).unwrap_err();
    match err {
        ControllerError::InvalidAction(detail) => {
            assert!(detail.contains("south"));
        }
        other => panic!("expected InvalidAction, got {other:?}"),
    }
}

#[test]
fn a_move_into_a_wall_is_a_clean_error_without_mutating_state() {
    let err = lua_controller::run(&fixture("into_wall.lua"), |_| {}).unwrap_err();
    match err {
        ControllerError::InvalidAction(detail) => {
            assert!(detail.contains("east"));
            assert!(detail.contains("wall"));
        }
        other => panic!("expected InvalidAction, got {other:?}"),
    }
}

#[test]
fn the_observer_receives_one_record_per_tick_in_order() {
    let mut ticks = Vec::new();
    let outcome =
        lua_controller::run(&fixture("success.lua"), |record| ticks.push(record)).unwrap();

    assert_eq!(outcome, TickOutcome::Succeeded);
    assert!(!ticks.is_empty());
    assert_eq!(ticks.last().unwrap().outcome, TickOutcome::Succeeded);
    for (i, record) in ticks.iter().enumerate() {
        assert_eq!(record.tick, (i + 1) as u32);
    }
}
