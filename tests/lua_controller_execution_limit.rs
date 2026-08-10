//! Regression test for `lua_controller::validate`'s wall-clock execution
//! limit against Lua source that catches and re-triggers the
//! instruction-count hook's error with `pcall`, defeating a hook-only
//! bound.
//!
//! Kept in its own integration test binary, separate from every other
//! test: the reproduction below genuinely never lets its background
//! thread finish, which permanently holds `validate`'s single
//! concurrent-validation slot for the rest of whatever process runs it.
//! That's fine here, since nothing else in this binary calls `validate`
//! afterward, but it would spuriously fail unrelated tests sharing a
//! process with it.

use human_exception::ControllerError;
use human_exception::lua_controller::validate;

#[test]
fn validate_bounds_a_loop_that_catches_the_instruction_hook_with_pcall() {
    let err = validate("while true do pcall(function() while true do end end) end").unwrap_err();
    assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    assert!(err.to_string().contains("execution allowance"));
}
