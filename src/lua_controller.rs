//! The Lua controller boundary for the reconnaissance simulation.
//!
//! A player script is required to define one global callback, [`ON_TICK`],
//! invoked once per tick with a read-only [`Observation`]. The callback must
//! return the name of one [`Action`] as a string. This module translates
//! that return value into a validated `Action`, submits it to the
//! authoritative [`Simulation`], and repeats until the operation ends.
//!
//! Lua cannot reach `Simulation` directly: it only ever sees an
//! `Observation` table and only ever produces an action name. The contract
//! is intentionally small and provisional.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, MultiValue, StdLib, Table, Value,
    VmState,
};

use crate::simulation::{
    Action, ActionError, DiscoveredTile, Observation, Position, SimEvent, Simulation, TickOutcome,
    TileKind,
};

/// The name of the one callback a player script must define.
pub const ON_TICK: &str = "on_tick";

/// A generous but bounded ceiling on a controller's Lua memory use. Nothing
/// in the on_tick contract needs more than a trivial amount of state; this
/// exists so a native-library allocation (e.g. `string.rep("x", 1 << 30)`)
/// fails fast instead of exhausting host memory, regardless of how many
/// times a script retries it.
const SANDBOX_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// The fixed seed every sandboxed `Lua`'s `math.random` is reseeded with.
/// Lua 5.4 otherwise auto-seeds `math.random` from wall-clock time and the
/// Lua state's memory address at library-open time, which would make
/// identical controller source produce different `math.random` sequences
/// — and therefore potentially different validation results or deployed
/// behavior — across separate runs. AGENTS.md's "Keep the simulation core
/// deterministic" requirement applies here exactly as it does to the rest
/// of the simulation: the same source must behave the same way every time.
const SANDBOX_RANDOM_SEED: i64 = 1;

/// Builds a `Lua` instance exposing only the standard libraries a
/// controller's `on_tick` contract needs (tables, strings, numbers), never
/// `io`, `os`, `package`, `coroutine`, or `debug`, with `dofile`/`loadfile`/
/// `load`/`print` additionally stripped (the base library installs them
/// regardless of which `StdLib` flags are requested) and a bounded memory
/// ceiling. Player Lua is untrusted input (AGENTS.md, "Treat Lua programs
/// as untrusted input"); nothing in the on_tick contract needs filesystem,
/// process, module-loading, or dynamic-code-loading access. `print` writes
/// straight to process stdout — including raw escape sequences — which
/// would corrupt the alternate screen ratatui owns while the console is
/// running. `load` accepts Lua 5.4 *binary* chunks as well as text by
/// default; unlike text source (which the parser fully validates),
/// malformed bytecode is trusted structurally and can crash the process
/// outright rather than fail as a recoverable Lua error — and the string
/// library alone is enough to assemble arbitrary bytes for one. Nothing in
/// the on_tick contract needs to load code dynamically at all, so removing
/// `load` avoids needing to reason about restricting it to text-only mode.
fn sandboxed_lua() -> Lua {
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH;
    let lua = Lua::new_with(libs, LuaOptions::default())
        .expect("sandboxed stdlib set excludes debug/ffi, so this cannot fail");
    let globals = lua.globals();
    let _ = globals.set("dofile", Value::Nil);
    let _ = globals.set("loadfile", Value::Nil);
    let _ = globals.set("load", Value::Nil);
    let _ = globals.set("print", Value::Nil);
    let _ = lua.set_memory_limit(SANDBOX_MEMORY_LIMIT_BYTES);
    if let Ok(math) = globals.get::<Table>("math")
        && let Ok(randomseed) = math.get::<Function>("randomseed")
    {
        let _ = randomseed.call::<()>(SANDBOX_RANDOM_SEED);
        // `math.randomseed()` (no arguments) re-seeds from wall-clock time
        // in Lua 5.4, which would undo the fixed seed above the moment a
        // player script called it. There's no argument-checking variant to
        // keep, so remove the function entirely rather than leave a
        // nondeterministic escape hatch next to a "deterministic" seed.
        let _ = math.set("randomseed", Value::Nil);
    }
    if let Err(err) = install_deterministic_table_iteration(&lua) {
        debug_assert!(
            false,
            "deterministic pairs/next install should not fail: {err}"
        );
    }
    if let Err(err) = install_deterministic_tostring(&lua) {
        debug_assert!(
            false,
            "deterministic tostring install should not fail: {err}"
        );
    }
    if let Err(err) = install_deterministic_string_format(&lua) {
        debug_assert!(
            false,
            "deterministic string.format install should not fail: {err}"
        );
    }
    lua
}

