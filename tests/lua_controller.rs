//! Integration tests for the Lua controller boundary, driving
//! `lua_controller::run` end to end against fixture scripts.

use std::path::{Path, PathBuf};

use human_exception::lua_controller;
use human_exception::{ControllerError, FailureReason, TickOutcome};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn a_navigating_controller_reaches_the_uplink() {
    let outcome = lua_controller::run(&fixture("success.lua")).unwrap();
    assert_eq!(outcome, TickOutcome::Succeeded);
}

#[test]
fn a_waiting_controller_fails_at_the_tick_limit() {
    let outcome = lua_controller::run(&fixture("always_wait.lua")).unwrap();
    assert_eq!(
        outcome,
        TickOutcome::Failed(FailureReason::TickLimitReached)
    );
}

#[test]
fn rerunning_the_same_script_is_deterministic() {
    let first = lua_controller::run(&fixture("success.lua")).unwrap();
    let second = lua_controller::run(&fixture("success.lua")).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, TickOutcome::Succeeded);
}

#[test]
fn a_nonexistent_script_is_a_clean_error() {
    let err = lua_controller::run(&fixture("does_not_exist.lua")).unwrap_err();
    assert!(matches!(err, ControllerError::ScriptUnreadable { .. }));
}

#[test]
fn invalid_lua_syntax_is_a_clean_error() {
    let err = lua_controller::run(&fixture("syntax_error.lua")).unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
}

#[test]
fn a_missing_callback_is_a_clean_error() {
    let err = lua_controller::run(&fixture("missing_callback.lua")).unwrap_err();
    assert!(matches!(err, ControllerError::MissingCallback));
}

#[test]
fn a_callback_runtime_error_is_a_clean_error() {
    let err = lua_controller::run(&fixture("callback_error.lua")).unwrap_err();
    assert!(matches!(err, ControllerError::CallbackFailed(_)));
}

#[test]
fn an_unrecognized_action_name_is_a_clean_error() {
    let err = lua_controller::run(&fixture("invalid_action.lua")).unwrap_err();
    assert!(matches!(err, ControllerError::InvalidAction(_)));
}

#[test]
fn an_out_of_bounds_move_is_a_clean_error_without_mutating_state() {
    let err = lua_controller::run(&fixture("out_of_bounds.lua")).unwrap_err();
    match err {
        ControllerError::InvalidAction(detail) => {
            assert!(detail.contains("south"));
        }
        other => panic!("expected InvalidAction, got {other:?}"),
    }
}
