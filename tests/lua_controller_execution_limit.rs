//! Regression tests for `lua_controller::validate`'s wall-clock execution
//! limit against Lua source that defeats a purely hook-based bound.
//!
//! Kept in their own integration test binary, separate from every other
//! test: both reproductions below genuinely never let their background
//! thread finish, which permanently holds one of `validate`'s
//! concurrent-validation slots for the rest of whatever process runs
//! them. That's fine here, since nothing else in this binary calls
//! `validate` after they run, but it would spuriously fail unrelated
//! tests sharing a process with them.

use human_exception::ControllerError;
use human_exception::lua_controller::validate;

#[test]
fn validate_bounds_a_loop_that_catches_the_instruction_hook_with_pcall() {
    let err = validate("while true do pcall(function() while true do end end) end").unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    assert!(err.to_string().contains("execution allowance"));
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