/// Overrides `string.format` (which also covers `s:format(...)` method
/// syntax, since both resolve to the same `string` table entry) to reject
/// a format string containing a `%p` conversion. Lua 5.4 added `%p` to
/// print a value's raw process pointer — a second, independent way to leak
/// the same nondeterministic address information [`install_deterministic_tostring`]
/// closes for the default `tostring` representation, but reachable even
/// after that fix since `string.format` never calls the (now-overridden)
/// global `tostring` internally. Unlike `tostring`'s output, `%p`'s
/// expansion can appear anywhere inside an otherwise arbitrary formatted
/// string, so there's no reliable output pattern to normalize after the
/// fact; refusing the call outright (like `dofile`/`loadfile`/`print`
/// before it) is the safe option.
fn install_deterministic_string_format(lua: &Lua) -> mlua::Result<()> {
    let string_lib: Table = lua.globals().get("string")?;
    let real_format: Function = string_lib.get("format")?;
    // Already the overridden version by this point (installed earlier in
    // `sandboxed_lua`): reused so a `%s`/`%q` argument gets exactly the
    // same address-hiding, custom-`__tostring`-respecting, binary-safe
    // treatment `tostring(...)` itself gets, instead of duplicating that
    // logic here.
    let det_tostring: Function = lua.globals().get("tostring")?;
    let det_format = lua.create_function(
        move |_, args: MultiValue| -> mlua::Result<mlua::LuaString> {
            let mut iter = args.into_iter();
            let Some(fmt_value) = iter.next() else {
                return real_format.call(MultiValue::new());
            };
            if let Value::String(fmt) = &fmt_value
                && format_string_uses_pointer_specifier(&fmt.as_bytes())
            {
                return Err(mlua::Error::RuntimeError(
                    "string.format: '%p' is not available (it would expose a process memory \
                 address, which is nondeterministic)"
                        .to_string(),
                ));
            }
            // `%s`/`%q` format a value via Lua's own C-level `tolstring`,
            // which respects a value's `__tostring` metamethod but not
            // our *overridden* global `tostring` — so a table or function
            // argument without one would still get the real, nondeterministic
            // "type: 0xADDRESS" text straight from `%s`, independent of the
            // `%p` case above. Route any table/function/thread/userdata
            // argument through the deterministic `tostring` first so
            // `real_format` only ever sees an ordinary string for it,
            // regardless of which conversion the format string uses it
            // with — a specifier that expected a number would already have
            // rejected the original table/function argument too, so this
            // doesn't change any currently-succeeding case, only the ones
            // that were leaking an address.
            let mut rebuilt = MultiValue::new();
            rebuilt.push_back(fmt_value);
            for value in iter {
                let value = match value {
                    Value::Table(_)
                    | Value::Function(_)
                    | Value::Thread(_)
                    | Value::UserData(_) => {
                        Value::String(det_tostring.call::<mlua::LuaString>(value)?)
                    }
                    other => other,
                };
                rebuilt.push_back(value);
            }
            real_format.call(rebuilt)
        },
    )?;
    string_lib.set("format", det_format)?;
    Ok(())
}

/// Whether `fmt` contains a genuine `%p` conversion specifier (as opposed
/// to a `%%p`, which is an escaped literal `%` followed by the letter
/// `p`). Handles the usual printf-style flag/width/precision characters
/// between `%` and the conversion letter (e.g. `%-10p`). Operates on raw
/// bytes, not `&str`, since Lua strings (including format strings) are
/// arbitrary byte sequences, not necessarily valid UTF-8.
fn format_string_uses_pointer_specifier(fmt: &[u8]) -> bool {
    let bytes = fmt;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'%' {
            i += 1;
            continue;
        }
        while i < bytes.len() && matches!(bytes[i], b'-' | b'+' | b' ' | b'#' | b'0'..=b'9' | b'.')
        {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'p' {
            return true;
        }
        i += 1;
    }
    false
}

/// Overrides the global `tostring` so the default (no `__tostring`
/// metamethod) representation of a table, function, thread, or userdata —
/// normally `"<type>: 0x<address>"`, baking in that value's actual process
/// memory address — reports a fixed placeholder address instead. Without
/// this, a script branching on that address (e.g. a digit of
/// `tostring({})`) could behave differently between separate runs of
/// byte-identical source purely because of address-space layout
/// randomization, the same class of leak the fixed `math.random` seed and
/// deterministic `pairs`/`next` close for other sources of entropy. Any
/// other `tostring` result — numbers, strings, booleans, nil, or a value
/// with its own `__tostring` metamethod — passes through unchanged.
fn install_deterministic_tostring(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let real_tostring: Function = globals.get("tostring")?;
    let det_tostring =
        lua.create_function(move |lua, value: Value| -> mlua::Result<mlua::LuaString> {
            let text: mlua::LuaString = real_tostring.call(value)?;
            // Lua strings are arbitrary byte sequences, not necessarily
            // UTF-8 (e.g. `tostring(string.char(255))`); work on the raw
            // bytes and hand back the original `LuaString` unchanged
            // whenever no address pattern matches, rather than routing
            // through a Rust `String` (which would reject anything that
            // isn't valid UTF-8, breaking `tostring` for ordinary binary
            // Lua strings this override was never meant to touch).
            let bytes = text.as_bytes();
            for prefix in [
                &b"table: "[..],
                &b"function: "[..],
                &b"thread: "[..],
                &b"userdata: "[..],
            ] {
                if let Some(rest) = bytes.strip_prefix(prefix)
                    && rest.starts_with(b"0x")
                {
                    let mut placeholder = prefix.to_vec();
                    placeholder.extend_from_slice(b"0x0");
                    return lua.create_string(placeholder);
                }
            }
            Ok(text)
        })?;
    globals.set("tostring", det_tostring)?;
    Ok(())
}

