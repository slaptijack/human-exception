//! Regression tests for `lua_controller::validate` and `LiveOperation`'s
//! wall-clock execution limits against Lua source that defeats a purely
//! hook-based bound.
//!
//! Kept in their own integration test binary, separate from every other
//! test: every reproduction below genuinely never lets its background
//! thread finish, which permanently holds one of `validate`'s
//! concurrent-validation slots (or, for the `LiveOperation` cases, just
//! leaks a thread) for the rest of whatever process runs them. That's fine
//! here, since nothing else in this binary depends on that slot/thread
//! afterward, but it would spuriously fail unrelated tests sharing a
//! process with them.

use human_exception::ControllerError;
use human_exception::lua_controller::{LiveOperation, validate};

#[test]
fn validate_bounds_a_loop_that_catches_the_instruction_hook_with_pcall() {
    let err = validate("while true do pcall(function() while true do end end) end").unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    assert!(err.to_string().contains("execution allowance"));
}

#[test]
fn live_operation_deploy_bounds_a_top_level_loop_that_catches_the_instruction_hook_with_pcall() {
    // A controller isn't required to pass `validate` before `deploy` (and
    // an edit invalidates any earlier validation anyway), so `deploy`'s own
    // top-level load needs the identical wall-clock backstop `validate`
    // has, not just the same instruction hook.
    let source = "while true do pcall(function() while true do end end) end\n\
                  function on_tick(observation) return 'wait' end";
    let err = LiveOperation::deploy(source).unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    assert!(err.to_string().contains("execution allowance"));
}

#[test]
fn live_operation_step_bounds_an_on_tick_that_never_returns_even_when_it_keeps_catching_the_hook() {
    // Unlike a single caught hook error followed by a normal return (which
    // the instruction hook plus a post-call check already handles), an
    // outer loop that keeps re-triggering `pcall` around its own infinite
    // loop never lets `on_tick` return control to Rust at all. Only a
    // wall-clock timeout on the worker thread that actually owns this
    // `Lua` can end the deployment instead of freezing the caller forever.
    let source = "function on_tick(observation)\n\
                    while true do\n\
                      pcall(function() while true do end end)\n\
                    end\n\
                  end";
    let mut op = LiveOperation::deploy(source).expect("script loads; the loop is only in on_tick");

    let err = op.step().unwrap_err();
    assert!(
        matches!(err, ControllerError::ExecutionLimitExceeded),
        "{err}"
    );
    assert!(op.is_finished());
}

#[test]
fn validate_bounds_a_gc_finalizer_that_hangs_during_teardown() {
    // The script itself loads cleanly and defines on_tick — the hang is
    // entirely inside a `__gc` finalizer on a table nothing keeps a
    // reference to, which only runs once the sandboxed `Lua` is dropped
    // (closing it forces a full garbage-collection pass). That happens
    // after `load_controller` has already returned successfully, so this
    // is a distinct escape from the top-level-script case above: it
    // proves the concurrency-cap slot stays held through Lua teardown,
    // not just through the top-level load.
    let source = "setmetatable({}, {__gc = function() \
                    while true do pcall(function() while true do end end) end \
                  end})\n\
                  function on_tick(observation) return 'wait' end";

    let err = validate(source).unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    assert!(err.to_string().contains("execution allowance"));
}