/// Overrides the global `pairs`/`next` with versions that iterate a
/// table's entries in a fixed order (booleans, then numbers, then strings)
/// instead of Lua 5.4's real order.
///
/// Lua's own table/string hashing is randomized per `Lua` instance from
/// wall-clock time and the state's memory address specifically to resist
/// hash-flooding attacks, with no public API to fix that seed — so two
/// fresh sandboxes running byte-identical source can otherwise iterate the
/// same string-keyed table in a different order and make a different
/// choice from it (e.g. `for k in pairs(actions) do return k end`),
/// breaking the same "same input, same output" guarantee `sandboxed_lua`'s
/// fixed `math.random` seed exists to uphold. A key of any other type
/// (table, function, thread, userdata) has no identity that's both stable
/// across separate `Lua` instances and independent of its own address —
/// there is no meaningful deterministic order to assign it — so
/// [`sorted_table_entries`] rejects such a key outright instead of
/// silently leaving it in whatever order the randomized native iteration
/// produced.
fn install_deterministic_table_iteration(lua: &Lua) -> mlua::Result<()> {
    // Finds the entry immediately after `key` in sorted order using the
    // comparator directly, rather than searching for a `k == key` match
    // and yielding whatever follows it. That distinction matters because
    // real Lua's `next` explicitly supports clearing the *current* field
    // during a manual traversal (`next(t, k)`, then `t[k] = nil`, then
    // `next(t, k)` again with the same now-deleted `k`): comparing against
    // `key`'s value still tells us where it *would* sort even once it's no
    // longer a member of `table` at all, so a deleted key resumes
    // iteration correctly instead of silently ending it.
    let det_next = lua.create_function(|_, (table, key): (Table, Value)| {
        let entries = sorted_table_entries(&table)?;
        if matches!(key, Value::Nil) {
            return Ok(entries
                .into_iter()
                .next()
                .unwrap_or((Value::Nil, Value::Nil)));
        }
        Ok(entries
            .into_iter()
            .find(|(k, _)| compare_lua_keys(k, &key) == std::cmp::Ordering::Greater)
            .unwrap_or((Value::Nil, Value::Nil)))
    })?;

    let det_pairs = lua.create_function(move |lua, table: Table| -> mlua::Result<MultiValue> {
        // Real Lua 5.4 defers entirely to a table's `__pairs` metamethod
        // when present (checked before ever calling the real `next`);
        // match that instead of silently overriding a proxy/wrapper
        // table's own iteration behavior with ours. A script's own
        // `__pairs` is responsible for its own determinism, same as a
        // custom `__tostring` is for its own output.
        if let Some(metatable) = table.metatable()
            && let Ok(pairs_mm) = metatable.get::<Function>("__pairs")
        {
            return pairs_mm.call::<MultiValue>(table);
        }
        // Snapshot only the sorted *key order* once, here, and walk it by
        // position — not `next`'s comparator-based search on every step,
        // and not a snapshot of (key, value) pairs either. A pure-position
        // walk survives a table cleared mid-traversal (`for k in pairs(t)
        // do t[k] = nil end`), which a per-step search through the table's
        // live contents can't (the deleted key is gone from what's being
        // searched). Re-reading each value from the live table when its
        // key is yielded — instead of a value cloned back when `pairs`
        // was first called — keeps Lua's other supported mid-traversal
        // edit, changing an existing field's value before it's visited,
        // showing that current value rather than a stale one.
        let keys: Vec<Value> = sorted_table_entries(&table)?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let position = std::cell::Cell::new(0usize);
        let live_table = table.clone();
        let iterator =
            lua.create_function(move |lua, _: MultiValue| -> mlua::Result<MultiValue> {
                let i = position.get();
                let Some(key) = keys.get(i) else {
                    return Ok(MultiValue::new());
                };
                position.set(i + 1);
                let value: Value = live_table.get(key.clone())?;
                (key.clone(), value).into_lua_multi(lua)
            })?;
        (iterator, Value::Nil, Value::Nil).into_lua_multi(lua)
    })?;

    let globals = lua.globals();
    globals.set("next", det_next)?;
    globals.set("pairs", det_pairs)?;
    Ok(())
}

/// `table`'s entries in a fixed order: `false` before `true`, then numeric
/// keys by value, then string keys by byte content. Errors if any key is
/// of a type without a meaningful deterministic order to assign it (table,
/// function, thread, userdata) — see [`install_deterministic_table_iteration`].
fn sorted_table_entries(table: &Table) -> mlua::Result<Vec<(Value, Value)>> {
    let mut entries: Vec<(Value, Value)> =
        table.pairs::<Value, Value>().collect::<mlua::Result<_>>()?;
    for (key, _) in &entries {
        if !matches!(
            key,
            Value::Boolean(_) | Value::Integer(_) | Value::Number(_) | Value::String(_)
        ) {
            return Err(mlua::Error::RuntimeError(format!(
                "table keys of type '{}' cannot be iterated deterministically",
                key.type_name()
            )));
        }
    }
    entries.sort_by(|(a, _), (b, _)| compare_lua_keys(a, b));
    Ok(entries)
}

fn compare_lua_keys(a: &Value, b: &Value) -> std::cmp::Ordering {
    fn rank(value: &Value) -> u8 {
        match value {
            Value::Boolean(_) => 0,
            Value::Integer(_) | Value::Number(_) => 1,
            Value::String(_) => 2,
            _ => 3,
        }
    }

    let (rank_a, rank_b) = (rank(a), rank(b));
    if rank_a != rank_b {
        return rank_a.cmp(&rank_b);
    }
    match (a, b) {
        (Value::Boolean(x), Value::Boolean(y)) => x.cmp(y),
        (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => x.total_cmp(y),
        (Value::Integer(x), Value::Number(y)) => (*x as f64).total_cmp(y),
        (Value::Number(x), Value::Integer(y)) => x.total_cmp(&(*y as f64)),
        (Value::String(x), Value::String(y)) => x.as_bytes().cmp(&y.as_bytes()),
        _ => std::cmp::Ordering::Equal,
    }
}

/// A failure at the Lua controller boundary. Every variant is returned
/// without the simulation having advanced or mutated as a result of the
/// failing tick.
#[derive(Debug)]
pub enum ControllerError {
    /// The script file could not be read.
    ScriptUnreadable { path: PathBuf, source: io::Error },
    /// The script's Lua source failed to load (e.g. a syntax error).
    ScriptInvalid(mlua::Error),
    /// The script did not define a global `on_tick` function.
    MissingCallback,
    /// `on_tick` raised a Lua error while running.
    CallbackFailed(mlua::Error),
    /// `on_tick` returned a value that is not a valid action.
    InvalidAction(String),
}

impl fmt::Display for ControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControllerError::ScriptUnreadable { path, source } => {
                write!(f, "could not read script '{}': {source}", path.display())
            }
            ControllerError::ScriptInvalid(err) => {
                write!(f, "script failed to load: {err}")
            }
            ControllerError::MissingCallback => {
                write!(
                    f,
                    "script must define a global '{ON_TICK}(observation)' callback"
                )
            }
            ControllerError::CallbackFailed(err) => {
                write!(f, "'{ON_TICK}' raised an error: {err}")
            }
            ControllerError::InvalidAction(detail) => {
                write!(f, "'{ON_TICK}' returned an invalid action: {detail}")
            }
        }
    }
}

impl Error for ControllerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ControllerError::ScriptUnreadable { source, .. } => Some(source),
            ControllerError::ScriptInvalid(err) | ControllerError::CallbackFailed(err) => Some(err),
            ControllerError::MissingCallback | ControllerError::InvalidAction(_) => None,
        }
    }
}

/// A record of one completed tick, handed to the caller's observer so it
/// can render telemetry without this module knowing anything about
/// presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickRecord {
    pub tick: u32,
    pub drone_position: Position,
    pub action: Action,
    pub budget_remaining: u32,
    pub outcome: TickOutcome,
    pub events: Vec<SimEvent>,
    pub map_width: i32,
    pub map_height: i32,
    pub discovered: Vec<DiscoveredTile>,
}

/// Loads `script_path`, then drives a fresh [`Simulation`] to completion by
/// calling its `on_tick` callback once per tick until the operation
/// succeeds or fails. After each completed tick, `observer` (the Rust-side
/// caller's hook, not the Lua callback) is invoked with a [`TickRecord`]
/// describing what happened, so a caller can render live telemetry without
/// this module knowing anything about presentation.
pub fn run(
    script_path: &Path,
    mut observer: impl FnMut(TickRecord),
) -> Result<TickOutcome, ControllerError> {
    let source =
        fs::read_to_string(script_path).map_err(|source| ControllerError::ScriptUnreadable {
            path: script_path.to_path_buf(),
            source,
        })?;

    let lua = sandboxed_lua();
    load_controller(&lua, &source)?;

    let callback: Function = lua
        .globals()
        .get(ON_TICK)
        .map_err(|_| ControllerError::MissingCallback)?;

    let mut simulation = Simulation::new();

    loop {
        let observation_table = observation_to_table(&lua, simulation.observe())
            .map_err(ControllerError::ScriptInvalid)?;

        let response: String = callback
            .call(observation_table)
            .map_err(ControllerError::CallbackFailed)?;

        let action = parse_action(&response)?;

        let report = simulation
            .step(action)
            .map_err(|err| invalid_action_error(&response, err))?;

        let obs = simulation.observe();
        let map = simulation.map();
        observer(TickRecord {
            tick: obs.tick,
            drone_position: obs.drone_position,
            action,
            budget_remaining: obs.budget_remaining,
            outcome: report.outcome,
            events: report.events,
            map_width: map.width(),
            map_height: map.height(),
            discovered: obs.discovered,
        });

        if report.outcome != TickOutcome::Running {
            return Ok(report.outcome);
        }
    }
}

/// Loads `source` into `lua` and confirms it exposes the required
/// `on_tick` callback, without invoking it. Shared by [`run`] and
/// [`validate`] so the console's Controller view can check whether a
/// player's edited source is loadable Lua before anything ever tries to
/// deploy or execute it.
fn load_controller(lua: &Lua, source: &str) -> Result<(), ControllerError> {
    lua.load(source)
        .set_name("controller.lua")
        .exec()
        .map_err(ControllerError::ScriptInvalid)?;

    lua.globals()
        .get::<Function>(ON_TICK)
        .map(|_| ())
        .map_err(|_| ControllerError::MissingCallback)
}

/// The number of Lua VM instructions [`validate`] allows the player's
/// top-level source to execute before treating it as runaway. Valid
/// controllers only define functions and a little local state at load
/// time, so this is generous for legitimate scripts while still bounding an
/// accidental `while true do end` to a short, recoverable pause instead of
/// hanging the console. See `docs/TUI_DESIGN.md`, "Runaway Lua and
/// responsiveness".
const VALIDATE_INSTRUCTION_BUDGET: u32 = 2_000_000;

/// The message used for both the instruction-count hook (a fast, common-case
/// exit) and the thread-timeout backstop (the actual guarantee — see
/// [`validate`]) when a controller's top-level source is treated as
/// runaway.
const EXECUTION_ALLOWANCE_MESSAGE: &str =
    "controller exceeded its execution allowance while loading";

/// An upper bound on how long [`validate`] will wait for the player's
/// top-level source to finish loading before giving up on it. This, not the
/// instruction-count hook, is what actually guarantees the console stays
/// responsive: a hook's error is an ordinary Lua error, so player source
/// that wraps its own infinite loop in `pcall` can catch and keep
/// re-triggering it forever, and native-library work (e.g.
/// `string.rep("x", 1 << 30)`) can block for a long time between
/// instruction-hook checkpoints entirely. Neither of those can defeat a
/// wall-clock timeout enforced from outside the Lua VM.
const VALIDATE_TIMEOUT: Duration = Duration::from_millis(500);

/// What a background [`validate`] thread reports back, kept `Send` (unlike
/// `ControllerError`/`mlua::Error`, which aren't by default) so it can cross
/// an `mpsc` channel.
enum ValidationOutcome {
    Ok,
    MissingCallback,
    Invalid(String),
}

/// An upper bound on how many `validate` background threads can be
/// outstanding at once. In production there's only ever one caller — the
/// console's single-threaded event loop — calling `validate` synchronously,
/// so concurrent callers never legitimately happen there; this cap exists
/// so that a player repeatedly pressing `Ctrl+Enter`/`Ctrl+V` against a
/// script that caught the instruction-hook error with `pcall` (see
/// [`VALIDATE_TIMEOUT`]) leaves at most this many permanently-abandoned
/// threads behind, not one per press. Comfortably larger than the number of
/// tests in this crate that call `validate` concurrently, so ordinary test
/// parallelism (which the single production caller never exhibits) doesn't
/// trip it.
const MAX_CONCURRENT_VALIDATIONS: usize = 16;

/// How many `validate` background threads are currently outstanding. See
/// [`MAX_CONCURRENT_VALIDATIONS`].
static VALIDATIONS_IN_PROGRESS: AtomicUsize = AtomicUsize::new(0);

/// Checks whether `source` is loadable Lua that defines the required
/// `on_tick` callback, without calling `on_tick` itself. The top-level
/// chunk *does* execute (e.g. local state setup, or an `error()` call
/// outside any function), the same as it would in [`run`] — only
/// `on_tick` is never invoked. Used by the console's Controller view to
/// validate/prepare a controller for deployment ahead of time; running the
/// operation itself is a separate step (see [`run`]).
///
/// Only the top-level load is bounded here (not `on_tick` itself, which
/// isn't called): bounding a live deployment's per-tick execution is #45's
/// concern, and applying the same hook to [`run`]'s shared `Lua` would risk
/// tripping on an ordinary multi-tick operation's cumulative instruction
/// count.
///
/// Runs the load on a background thread and never waits longer than
/// [`VALIDATE_TIMEOUT`] for it, so the caller (the console's synchronous
/// event loop) always gets an answer promptly regardless of what the
/// player's source actually does. A thread that doesn't finish in time is
/// abandoned rather than force-killed — Rust has no safe way to do that —
/// but the sandbox's stripped standard-library set and memory ceiling
/// (`sandboxed_lua`) keep an abandoned thread's worst case bounded, and
/// [`VALIDATIONS_IN_PROGRESS`] caps the number of such threads at
/// [`MAX_CONCURRENT_VALIDATIONS`] rather than letting repeated validation
/// attempts against the same runaway script accumulate without limit.
pub fn validate(source: &str) -> Result<(), ControllerError> {
    if VALIDATIONS_IN_PROGRESS
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            (count < MAX_CONCURRENT_VALIDATIONS).then_some(count + 1)
        })
        .is_err()
    {
        return Err(ControllerError::ScriptInvalid(mlua::Error::RuntimeError(
            "too many validations are still finishing; try again in a moment".to_string(),
        )));
    }

    let source = source.to_string();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let lua = sandboxed_lua();
        let _ = lua.set_hook(
            HookTriggers {
                every_nth_instruction: Some(VALIDATE_INSTRUCTION_BUDGET),
                ..HookTriggers::default()
            },
            |_, _| -> mlua::Result<VmState> {
                Err(mlua::Error::RuntimeError(
                    EXECUTION_ALLOWANCE_MESSAGE.to_string(),
                ))
            },
        );
        let outcome = match load_controller(&lua, &source) {
            Ok(()) => ValidationOutcome::Ok,
            Err(ControllerError::MissingCallback) => ValidationOutcome::MissingCallback,
            Err(ControllerError::ScriptInvalid(err)) => ValidationOutcome::Invalid(err.to_string()),
            Err(_) => ValidationOutcome::Invalid(
                "controller failed to load for an unexpected reason".to_string(),
            ),
        };
        // Dropping `lua` runs any pending Lua finalizers — a table with a
        // `__gc` metamethod is collectible source-controlled Lua just like
        // anything else `load_controller` ran, and one wrapping its own
        // `pcall`-protected infinite loop can hang exactly like the
        // top-level-script case `VALIDATE_TIMEOUT`/the instruction hook
        // exist for. Drop it, and only *then* free this thread's
        // concurrency-cap slot, so a hung teardown still counts against
        // `MAX_CONCURRENT_VALIDATIONS` instead of quietly making room for
        // another thread while this one keeps running forever.
        drop(lua);
        VALIDATIONS_IN_PROGRESS.fetch_sub(1, Ordering::SeqCst);
        // If the receiver already timed out and dropped, there's nowhere
        // left to report this; the thread just exits.
        let _ = tx.send(outcome);
    });

    match rx.recv_timeout(VALIDATE_TIMEOUT) {
        Ok(ValidationOutcome::Ok) => Ok(()),
        Ok(ValidationOutcome::MissingCallback) => Err(ControllerError::MissingCallback),
        Ok(ValidationOutcome::Invalid(message)) => Err(ControllerError::ScriptInvalid(
            mlua::Error::RuntimeError(message),
        )),
        Err(_) => Err(ControllerError::ScriptInvalid(mlua::Error::RuntimeError(
            EXECUTION_ALLOWANCE_MESSAGE.to_string(),
        ))),
    }
}

fn observation_to_table(lua: &Lua, observation: Observation) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let drone = lua.create_table()?;
    drone.set("x", observation.drone_position.x)?;
    drone.set("y", observation.drone_position.y)?;
    table.set("drone", drone)?;

    table.set("tick", observation.tick)?;
    table.set("budget_remaining", observation.budget_remaining)?;

    let discovered = lua.create_table()?;
    for (index, tile) in observation.discovered.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set("x", tile.position.x)?;
        entry.set("y", tile.position.y)?;
        entry.set("tile", tile_kind_name(tile.kind))?;
        entry.set("traversable", tile.is_traversable)?;
        entry.set("uplink", tile.is_uplink)?;
        discovered.set(index + 1, entry)?;
    }
    table.set("discovered", discovered)?;

    Ok(table)
}

fn tile_kind_name(kind: TileKind) -> &'static str {
    match kind {
        TileKind::Floor => "floor",
        TileKind::Wall => "wall",
        TileKind::Hazard => "hazard",
    }
}

fn parse_action(name: &str) -> Result<Action, ControllerError> {
    match name {
        "north" => Ok(Action::MoveNorth),
        "south" => Ok(Action::MoveSouth),
        "east" => Ok(Action::MoveEast),
        "west" => Ok(Action::MoveWest),
        "wait" => Ok(Action::Wait),
        "scan" => Ok(Action::Scan),
        other => Err(ControllerError::InvalidAction(format!(
            "'{other}' is not one of north, south, east, west, wait, scan"
        ))),
    }
}

fn invalid_action_error(response: &str, err: ActionError) -> ControllerError {
    ControllerError::InvalidAction(format!("action '{response}' was rejected: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_accepts_documented_names() {
        assert_eq!(parse_action("north").unwrap(), Action::MoveNorth);
        assert_eq!(parse_action("south").unwrap(), Action::MoveSouth);
        assert_eq!(parse_action("east").unwrap(), Action::MoveEast);
        assert_eq!(parse_action("west").unwrap(), Action::MoveWest);
        assert_eq!(parse_action("wait").unwrap(), Action::Wait);
        assert_eq!(parse_action("scan").unwrap(), Action::Scan);
    }

    #[test]
    fn parse_action_rejects_unknown_names() {
        let err = parse_action("north-east").unwrap_err();
        assert!(matches!(err, ControllerError::InvalidAction(_)));
        assert!(err.to_string().contains("north-east"));
    }

    #[test]
    fn missing_callback_error_names_the_callback() {
        assert_eq!(
            ControllerError::MissingCallback.to_string(),
            "script must define a global 'on_tick(observation)' callback"
        );
    }

    #[test]
    fn validate_accepts_a_script_defining_on_tick() {
        assert!(validate("function on_tick(observation) return \"wait\" end").is_ok());
    }

    #[test]
    fn validate_rejects_a_syntax_error() {
        let err = validate("function on_tick( ").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn validate_rejects_a_script_missing_on_tick() {
        let err = validate("local x = 1").unwrap_err();
        assert!(matches!(err, ControllerError::MissingCallback));
    }

    #[test]
    fn validate_does_not_execute_on_tick() {
        // If this ran on_tick, `error(...)` would surface as CallbackFailed
        // instead of validate succeeding; validate must only load the
        // script and check the callback exists.
        assert!(validate("function on_tick(observation) error('should not run') end").is_ok());
    }

    #[test]
    fn validate_bounds_a_runaway_top_level_loop_instead_of_hanging() {
        let err = validate("while true do end").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
        assert!(err.to_string().contains("execution allowance"));
    }

    // The pcall-wrapped-instruction-hook-bypass regression test lives in its
    // own integration test binary
    // (`tests/lua_controller_execution_limit.rs`), not here: that script
    // genuinely never lets its background thread finish, which permanently
    // holds `VALIDATION_IN_PROGRESS`'s single slot for the rest of whatever
    // process runs it — harmless in a dedicated process with no other
    // `validate` calls after it, but it would spuriously fail every other
    // test in this file (and every other unit test in the crate, since
    // `cargo test`'s unit tests all share one binary/process) that happens
    // to run afterward.

    #[test]
    fn validate_rejects_dofile_and_loadfile() {
        for source in ["dofile('/etc/passwd')", "loadfile('/etc/passwd')"] {
            let err = validate(source).unwrap_err();
            assert!(
                matches!(err, ControllerError::ScriptInvalid(_)),
                "{source} should fail without filesystem access"
            );
        }
    }

    #[test]
    fn validate_rejects_a_script_that_reseeds_math_random_itself() {
        // math.randomseed() with no arguments would otherwise re-seed from
        // wall-clock time in real Lua 5.4, undoing the fixed seed
        // sandboxed_lua() installs; removing the function entirely means
        // any use of it (with or without arguments) fails to load instead
        // of silently reintroducing nondeterminism.
        let err = validate("math.randomseed()").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn pairs_iterates_a_string_keyed_table_in_a_fixed_order_across_fresh_states() {
        // Each `validate` call gets its own fresh sandboxed Lua, so this
        // exercises exactly the scenario a hash-seed-randomized `pairs`
        // would make nondeterministic: iterating the same string-keyed
        // table built fresh each time.
        let source = r#"
            local order = {}
            for k in pairs({north = 1, south = 2, east = 3, west = 4, wait = 5}) do
                order[#order + 1] = k
            end
            assert(table.concat(order, ",") == "east,north,south,wait,west",
                   table.concat(order, ","))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn pairs_orders_boolean_keys_deterministically() {
        let source = r#"
            local order = {}
            for k in pairs({[true] = 1, [false] = 2}) do
                order[#order + 1] = tostring(k)
            end
            assert(table.concat(order, ",") == "false,true", table.concat(order, ","))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn pairs_rejects_a_table_valued_key_instead_of_leaving_it_nondeterministic() {
        let err = validate("for k in pairs({[{}] = 1}) do end").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn pairs_survives_clearing_the_current_field_during_iteration() {
        // A common idiom for clearing a table; the naive "search for the
        // previous key in a freshly recomputed entry list" implementation
        // silently stops after the first deletion because the deleted key
        // is no longer present to search for.
        let source = r#"
            local t = {a = 1, b = 2, c = 3}
            local seen = 0
            for k in pairs(t) do
                t[k] = nil
                seen = seen + 1
            end
            assert(seen == 3, seen)
            assert(next(t) == nil)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn standalone_next_survives_clearing_the_current_field_during_manual_traversal() {
        // Same idiom as above but driven by hand via `next` directly,
        // rather than a `for ... in pairs(t)` loop (which now uses its own
        // position-based iterator and wouldn't exercise `next`'s own fix).
        let source = r#"
            local t = {a = 1, b = 2, c = 3}
            local seen = 0
            local k = next(t)
            while k ~= nil do
                local current = k
                k = next(t, k)
                t[current] = nil
                seen = seen + 1
            end
            assert(seen == 3, seen)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn pairs_sees_a_live_update_to_a_not_yet_visited_field() {
        let source = r#"
            local t = {a = 1, b = 2}
            local seen_b = nil
            for k, v in pairs(t) do
                if k == "a" then
                    t.b = 99
                elseif k == "b" then
                    seen_b = v
                end
            end
            assert(seen_b == 99, seen_b)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn a_table_with_pairs_metamethod_still_iterates_through_it() {
        // Real Lua 5.4 defers entirely to `__pairs` when a table has one;
        // the deterministic override must do the same, not silently
        // replace a proxy table's own iteration with the underlying raw
        // table's entries.
        let source = r#"
            local proxy = setmetatable({}, {
                __pairs = function(self)
                    local done = false
                    return function()
                        if done then return nil end
                        done = true
                        return "only", "value"
                    end, self, nil
                end,
            })
            local seen = {}
            for k, v in pairs(proxy) do
                seen[#seen + 1] = k .. "=" .. v
            end
            assert(#seen == 1 and seen[1] == "only=value", table.concat(seen, ","))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn tostring_hides_the_process_address_of_a_table() {
        let lua_source = "local text = tostring({})\n\
                           assert(text == 'table: 0x0', text)\n\
                           function on_tick(observation) return 'wait' end";
        assert!(validate(lua_source).is_ok());
    }

    #[test]
    fn tostring_still_respects_a_custom_tostring_metamethod() {
        let source = r#"
            local labeled = setmetatable({}, {__tostring = function(_) return "named" end})
            assert(tostring(labeled) == "named", tostring(labeled))
            assert(tostring(42) == "42")
            assert(tostring(true) == "true")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn tostring_preserves_a_non_utf8_lua_string_unchanged() {
        let source = r#"
            local raw = string.char(255)
            assert(tostring(raw) == raw)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn validate_rejects_a_script_that_calls_load() {
        let err = validate("load('return 1')").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn string_format_rejects_a_pointer_specifier() {
        for source in [
            "string.format('%p', {})",
            "string.format('addr=%p!', {})",
            "('%p'):format({})",
            "string.format('%-10p', print)",
        ] {
            let err = validate(source).unwrap_err();
            assert!(
                matches!(err, ControllerError::ScriptInvalid(_)),
                "{source} should be rejected"
            );
        }
    }

    #[test]
    fn string_format_still_works_for_an_escaped_percent_followed_by_p() {
        let source = r#"
            assert(string.format("%%p") == "%p")
            assert(string.format("%d", 42) == "42")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn string_format_hides_the_process_address_of_a_table_via_percent_s() {
        let source = r#"
            local text = string.format("%s", {})
            assert(text == "table: 0x0", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn string_format_still_respects_a_custom_tostring_metamethod_via_percent_s() {
        let source = r#"
            local labeled = setmetatable({}, {__tostring = function(_) return "named" end})
            assert(string.format("%s", labeled) == "named")
            assert(string.format("%s and %d", "x", 3) == "x and 3")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn string_format_accepts_a_non_utf8_format_string() {
        let source = r#"
            local fmt = string.char(255) .. "%d"
            local text = string.format(fmt, 7)
            assert(text == string.char(255) .. "7", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate(source).is_ok());
    }

    #[test]
    fn validate_bounds_excessive_native_allocation() {
        let err = validate("local n = 1 << 30\nlocal s = string.rep('x', n)").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn validate_rejects_scripts_that_reach_for_host_capabilities() {
        for source in [
            "os.execute('true')",
            "io.open('/etc/passwd')",
            "require('os')",
        ] {
            let err = validate(source).unwrap_err();
            assert!(
                matches!(err, ControllerError::ScriptInvalid(_)),
                "{source} should fail to load without host library access"
            );
        }
    }
}
