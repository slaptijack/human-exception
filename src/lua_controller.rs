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

use std::cell::Cell;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// regardless of which `StdLib` flags are requested), `string.pack`/
/// `unpack`/`packsize` also removed, and a bounded memory ceiling. Player
/// Lua is untrusted input (AGENTS.md, "Treat Lua programs as untrusted
/// input"); nothing in the on_tick contract needs filesystem, process,
/// module-loading, or dynamic-code-loading access. `print` writes straight
/// to process stdout — including raw escape sequences — which would
/// corrupt the alternate screen ratatui owns while the console is running.
/// `load` accepts Lua 5.4 *binary* chunks as well as text by default;
/// unlike text source (which the parser fully validates), malformed
/// bytecode is trusted structurally and can crash the process outright
/// rather than fail as a recoverable Lua error — and the string library
/// alone is enough to assemble arbitrary bytes for one. Nothing in the
/// on_tick contract needs to load code dynamically at all, so removing
/// `load` avoids needing to reason about restricting it to text-only mode.
/// `string.pack`/`unpack`/`packsize` expose native platform layout
/// (endianness, and implementation-defined widths like `size_t` via the
/// `T`/`s`/`j`/`J` format options) unless every format string an
/// invocation ever uses pins both explicitly — the same source could
/// behave differently across architectures with no per-process
/// randomness involved at all, just a different build target. Nothing in
/// the on_tick contract needs binary packing, so removing them avoids
/// needing to validate every format string that reaches them instead.
/// `string.dump` is removed for the same reason: it serializes a Lua
/// function to platform-specific bytecode, leaking the same kind of
/// build-dependent native layout information. `collectgarbage` is removed
/// too — beyond manual GC control nothing in the on_tick contract needs,
/// its `"count"` query reports the Lua state's live memory in a way that
/// differs between [`validate`]'s hooked state and [`run`]'s unhooked one,
/// letting a script detect which one it's executing under and behave
/// differently — the same "same source, same behavior" guarantee the rest
/// of this sandbox exists to uphold.
fn sandboxed_lua() -> Lua {
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH;
    let lua = Lua::new_with(libs, LuaOptions::default())
        .expect("sandboxed stdlib set excludes debug/ffi, so this cannot fail");
    let globals = lua.globals();
    let _ = globals.set("dofile", Value::Nil);
    let _ = globals.set("loadfile", Value::Nil);
    let _ = globals.set("load", Value::Nil);
    let _ = globals.set("print", Value::Nil);
    let _ = globals.set("collectgarbage", Value::Nil);
    if let Ok(string_lib) = globals.get::<Table>("string") {
        let _ = string_lib.set("pack", Value::Nil);
        let _ = string_lib.set("unpack", Value::Nil);
        let _ = string_lib.set("packsize", Value::Nil);
        let _ = string_lib.set("dump", Value::Nil);
    }
    let _ = lua.set_memory_limit(SANDBOX_MEMORY_LIMIT_BYTES);
    // Stops Lua's automatic incremental garbage collection entirely for the
    // rest of this state's lifetime (never restarted — nothing in this
    // module calls `gc_restart`/`gc_collect`). Without this, an unreachable
    // table with a `__gc` finalizer can be collected — and that finalizer
    // run — at a point during top-level execution that depends on the
    // state's allocation pacing, which [`validate`]'s instruction hook (an
    // extra allocation source `run` doesn't have) shifts just enough to
    // make the *same* source's automatic-collection timing differ between
    // the two: a finalizer that defines `on_tick` could run before
    // `load_controller` finishes in one but not the other, reporting
    // `READY`/`MissingCallback` inconsistently with no memory-introspection
    // API involved at all (`collectgarbage` is already removed above).
    // Stopping automatic collection in both means neither ever runs a
    // finalizer *during* the top-level chunk purely because of incidental
    // allocation timing; `lua_close` (this `Lua`'s `Drop`) still finalizes
    // everything still unreachable once the state itself goes away, same
    // as always. The 32MB memory limit set just above is the sole backstop
    // against uncollected garbage for the rest of a state's lifetime, which
    // is already sufficient for the on_tick contract's "trivial amount of
    // state" (see `SANDBOX_MEMORY_LIMIT_BYTES`'s doc comment).
    lua.gc_stop();
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
            let Value::String(fmt) = &fmt_value else {
                // Not a string at all; let `real_format` raise its own
                // type error rather than second-guessing it here.
                let mut rebuilt = MultiValue::new();
                rebuilt.push_back(fmt_value.clone());
                for value in iter {
                    rebuilt.push_back(value);
                }
                return real_format.call(rebuilt);
            };
            let conversions = format_argument_conversions(&fmt.as_bytes());
            if conversions.contains(&b'p') {
                return Err(mlua::Error::RuntimeError(
                    "string.format: '%p' is not available (it would expose a process memory \
                 address, which is nondeterministic)"
                        .to_string(),
                ));
            }
            // `%s` formats a value via Lua's own C-level `tolstring`,
            // which respects a value's `__tostring` metamethod but not
            // our *overridden* global `tostring` — so a table or function
            // argument without one would still get the real, nondeterministic
            // "type: 0xADDRESS" text straight from `%s`, independent of the
            // `%p` case above. Route only the arguments a `%s` conversion
            // will actually consume through the deterministic `tostring`
            // first, matched up positionally with `conversions`; unlike
            // `%s`, `%q` has no default-representation fallback of its own
            // at all (real Lua rejects a table/function argument to `%q`
            // outright, since it has no literal form), so leaving those
            // arguments untouched preserves that rejection instead of
            // silently succeeding with a normalized placeholder.
            //
            // A table with its own `__tostring` is deliberately *not*
            // converted here, even though it's a reference type a `%s`
            // conversion will consume: real Lua's own conversion loop
            // processes each specifier strictly left to right and stops at
            // the first one that fails (e.g. `%d` given a non-number), so
            // a custom `__tostring` on a *later* `%s` argument is never
            // even called if an earlier conversion errors first. Converting
            // it here, in a separate pass before `real_format` runs at
            // all, would call that (potentially side-effecting, e.g. a
            // counter, or one that itself raises) metamethod regardless of
            // whether real Lua would ever have reached it — an observable
            // divergence beyond just the formatted text. Leaving it
            // untouched instead lets `real_format` call it itself, exactly
            // where and only if real Lua's own sequential processing would.
            // A reference-typed value *without* a custom `__tostring` has
            // no such script-visible call to protect: our replacement text
            // is a fixed placeholder with no side effects and always
            // succeeds, so converting it eagerly here cannot change
            // whether, or where, `real_format` fails.
            let mut rebuilt = MultiValue::new();
            rebuilt.push_back(fmt_value.clone());
            for (index, value) in iter.enumerate() {
                let value = if conversions.get(index) == Some(&b's')
                    && matches!(
                        value,
                        Value::Table(_)
                            | Value::Function(_)
                            | Value::Thread(_)
                            | Value::UserData(_)
                    )
                    && !table_has_custom_tostring(&value)
                {
                    Value::String(det_tostring.call::<mlua::LuaString>(value)?)
                } else {
                    value
                };
                rebuilt.push_back(value);
            }
            real_format.call(rebuilt)
        },
    )?;
    // `real_format` itself can raise a plain string error (e.g. a bad
    // argument type), and a `%s` conversion can invoke a value's own
    // custom `__tostring` (unrestricted script code, free to raise
    // anything) — see `wrap_lua_errors_as_strings` for why either error
    // would otherwise cross `det_format`'s own Rust closure boundary as
    // userdata instead of the plain string real Lua would raise.
    let det_format = wrap_lua_errors_as_strings(lua, det_format)?;
    string_lib.set("format", det_format)?;
    Ok(())
}

/// The conversion character (`s`, `d`, `q`, `p`, ...) for each
/// argument-consuming specifier in `fmt`, in order — an escaped `%%`
/// consumes no argument and contributes nothing to this list. Handles the
/// usual printf-style flag/width/precision characters between `%` and the
/// conversion letter (e.g. `%-10s`). Operates on raw bytes, not `&str`,
/// since Lua strings (including format strings) are arbitrary byte
/// sequences, not necessarily valid UTF-8.
fn format_argument_conversions(fmt: &[u8]) -> Vec<u8> {
    let mut conversions = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        if fmt[i] == b'%' {
            i += 1;
            continue;
        }
        while i < fmt.len() && matches!(fmt[i], b'-' | b'+' | b' ' | b'#' | b'0'..=b'9' | b'.') {
            i += 1;
        }
        if i < fmt.len() {
            conversions.push(fmt[i]);
            i += 1;
        }
    }
    conversions
}

/// Wraps `implementation` (a Rust-implemented `Function`) so that any error
/// it raises reaches a caller's own `pcall` as a genuine Lua string,
/// instead of the userdata `mlua` always wraps a Rust callback's `Err` in
/// (confirmed via `mlua`'s own source: `callback_error` unconditionally
/// wraps whatever a `create_function` closure returns as `Err` in a
/// `WrappedFailure` userdata before raising it — this is not specific to
/// any one override here). The wrapper itself is a genuine Lua closure
/// (built by evaluating a small Lua chunk and calling the factory it
/// returns with `implementation`), so calling the *returned* function
/// never crosses a Rust closure boundary on the way out — only
/// internally, via its own `pcall`, where the caught (possibly userdata,
/// possibly already a plain value) error is converted with `tostring` and
/// re-`error`ed at level 0 (no position prefix added, since the original
/// message already has whatever `error`/`assert` text it needs).
///
/// This does not preserve a raised *value*'s original type — a table
/// passed to `error()` inside a custom callback becomes its `tostring()`
/// text here, not the table itself, unlike a raise that never crosses any
/// Rust boundary at all in real Lua. Doing better than that would need
/// `pairs`/`tostring`/`string.format` (and everything they can call into)
/// to never touch a Rust closure anywhere on their call path, which is a
/// materially bigger restructuring than this fixes; every error this
/// module raises directly is already a plain string, so the only real
/// loss is for a script's own callback (`__pairs`, `__tostring`) that
/// raises a non-string value on purpose.
fn wrap_lua_errors_as_strings(lua: &Lua, implementation: Function) -> mlua::Result<Function> {
    lua.load(
        r#"
        return function(implementation)
            return function(...)
                local results = table.pack(pcall(implementation, ...))
                if not results[1] then
                    error(tostring(results[2]), 0)
                end
                return table.unpack(results, 2, results.n)
            end
        end
    "#,
    )
    .set_name("lua_error_string_unwrap_factory")
    .eval::<Function>()?
    .call(implementation)
}

/// Whether `value` is a table carrying its own `__tostring` metamethod.
/// Lua's standard `setmetatable` only ever attaches to tables, so a table
/// is the only reference type that can have one; a table that does
/// produces its own script-controlled `tostring`/`%s` output, which must be
/// left completely untouched (both what it says and *when* it's called)
/// rather than treated as address-bearing default text to normalize.
fn table_has_custom_tostring(value: &Value) -> bool {
    let Value::Table(table) = value else {
        return false;
    };
    table
        .metatable()
        .map(|metatable| {
            !matches!(
                metatable.raw_get::<Value>("__tostring"),
                Ok(Value::Nil) | Err(_)
            )
        })
        .unwrap_or(false)
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
    let det_tostring = lua.create_function(
        move |lua, args: MultiValue| -> mlua::Result<mlua::LuaString> {
            // A single `Value` parameter can't distinguish "no argument at
            // all" from an explicit `nil` — `mlua` pads a missing argument
            // to a fixed-arity closure with `Nil` the same way Lua itself
            // pads a missing parameter in an ordinary function call. Real
            // Lua's `tostring` is a C function that explicitly checks for
            // at least one argument (`luaL_checkany`) and raises "bad
            // argument #1 to 'tostring' (value expected)" for zero
            // arguments, while `tostring(nil)` (one explicit argument)
            // succeeds and returns `"nil"` — accepting `MultiValue` instead
            // keeps that distinction, which matters to controller code that
            // probes or forwards a call by argument count.
            let Some(value) = args.into_iter().next() else {
                return Err(mlua::Error::RuntimeError(
                    "bad argument #1 to 'tostring' (value expected)".to_string(),
                ));
            };
            // Only a table/function/thread/userdata's *default* (no
            // `__tostring`) representation is address-bearing; gate on the
            // input value's type, not just the output text, so an ordinary
            // string that merely *looks* like one (`tostring("table:
            // 0xabc")`, a string literal, must come back unchanged) is
            // never mistaken for it.
            let default_type_name = match &value {
                Value::Table(_) => Some("table"),
                Value::Function(_) => Some("function"),
                Value::Thread(_) => Some("thread"),
                Value::UserData(_) => Some("userdata"),
                _ => None,
            };
            let Some(default_type_name) = default_type_name else {
                return real_tostring.call(value);
            };
            // Lua's standard `setmetatable` only ever attaches to tables, so
            // a table is the only reference type here that can carry a
            // custom `__tostring` — one that has it produces its own,
            // script-controlled output that must pass through completely
            // untouched, calling real Lua's `tostring` exactly where/when
            // it otherwise would.
            if table_has_custom_tostring(&value) {
                return real_tostring.call(value);
            }
            // Every other reference-typed value's default representation
            // bakes in this value's real process memory address — do not
            // call real Lua's `tostring` for it at all, not even to
            // normalize its output afterward. An earlier version of this
            // override called through to real `tostring` and searched its
            // text for a `": 0x"` marker to replace, which assumed Lua's
            // C-level default representation always uses glibc/POSIX
            // `printf("%p")` spelling; on MSVC targets `%p` instead
            // produces fixed-width hexadecimal text with no `0x` prefix at
            // all, so that search silently found nothing and returned the
            // real, unredacted address untouched — defeating this
            // protection entirely on that platform. Building the
            // placeholder text ourselves, directly from the value's type
            // name (or `__name`, the same override Lua's own default
            // representation would use there), never looks at platform-
            // specific address formatting in the first place.
            let name = if let Value::Table(table) = &value {
                table
                    .metatable()
                    .and_then(|metatable| metatable.raw_get::<Value>("__name").ok())
                    .and_then(|name| match name {
                        Value::String(name) => Some(name),
                        _ => None,
                    })
            } else {
                None
            };
            let mut placeholder = match &name {
                Some(name) => name.as_bytes().to_vec(),
                None => default_type_name.as_bytes().to_vec(),
            };
            placeholder.extend_from_slice(b": 0x0");
            lua.create_string(placeholder)
        },
    )?;
    // A table's custom `__tostring` (called just above via `real_tostring`)
    // is unrestricted script code, free to raise — see
    // `wrap_lua_errors_as_strings` for why that error would otherwise
    // cross `det_tostring`'s own Rust closure boundary as userdata instead
    // of the plain string real Lua would raise.
    let det_tostring = wrap_lua_errors_as_strings(lua, det_tostring)?;
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
    // Host-side-only bookkeeping (never enumerated with `pairs`/`next`
    // itself, so raw Lua table-identity hashing here can't leak any
    // nondeterminism to a script) recording every key `det_next` has ever
    // confirmed was a genuine member of a given table: `key_history[table]`
    // is itself a table used purely as a set, `key_history[table][key] =
    // true` once `key` has been seen. Real Lua's `next` raises "invalid
    // key to 'next'" for a control value that was never actually a member
    // of the table (e.g. a typo, or a value fabricated to probe for one),
    // while still tolerating a key that *was* a member but has since been
    // deleted mid-traversal (the Lua manual explicitly allows clearing the
    // current field). `sorted_table_entries` alone can't tell these apart
    // — a "find the first entry greater than key" search succeeds for any
    // key value, member or not — so this records genuine membership as
    // it's observed, closing that gap without needing to intercept every
    // raw table assignment (which nothing in the sandboxed API can do).
    let key_history = lua.create_table()?;
    let raw_next = lua.create_function(
        move |lua, (table, key): (Table, Value)| -> mlua::Result<MultiValue> {
            let entries = sorted_table_entries(&table)?;
            if matches!(key, Value::Nil) {
                return match entries.into_iter().next() {
                    None => (Value::Nil,).into_lua_multi(lua),
                    Some((k, v)) => {
                        remember_next_key(lua, &key_history, &table, &k)?;
                        (k, v).into_lua_multi(lua)
                    }
                };
            }
            let currently_present = !matches!(table.raw_get::<Value>(key.clone())?, Value::Nil);
            if currently_present {
                remember_next_key(lua, &key_history, &table, &key)?;
            } else if !next_key_was_seen(&key_history, &table, &key)? {
                return Err(mlua::Error::RuntimeError(
                    "invalid key to 'next'".to_string(),
                ));
            }
            let found = entries
                .into_iter()
                .find(|(k, _)| compare_lua_keys(k, &key) == std::cmp::Ordering::Greater);
            match found {
                // Lua 5.4's real `next` returns exactly one `nil` once
                // exhausted, not a `nil, nil` pair — `select("#", next({}))`
                // is `1`, and controller code that inspects the return
                // arity to detect the end of a traversal would otherwise
                // see a different arity here than in real Lua.
                None => (Value::Nil,).into_lua_multi(lua),
                Some((k, v)) => {
                    remember_next_key(lua, &key_history, &table, &k)?;
                    (k, v).into_lua_multi(lua)
                }
            }
        },
    )?;
    // Real Lua's `next` raises a genuine string (e.g. "invalid key to
    // 'next'" above), so `local ok, err = pcall(next, t, bogus_key);
    // type(err)` must be `"string"` here too — see
    // `wrap_lua_errors_as_strings`.
    let det_next = wrap_lua_errors_as_strings(lua, raw_next)?;

    // A tiny Lua-level trampoline used to invoke a table's `__pairs` value
    // through Lua's own call operator rather than Rust matching on
    // `Value::Function`. Real Lua allows *any* callable value there — a
    // table or userdata with its own `__call` metamethod counts too — and
    // re-implementing that resolution in Rust would mean re-deriving Lua's
    // whole call-dispatch chain; delegating the actual call back into Lua
    // gets that (and its exact "attempt to call a ... value" error wording
    // for a genuinely uncallable value) for free.
    let call_pairs_metamethod: Function = lua
        .load("return function(fn, tbl) return fn(tbl) end")
        .set_name("pairs_metamethod_dispatch")
        .eval()?;

    // Cloned in before `det_pairs`'s closure captures it below (a `Function`
    // handle is cheap to clone — it doesn't copy the underlying Lua
    // function) so `det_next` is still available to register as `next`
    // itself afterward.
    let det_next_for_pairs = det_next.clone();
    let det_pairs = lua.create_function(move |lua, table: Table| -> mlua::Result<MultiValue> {
        // Real Lua 5.4 defers entirely to a table's `__pairs` metamethod
        // when present (checked before ever calling the real `next`);
        // match that instead of silently overriding a proxy/wrapper
        // table's own iteration behavior with ours. A script's own
        // `__pairs` is responsible for its own determinism, same as a
        // custom `__tostring` is for its own output.
        if let Some(metatable) = table.metatable() {
            // A raw lookup, not `Table::get`: the latter would follow the
            // metatable's own `__index` chain (if it has one) and could
            // pick up an *inherited* `__pairs` a script never actually set
            // on this table's own metatable — real Lua only ever looks at
            // the metatable's raw `__pairs` entry.
            let pairs_field: Value = metatable.raw_get("__pairs")?;
            if !matches!(pairs_field, Value::Nil) {
                // Real Lua 5.4's `pairs` calls `__pairs` requesting exactly
                // three results (`lua_call(L, 1, 3)`): fewer than three
                // returned are padded with `nil` by the VM, and more than
                // three are discarded. Forwarding *however many* the
                // metamethod actually returned instead leaks extra values
                // into the generic `for` loop's own protocol (its 4th slot
                // is the to-be-closed value, so a leaked non-nil 4th value
                // can make an otherwise-valid `for k, v in pairs(t)` fail
                // with "got a non-closable value"), or leaves callers that
                // inspect the result count (`select("#", pairs(t))`)
                // observing a different arity than real Lua's fixed three.
                let mut results: Vec<Value> = call_pairs_metamethod
                    .call::<MultiValue>((pairs_field, table))?
                    .into_iter()
                    .collect();
                results.resize(3, Value::Nil);
                return Ok(MultiValue::from_iter(results));
            }
        }
        // Real Lua's `pairs` (absent a custom `__pairs`) returns `next`
        // itself as the iterator, along with the table as state and `nil`
        // as the initial control value — not a fresh, purpose-built
        // iterator. `next` is already a stateless "give me the entry after
        // `key`" function (see `det_next` above), safe to call with any
        // control value, including a script driving a manual
        // generic-for-style traversal directly (`local f, s, k = pairs(t)`,
        // then calling `f(s, k)` more than once with the same `k`, or out
        // of the order a `for` loop would use) — and its
        // [`sorted_table_entries`]-based lookup already reflects every
        // documented mid-traversal edit (a value changed before it's
        // reached, a key deleted at or before it) for free.
        //
        // This used to be a separate closure that tracked its own position
        // internally instead of honoring the control value it was called
        // with, which broke exactly that contract: two calls with the same
        // control value returned *different* keys, since the second call
        // silently continued from the first's advanced position rather
        // than recomputing "the entry after this specific key" the way
        // real Lua's `next` (and a real generic `for` loop, which always
        // passes back the previous key as the control value) does.
        (det_next_for_pairs.clone(), Value::Table(table), Value::Nil).into_lua_multi(lua)
    })?;

    // Same reasoning as `det_next` above: `det_pairs` calls back into a
    // table's own `__pairs` (unrestricted script code, free to raise
    // anything), and that error would otherwise cross `det_pairs`'s own
    // Rust closure boundary as userdata.
    let det_pairs = wrap_lua_errors_as_strings(lua, det_pairs)?;

    let globals = lua.globals();
    globals.set("next", det_next)?;
    globals.set("pairs", det_pairs)?;
    Ok(())
}

/// Records that `key` has been confirmed a genuine member of `table`, in
/// `key_history` (see [`install_deterministic_table_iteration`]).
fn remember_next_key(
    lua: &Lua,
    key_history: &Table,
    table: &Table,
    key: &Value,
) -> mlua::Result<()> {
    let history: Table = match key_history.get::<Value>(table.clone())? {
        Value::Table(history) => history,
        _ => {
            let history = lua.create_table()?;
            key_history.set(table.clone(), history.clone())?;
            history
        }
    };
    history.set(key.clone(), true)
}

/// Whether `key` has previously been recorded (via [`remember_next_key`])
/// as a genuine member of `table`.
fn next_key_was_seen(key_history: &Table, table: &Table, key: &Value) -> mlua::Result<bool> {
    match key_history.get::<Value>(table.clone())? {
        Value::Table(history) => Ok(!matches!(history.get::<Value>(key.clone())?, Value::Nil)),
        _ => Ok(false),
    }
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

/// Compares an integer key against a float key without the precision loss
/// a plain `i as f64` cast (as either operand) can introduce once the
/// magnitude exceeds what `f64`'s 53-bit mantissa can represent exactly —
/// `i64::MAX` and `9223372036854775808.0` (2^63) are distinct table keys
/// that a naive cast makes compare equal, since `i64::MAX as f64` rounds
/// up to exactly `2^63`. Table keys can never be NaN (Lua rejects that at
/// assignment), so `n` is always a comparable value here.
fn compare_integer_and_number(i: i64, n: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    const TWO_POW_63: f64 = 9223372036854775808.0; // one past i64::MAX
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // exactly representable
    if n >= TWO_POW_63 {
        return Ordering::Less; // every i64 is less than any such n
    }
    if n < I64_MIN_AS_F64 {
        return Ordering::Greater; // every i64 is greater than any such n
    }
    // n now fits within `i64`'s range (with room to spare below i64::MAX,
    // since TWO_POW_63 was excluded above), so floor(n) truncates safely.
    let n_floor = n.floor();
    let n_floor_i = n_floor as i64;
    match i.cmp(&n_floor_i) {
        Ordering::Equal if n > n_floor => Ordering::Less, // n has a fractional part above i
        other => other,
    }
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
        (Value::Integer(x), Value::Number(y)) => compare_integer_and_number(*x, *y),
        (Value::Number(x), Value::Integer(y)) => compare_integer_and_number(*y, *x).reverse(),
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
    /// A single `on_tick` call exceeded its per-tick execution allowance
    /// (see [`TICK_INSTRUCTION_BUDGET`]), most likely because it never
    /// returns. Distinct from [`ControllerError::CallbackFailed`] so a live
    /// deployment can show the player a precise "this is a runaway
    /// controller" diagnostic rather than a generic Lua error.
    ExecutionLimitExceeded,
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
            ControllerError::ExecutionLimitExceeded => {
                write!(
                    f,
                    "'{ON_TICK}' exceeded its execution allowance and was stopped"
                )
            }
        }
    }
}

impl Error for ControllerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ControllerError::ScriptUnreadable { source, .. } => Some(source),
            ControllerError::ScriptInvalid(err) | ControllerError::CallbackFailed(err) => Some(err),
            ControllerError::MissingCallback
            | ControllerError::InvalidAction(_)
            | ControllerError::ExecutionLimitExceeded => None,
        }
    }
}

impl ControllerError {
    /// Whether this failure represents the controller exceeding its
    /// execution allowance, whether reported as
    /// [`ControllerError::ExecutionLimitExceeded`] (a callback caught
    /// mid-tick, which has simulation state to attach a distinct variant
    /// to) or as a [`ControllerError::ScriptInvalid`] carrying
    /// [`EXECUTION_ALLOWANCE_MESSAGE`] (the top-level load path, which has
    /// no simulation state yet — see [`LiveOperation::deploy`] and
    /// [`validate`]). Lets callers present both as the same "runaway
    /// controller" diagnostic instead of the top-level case reading like an
    /// ordinary syntax error.
    pub fn is_execution_limit(&self) -> bool {
        match self {
            ControllerError::ExecutionLimitExceeded => true,
            ControllerError::ScriptInvalid(err) => {
                err.to_string().contains(EXECUTION_ALLOWANCE_MESSAGE)
            }
            _ => false,
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
    let callback = load_controller(&lua, &source)?;

    let mut simulation = Simulation::new();

    loop {
        let record = advance_tick(&lua, &callback, &mut simulation, None)?;
        let outcome = record.outcome;
        observer(record);

        if outcome != TickOutcome::Running {
            return Ok(outcome);
        }
    }
}

/// Runs exactly one tick: builds the observation table for `simulation`'s
/// current state, calls `callback`, validates and applies the returned
/// action, and reports the result as a [`TickRecord`]. Shared by [`run`]
/// (which loops this to completion for the noninteractive CLI path, passing
/// `hook_fired: None` since it installs no execution-limit hook at all) and
/// [`LiveOperation`]'s worker (which runs it once per player-paced tick,
/// passing its per-tick instruction-hook flag), so the two can't drift
/// apart.
///
/// `hook_fired`, when given, is checked immediately after `callback.call`
/// returns and before the resulting action is ever submitted to
/// `simulation.step` — not just on a non-`Ok` result. The hook's own error
/// is an ordinary catchable Lua error: a script that wraps its own
/// budget-exceeding loop in `pcall` and then returns an apparently normal
/// action would otherwise have that action silently applied before anyone
/// noticed the hook had fired mid-call, contradicting every other
/// `ControllerError` path's guarantee that a failing tick never mutates
/// simulation state.
fn advance_tick(
    lua: &Lua,
    callback: &Function,
    simulation: &mut Simulation,
    hook_fired: Option<&Cell<bool>>,
) -> Result<TickRecord, ControllerError> {
    let observation_table =
        observation_to_table(lua, simulation.observe()).map_err(ControllerError::ScriptInvalid)?;

    let call_result: mlua::Result<String> = callback.call(observation_table);

    // Checked against the call's raw result, before `?` can propagate
    // either a success or a failure onward: an uncaught hook error surfaces
    // as an ordinary `Err` here (the same as any other Lua runtime error),
    // but a script that wraps its own budget-exceeding loop in `pcall` can
    // catch it and return an apparently normal action instead — the hook
    // having fired at all must win over whatever the call otherwise
    // produced, in both directions.
    if hook_fired.is_some_and(Cell::get) {
        return Err(ControllerError::ExecutionLimitExceeded);
    }

    let response = call_result.map_err(ControllerError::CallbackFailed)?;

    let action = parse_action(&response)?;

    let report = simulation
        .step(action)
        .map_err(|err| invalid_action_error(&response, err))?;

    let obs = simulation.observe();
    let map = simulation.map();
    Ok(TickRecord {
        tick: obs.tick,
        drone_position: obs.drone_position,
        action,
        budget_remaining: obs.budget_remaining,
        outcome: report.outcome,
        events: report.events,
        map_width: map.width(),
        map_height: map.height(),
        discovered: obs.discovered,
    })
}

/// The number of Lua VM instructions a single tick's `on_tick` call is
/// allowed to execute before [`LiveOperation`]'s worker treats it as
/// runaway. Reinstalled (which resets the underlying instruction countdown)
/// before every tick, so this is a genuine per-call budget, not a
/// cumulative one an ordinary multi-tick operation could exhaust over time.
/// See `docs/TUI_DESIGN.md`, "Runaway Lua and responsiveness".
const TICK_INSTRUCTION_BUDGET: u32 = 2_000_000;

/// An upper bound on how long [`LiveOperation`]'s worker will wait for a
/// single tick's `on_tick` call to finish before treating the deployment as
/// runaway. This — not [`TICK_INSTRUCTION_BUDGET`]'s hook — is the actual
/// backstop: a hook's error is an ordinary catchable Lua error, so
/// `on_tick` code that wraps its own infinite loop in `pcall` inside an
/// outer loop that keeps re-triggering it can absorb the hook forever and
/// never return control to the hook at all. Only a wall-clock timeout
/// enforced from outside the Lua VM (here, from the console's main thread,
/// watching the worker thread that owns the actual `Lua`) can defeat that.
/// Mirrors [`VALIDATE_TIMEOUT`]'s reasoning for the same class of escape
/// during validation's top-level load.
const TICK_TIMEOUT: Duration = Duration::from_millis(500);

/// A live, steppable deployment of a player's controller against a fresh
/// [`Simulation`]: the console's interactive counterpart to [`run`]. Unlike
/// `run`, which drives a script to completion in one blocking call,
/// [`LiveOperation::step`] advances exactly one tick per call so the
/// interactive console can pace ticks under player control (`Space`
/// pause/resume, `Enter` single-step — see `docs/TUI_DESIGN.md`'s
/// "Operation" section) instead of just observing a finished run.
///
/// The actual `Lua`, its loaded `on_tick` callback, and the live
/// `Simulation` all live on a dedicated worker thread spawned by
/// [`LiveOperation::deploy`], never on the caller's thread — `step` just
/// sends a request and waits up to [`TICK_TIMEOUT`] for a reply. This is
/// what lets a hung tick (a runaway `on_tick` that never returns at all,
/// e.g. because it keeps catching the instruction hook's own error with
/// `pcall`) be turned into a clean [`ControllerError::ExecutionLimitExceeded`]
/// instead of freezing the console's event loop: the worker thread is
/// simply abandoned rather than force-killed (Rust has no safe way to do
/// that), the same tradeoff [`validate`] already makes for a hung top-level
/// load.
pub struct LiveOperation {
    to_worker: mpsc::Sender<()>,
    from_worker: mpsc::Receiver<Result<TickRecord, WorkerError>>,
    finished: bool,
}

// `mpsc::Sender`/`Receiver` don't implement `Debug`, so this can't be
// derived. Just enough for `unwrap`/`unwrap_err`/`{:?}` in tests and any
// future caller-side logging.
impl fmt::Debug for LiveOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveOperation")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

/// What [`LiveOperation::deploy`]'s worker thread reports back once it's
/// finished (successfully or not) loading `source`, before it ever starts
/// waiting for step requests.
enum DeployOutcome {
    Ready,
    Failed(WorkerError),
}

/// A `Send`-safe mirror of [`ControllerError`], used to cross the channel
/// between [`LiveOperation`]'s worker thread and its caller. `mlua::Error`
/// (wrapped by several `ControllerError` variants) isn't `Send` — it can
/// hold an `Arc<dyn Error>` internally — so the worker converts each error
/// to its message text before sending, and the caller reconstructs an
/// equivalent `ControllerError` from that text on the other side. This
/// mirrors [`validate`]'s existing `ValidationOutcome`, which crosses the
/// same kind of thread boundary for the same reason.
enum WorkerError {
    ScriptInvalid(String),
    MissingCallback,
    CallbackFailed(String),
    InvalidAction(String),
    ExecutionLimitExceeded,
}

impl WorkerError {
    /// Captures just the inner message text of `err` — not `err.to_string()`
    /// itself, which for `ScriptInvalid`/`CallbackFailed` already includes
    /// `ControllerError`'s own "script failed to load: "/"'on_tick' raised
    /// an error: " prefix. Reconstructing from the *inner* message in
    /// [`WorkerError::into_controller_error`] (which re-applies that same
    /// prefix via the same `ControllerError` variant) keeps the message the
    /// caller ultimately sees identical to what the worker actually saw,
    /// instead of nesting the prefix twice.
    /// Every `String`-carrying variant is passed through
    /// [`truncate_diagnostic_message`] — the same bound [`validate`] applies
    /// to its own diagnostics — since the underlying text can originate
    /// from untrusted Lua (an `error()` call, or an action name a script
    /// returned) that's free to raise or return an arbitrarily large
    /// string within the sandbox's memory ceiling. Without it, an
    /// `Operation` that stores this error would re-render (and this
    /// conversion itself would re-allocate) a multi-megabyte diagnostic on
    /// every frame for as long as the result stays on screen.
    fn from_controller_error(err: ControllerError) -> Self {
        match err {
            ControllerError::MissingCallback => WorkerError::MissingCallback,
            ControllerError::InvalidAction(detail) => {
                WorkerError::InvalidAction(truncate_diagnostic_message(detail))
            }
            ControllerError::ExecutionLimitExceeded => WorkerError::ExecutionLimitExceeded,
            ControllerError::ScriptInvalid(inner) => {
                WorkerError::ScriptInvalid(truncate_diagnostic_message(inner.to_string()))
            }
            ControllerError::CallbackFailed(inner) => {
                WorkerError::CallbackFailed(truncate_diagnostic_message(inner.to_string()))
            }
            // `LiveOperation` never reads a script file itself (only
            // `run`'s `fs::read_to_string` path produces this variant), but
            // handle it defensively rather than let a future change panic.
            ControllerError::ScriptUnreadable { path, source } => {
                WorkerError::ScriptInvalid(truncate_diagnostic_message(format!(
                    "could not read script '{}': {source}",
                    path.display()
                )))
            }
        }
    }

    fn into_controller_error(self) -> ControllerError {
        match self {
            WorkerError::ScriptInvalid(message) => {
                ControllerError::ScriptInvalid(mlua::Error::RuntimeError(message))
            }
            WorkerError::MissingCallback => ControllerError::MissingCallback,
            WorkerError::CallbackFailed(message) => {
                ControllerError::CallbackFailed(mlua::Error::RuntimeError(message))
            }
            WorkerError::InvalidAction(message) => ControllerError::InvalidAction(message),
            WorkerError::ExecutionLimitExceeded => ControllerError::ExecutionLimitExceeded,
        }
    }
}

impl LiveOperation {
    /// Loads `source` into a fresh sandboxed `Lua` against a fresh
    /// [`Simulation`] on a dedicated worker thread (see [`LiveOperation`]'s
    /// own docs), and returns a handle that can step it one tick at a time
    /// instead of driving it to completion immediately. The top-level load
    /// itself is bounded exactly like [`validate`]'s (an instruction hook
    /// plus a [`VALIDATE_TIMEOUT`] wall-clock backstop): unlike `validate`,
    /// a controller is not required to pass validation before deployment,
    /// and an edit invalidates any earlier validation result, so deploying
    /// an unvalidated (or since-edited) source with a non-returning
    /// top-level statement — e.g. a bare `while true do end` outside any
    /// function — needs the same bounded-load guarantee `validate` already
    /// has, not just protection for `on_tick` itself.
    pub fn deploy(source: &str) -> Result<LiveOperation, ControllerError> {
        let source = source.to_string();
        let (to_worker, step_requests) = mpsc::channel::<()>();
        let (results_tx, from_worker) = mpsc::channel::<Result<TickRecord, WorkerError>>();
        let (ready_tx, ready_rx) = mpsc::channel::<DeployOutcome>();

        thread::spawn(move || {
            let lua = sandboxed_lua();
            let hook_fired = Rc::new(Cell::new(false));

            let load_hook_fired = Rc::clone(&hook_fired);
            let _ = lua.set_hook(
                HookTriggers {
                    every_nth_instruction: Some(VALIDATE_INSTRUCTION_BUDGET),
                    ..HookTriggers::default()
                },
                move |_, _| -> mlua::Result<VmState> {
                    load_hook_fired.set(true);
                    Err(mlua::Error::RuntimeError(
                        EXECUTION_ALLOWANCE_MESSAGE.to_string(),
                    ))
                },
            );

            let callback = match load_controller(&lua, &source) {
                Ok(callback) => callback,
                Err(err) => {
                    let _ = ready_tx.send(DeployOutcome::Failed(
                        WorkerError::from_controller_error(err),
                    ));
                    return;
                }
            };
            if hook_fired.get() {
                let _ = ready_tx.send(DeployOutcome::Failed(WorkerError::ScriptInvalid(
                    EXECUTION_ALLOWANCE_MESSAGE.to_string(),
                )));
                return;
            }
            if ready_tx.send(DeployOutcome::Ready).is_err() {
                // The caller already gave up waiting (see the
                // `recv_timeout` below) and dropped its receiver; there is
                // no one left to step this deployment, so there is nothing
                // useful left to do.
                return;
            }

            let mut simulation = Simulation::new();
            while step_requests.recv().is_ok() {
                // Reinstalling the hook resets its instruction countdown,
                // turning `TICK_INSTRUCTION_BUDGET` into a genuine per-tick
                // allowance rather than one shared across the deployment's
                // entire lifetime (an ordinary multi-tick operation doing
                // real but bounded work each tick must not be able to trip
                // it purely by accumulating ticks).
                hook_fired.set(false);
                let tick_hook_fired = Rc::clone(&hook_fired);
                let _ = lua.set_hook(
                    HookTriggers {
                        every_nth_instruction: Some(TICK_INSTRUCTION_BUDGET),
                        ..HookTriggers::default()
                    },
                    move |_, _| -> mlua::Result<VmState> {
                        tick_hook_fired.set(true);
                        Err(mlua::Error::RuntimeError(
                            "controller exceeded its execution allowance while running".to_string(),
                        ))
                    },
                );

                let result = advance_tick(&lua, &callback, &mut simulation, Some(&hook_fired));
                let finished = !matches!(
                    &result,
                    Ok(TickRecord {
                        outcome: TickOutcome::Running,
                        ..
                    })
                );
                let result = result.map_err(WorkerError::from_controller_error);
                if results_tx.send(result).is_err() {
                    // The caller gave up (see `step`'s `recv_timeout`) and
                    // dropped its receiver; nothing left to report to.
                    return;
                }
                if finished {
                    return;
                }
            }
        });

        match ready_rx.recv_timeout(VALIDATE_TIMEOUT) {
            Ok(DeployOutcome::Ready) => Ok(LiveOperation {
                to_worker,
                from_worker,
                finished: false,
            }),
            Ok(DeployOutcome::Failed(err)) => Err(err.into_controller_error()),
            Err(_) => Err(ControllerError::ScriptInvalid(mlua::Error::RuntimeError(
                EXECUTION_ALLOWANCE_MESSAGE.to_string(),
            ))),
        }
    }

    /// Advances the deployment by exactly one tick, calling `on_tick` once
    /// and applying the action it returns. Returns an error (without
    /// advancing the simulation) if the callback fails, returns an invalid
    /// action, or exceeds its per-tick execution allowance — including the
    /// case where `on_tick` never returns at all, via [`TICK_TIMEOUT`].
    /// Calling `step` again after the operation has already finished (a
    /// prior call returned an `outcome` other than [`TickOutcome::Running`],
    /// or this method already returned an error) is a programmer error the
    /// caller should avoid by checking [`LiveOperation::is_finished`] first;
    /// `step` does not re-check that itself so a caller inspecting the
    /// final tick's own result doesn't need an extra call first.
    pub fn step(&mut self) -> Result<TickRecord, ControllerError> {
        if self.to_worker.send(()).is_err() {
            // The worker thread has already exited (it only does so after
            // reporting a final result, which would already have set
            // `finished`) — a caller ignoring that contract, not a runaway
            // script. Report it the same way a hang would be reported
            // rather than panicking on an internal invariant.
            self.finished = true;
            return Err(ControllerError::ExecutionLimitExceeded);
        }

        match self.from_worker.recv_timeout(TICK_TIMEOUT) {
            Ok(Ok(record)) => {
                if record.outcome != TickOutcome::Running {
                    self.finished = true;
                }
                Ok(record)
            }
            Ok(Err(err)) => {
                self.finished = true;
                Err(err.into_controller_error())
            }
            Err(_) => {
                // The worker hasn't answered within the allowance — either
                // a genuinely runaway `on_tick` (see `TICK_TIMEOUT`'s doc
                // comment) or, defensively, a panicked/disconnected worker.
                // The thread is abandoned, not force-killed (Rust has no
                // safe way to do that); the sandbox's stripped standard
                // library and memory ceiling bound its worst case, the same
                // tradeoff `validate` already makes for a hung load.
                self.finished = true;
                Err(ControllerError::ExecutionLimitExceeded)
            }
        }
    }

    /// Whether this deployment has ended — successfully, in failure, or
    /// because [`LiveOperation::step`] returned an error — and should not
    /// be stepped again.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The fixed scenario's starting operational budget, for rendering
    /// `budget_remaining / starting_budget` telemetry. Every [`LiveOperation`]
    /// runs the same fixed "First Contact" scenario (see [`Simulation::new`]),
    /// so this needs no per-deployment state to compute.
    pub fn starting_budget(&self) -> u32 {
        crate::simulation::Scenario::first_contact().starting_budget()
    }

    /// A read-only snapshot of the fixed scenario's starting state,
    /// independent of whether any tick has run yet — the drone's starting
    /// position and whatever a fresh [`Simulation::new`] reveals around it
    /// — for rendering before the first tick completes. Every deployment
    /// starts from the same fixed scenario (see
    /// [`LiveOperation::starting_budget`]), so this needs no interaction
    /// with the worker thread actually running this deployment; callers
    /// only ever use it before any tick has advanced, at which point it's
    /// identical to that live state anyway.
    pub fn observe(&self) -> Observation {
        Simulation::new().observe()
    }

    /// The fixed scenario's facility width, for rendering the satellite
    /// feed before any [`TickRecord`] (which also carries it) exists yet.
    pub fn map_width(&self) -> i32 {
        crate::simulation::Scenario::first_contact().map().width()
    }

    /// The fixed scenario's facility height; see [`LiveOperation::map_width`].
    pub fn map_height(&self) -> i32 {
        crate::simulation::Scenario::first_contact().map().height()
    }
}

/// Loads `source` into `lua`, confirms it exposes the required `on_tick`
/// callback, and returns that exact callback value — without invoking it.
/// Shared by [`run`] and [`validate`] so the console's Controller view can
/// check whether a player's edited source is loadable Lua before anything
/// ever tries to deploy or execute it.
///
/// Returning the looked-up `Function` (rather than just confirming it
/// exists) matters: `on_tick` is fetched via an ordinary global lookup,
/// which a script is free to route through its own `__index` metamethod on
/// `_G` (`setmetatable` isn't restricted, and neither is setting a
/// metatable on the globals table). A *stateful* `__index` — one that
/// returns a function only the first time `on_tick` is requested, `nil`
/// after — would otherwise pass this check and then immediately fail a
/// second, separate lookup done later to actually call it, even within a
/// single [`run`], despite this function having already proven a callback
/// was available. Callers must reuse the value returned here as the
/// callback to invoke, not look it up again.
fn load_controller(lua: &Lua, source: &str) -> Result<Function, ControllerError> {
    lua.load(source)
        .set_name("controller.lua")
        .exec()
        .map_err(ControllerError::ScriptInvalid)?;

    match lua.globals().get::<Function>(ON_TICK) {
        Ok(callback) => Ok(callback),
        // `on_tick` is an ordinary global lookup, which a script can route
        // through its own `__index` metamethod on `_G` (`setmetatable`
        // isn't restricted). If that metamethod itself raises — as
        // opposed to just not returning a function — the failure is a
        // genuine script error, not "the callback is missing"; folding
        // both into `MissingCallback` would discard the real diagnostic
        // (and its useful source information) behind a misleading
        // message. Only a clean type mismatch (nil, or a non-function
        // value) is really "missing"; anything else is `ScriptInvalid`.
        Err(mlua::Error::FromLuaConversionError { .. }) => Err(ControllerError::MissingCallback),
        Err(err) => Err(ControllerError::ScriptInvalid(err)),
    }
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

/// A ceiling on how long a diagnostic message from a failed [`validate`]
/// crosses the background thread's channel and ends up stored (e.g. in the
/// console's `AppState`, re-rendered on every frame). Real Lua load/runtime
/// errors are always short; this exists so a script that deliberately raises
/// a multi-megabyte string (`error(string.rep("x", 5_000_000))`) can't turn
/// a validation failure into a UI stall, even though `validate` itself still
/// finishes well within [`VALIDATE_TIMEOUT`].
const MAX_DIAGNOSTIC_MESSAGE_LEN: usize = 4096;

/// Truncates `message` to [`MAX_DIAGNOSTIC_MESSAGE_LEN`] bytes (on a char
/// boundary) with a trailing marker, leaving shorter messages untouched.
fn truncate_diagnostic_message(message: String) -> String {
    if message.len() <= MAX_DIAGNOSTIC_MESSAGE_LEN {
        return message;
    }
    let mut end = MAX_DIAGNOSTIC_MESSAGE_LEN;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = message[..end].to_string();
    truncated.push_str(" ... (truncated)");
    truncated
}

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
        // The instruction-hook error is an ordinary catchable Lua error —
        // top-level source wrapping its own budget-exceeding work in
        // `pcall` can silently absorb it and carry on (e.g. defining
        // `on_tick` only in the `pcall`'s failure branch), reaching a
        // clean `Ok(())`/`MissingCallback` outcome below despite having
        // actually hit the ceiling. `run()` installs no such hook at all,
        // so the *same* source there would behave completely differently:
        // it wouldn't error at that point, wouldn't skip whatever the
        // `pcall` was guarding, and could hang forever if that guarded
        // work was actually a runaway loop. Recording whether the hook
        // ever fired — independent of whatever `load_controller` itself
        // ultimately returns — lets the outcome below reject the whole
        // validation once it did, instead of trusting a result the
        // top-level script may have manufactured specifically to hide it.
        let hook_fired = Arc::new(AtomicBool::new(false));
        let hook_fired_in_hook = Arc::clone(&hook_fired);
        let _ = lua.set_hook(
            HookTriggers {
                every_nth_instruction: Some(VALIDATE_INSTRUCTION_BUDGET),
                ..HookTriggers::default()
            },
            move |_, _| -> mlua::Result<VmState> {
                hook_fired_in_hook.store(true, Ordering::SeqCst);
                Err(mlua::Error::RuntimeError(
                    EXECUTION_ALLOWANCE_MESSAGE.to_string(),
                ))
            },
        );
        let mut outcome = match load_controller(&lua, &source) {
            Ok(_) => ValidationOutcome::Ok,
            Err(ControllerError::MissingCallback) => ValidationOutcome::MissingCallback,
            Err(ControllerError::ScriptInvalid(err)) => {
                ValidationOutcome::Invalid(truncate_diagnostic_message(err.to_string()))
            }
            Err(_) => ValidationOutcome::Invalid(
                "controller failed to load for an unexpected reason".to_string(),
            ),
        };
        if hook_fired.load(Ordering::SeqCst) {
            outcome = ValidationOutcome::Invalid(EXECUTION_ALLOWANCE_MESSAGE.to_string());
        }
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
    use crate::simulation::FailureReason;

    /// `MAX_CONCURRENT_VALIDATIONS` is a *production* safety cap sized for
    /// the single sequential caller the console's event loop actually is
    /// (see its doc comment); `cargo test`'s default thread pool can run
    /// far more than that many of this module's `validate`-calling tests
    /// truly concurrently, and how many depends on the machine/runner's
    /// available parallelism, not anything this module controls. Left
    /// alone, that makes the test outcome depend on hardware/thread count
    /// instead of the code under test — exactly the kind of environmental
    /// nondeterminism AGENTS.md's testing section asks tests to avoid.
    /// Every test-module call to `validate` goes through this instead,
    /// serialized behind one lock, so they never compete for that
    /// production cap regardless of how many the test runner schedules at
    /// once.
    static VALIDATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn validate_locked(source: &str) -> Result<(), ControllerError> {
        let _guard = VALIDATE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate(source)
    }

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
    fn a_raising_index_metamethod_during_on_tick_lookup_is_script_invalid_not_missing_callback() {
        // A stateful (or simply buggy) `__index` on `_G` can raise while
        // `on_tick` is being looked up — a genuine script error, not "the
        // callback is missing." Folding both into `MissingCallback` would
        // hide the real diagnostic (and its useful source information)
        // behind a misleading message.
        let source = r#"
            setmetatable(_G, {
                __index = function(_, key)
                    error("lookup exploded: " .. tostring(key))
                end,
            })
        "#;
        let lua = sandboxed_lua();
        let err = load_controller(&lua, source).unwrap_err();
        match err {
            ControllerError::ScriptInvalid(mlua_err) => {
                assert!(
                    mlua_err.to_string().contains("lookup exploded"),
                    "{mlua_err}"
                );
            }
            other => panic!("expected ScriptInvalid, got {other}"),
        }
    }

    #[test]
    fn validate_accepts_a_script_defining_on_tick() {
        assert!(validate_locked("function on_tick(observation) return \"wait\" end").is_ok());
    }

    #[test]
    fn load_controller_returns_a_reusable_callback_even_with_a_stateful_index_metamethod() {
        // `on_tick` is fetched via an ordinary global lookup, which a
        // script can route through its own `__index` on `_G`. A stateful
        // one that grants a function only the *first* time `on_tick` is
        // requested, `nil` after, must not be asked a second time — the
        // callback `load_controller` already looked up and confirmed
        // exists is the one that must actually get called, not a fresh
        // lookup that this kind of metamethod could fail.
        let source = r#"
            local granted = false
            setmetatable(_G, {
                __index = function(_, key)
                    if key == "on_tick" and not granted then
                        granted = true
                        return function(observation) return "wait" end
                    end
                    return nil
                end,
            })
        "#;
        let lua = sandboxed_lua();
        let callback =
            load_controller(&lua, source).expect("on_tick should be found on the first lookup");
        let observation = lua.create_table().expect("table");
        let result: String = callback
            .call(observation)
            .expect("the callback load_controller already returned must still be callable");
        assert_eq!(result, "wait");
    }

    #[test]
    fn validate_rejects_a_syntax_error() {
        let err = validate_locked("function on_tick( ").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
        // An ordinary syntax error is not an execution-limit failure, even
        // though both are reported as `ScriptInvalid` — only the specific
        // execution-allowance message should read as one.
        assert!(!err.is_execution_limit());
    }

    #[test]
    fn is_execution_limit_recognizes_the_top_level_execution_allowance_message() {
        let err = ControllerError::ScriptInvalid(mlua::Error::RuntimeError(
            EXECUTION_ALLOWANCE_MESSAGE.to_string(),
        ));
        assert!(err.is_execution_limit());
    }

    #[test]
    fn is_execution_limit_recognizes_a_callback_execution_limit() {
        assert!(ControllerError::ExecutionLimitExceeded.is_execution_limit());
    }

    #[test]
    fn is_execution_limit_rejects_unrelated_failures() {
        assert!(!ControllerError::MissingCallback.is_execution_limit());
        assert!(!ControllerError::InvalidAction("bad".to_string()).is_execution_limit());
    }

    #[test]
    fn validate_rejects_a_script_missing_on_tick() {
        let err = validate_locked("local x = 1").unwrap_err();
        assert!(matches!(err, ControllerError::MissingCallback));
    }

    #[test]
    fn validate_does_not_execute_on_tick() {
        // If this ran on_tick, `error(...)` would surface as CallbackFailed
        // instead of validate succeeding; validate must only load the
        // script and check the callback exists.
        assert!(
            validate_locked("function on_tick(observation) error('should not run') end").is_ok()
        );
    }

    #[test]
    fn validate_bounds_a_runaway_top_level_loop_instead_of_hanging() {
        let err = validate_locked("while true do end").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
        assert!(err.to_string().contains("execution allowance"));
    }

    #[test]
    fn validate_rejects_a_script_that_catches_the_instruction_hook_with_pcall() {
        // The instruction-hook's error is an ordinary catchable Lua error;
        // a script could otherwise `pcall` around a budget-exceeding loop
        // and only define `on_tick` in the failure branch, reaching a
        // clean `Ok`/`MissingCallback` outcome that hides having hit the
        // ceiling — `run()` has no such hook at all, so the same source
        // there behaves completely differently (this exact loop would just
        // hang forever). Whether the hook ever fired must make the whole
        // validation fail regardless of what the top-level script did
        // afterward.
        let source = r#"
            local ok = pcall(function()
                while true do end
            end)
            if not ok then
                function on_tick(observation) return "wait" end
            end
        "#;
        let err = validate_locked(source).unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
        assert!(err.to_string().contains("execution allowance"), "{err}");
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
            let err = validate_locked(source).unwrap_err();
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
        let err = validate_locked("math.randomseed()").unwrap_err();
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
        assert!(validate_locked(source).is_ok());
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
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_orders_a_boundary_integer_and_float_key_without_precision_loss() {
        // i64::MAX and 2.0^63 (one past it) are distinct table keys, but a
        // naive `integer as f64` cast rounds i64::MAX up to exactly 2^63,
        // making the two compare equal and causing `next` to skip one of
        // them when resuming from the other.
        let source = r#"
            local order = {}
            for k in pairs({[9223372036854775807] = "int", [9223372036854775808.0] = "float"}) do
                order[#order + 1] = tostring(k)
            end
            assert(#order == 2, #order)
            assert(order[1] == "9223372036854775807", order[1])
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_does_not_retain_a_growing_snapshot_per_call() {
        // `pairs(t)` (absent a custom `__pairs`) now returns the same
        // pre-existing `next` function, the table itself, and `nil` — no
        // full sorted-key snapshot allocated per call — instead of a fresh
        // closure wrapping its own copy of the table's entries. Saving many
        // iterators without ever driving them (a pattern an earlier
        // version of this sandbox could accumulate unbounded *host* memory
        // for, since a Rust-heap `Vec` per call sat outside Lua's own
        // tracked memory) must therefore stay cheap enough to succeed at a
        // moderate call count, not run into the sandbox's memory limit at
        // all — a lower call count than an earlier version of this test
        // used, since `pairs`'s own error-preserving wrapper (see
        // `wrap_lua_errors_as_strings`) now costs a few real Lua
        // instructions per call, and 200,000 calls to *anything* eventually
        // trips the unrelated instruction-budget guard this isn't testing.
        let source = r#"
            local t = {}
            for i = 1, 200 do t["key" .. i] = i end
            local saved = {}
            for i = 1, 10000 do
                saved[i] = pairs(t)
            end
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn string_format_rejects_a_table_argument_to_percent_q() {
        // Unlike %s, %q has no default representation to fall back to at
        // all — real Lua rejects a table argument outright since it has
        // no literal form. The pointer-hiding %s treatment must not mask
        // that by unconditionally normalizing every reference-typed
        // argument regardless of which specifier consumes it.
        let err = validate_locked("string.format('%q', {})").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn pairs_propagates_an_error_for_a_non_callable_pairs_metamethod() {
        let source = "for k in pairs(setmetatable({a = 1}, {__pairs = true})) do end";
        let err = validate_locked(source).unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn pairs_metamethod_can_be_a_table_with_its_own_call_metamethod() {
        // Real Lua dispatches *any* callable value through `__call`, not
        // only actual functions — a table whose own metatable sets
        // `__call` is valid Lua for `__pairs` too.
        let source = r#"
            local callable_pairs = setmetatable({}, {
                __call = function(_, self)
                    local done = false
                    return function()
                        if done then return nil end
                        done = true
                        return "only", "value"
                    end, self, nil
                end,
            })
            local proxy = setmetatable({}, {__pairs = callable_pairs})
            local seen = {}
            for k, v in pairs(proxy) do
                seen[#seen + 1] = k .. "=" .. v
            end
            assert(#seen == 1 and seen[1] == "only=value", table.concat(seen, ","))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_rejects_a_table_valued_key_instead_of_leaving_it_nondeterministic() {
        let err = validate_locked("for k in pairs({[{}] = 1}) do end").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn next_rejects_a_control_key_that_was_never_a_member_of_the_table() {
        // Real Lua's `next` raises "invalid key to 'next'" for a control
        // value that was never actually a member of the table (a typo, or
        // one fabricated to probe for a result) — a naive "find the first
        // entry greater than key" search instead succeeds for any key,
        // silently returning whatever sorts after it (here, "b", 1), which
        // could make `pcall`-guarded controller logic take a success
        // branch it never should against real Lua.
        let source = r#"
            local ok = pcall(next, {b = 1}, "a")
            assert(not ok, "next({b = 1}, 'a') should raise 'invalid key'")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn next_invalid_key_error_is_a_lua_string_not_userdata() {
        // A Rust-implemented callback's `Err(...)` always crosses into Lua
        // wrapped in a full userdata (`mlua`'s own callback-error
        // handling), but real Lua's `next` raises a genuine string — a
        // script inspecting, concatenating, or forwarding the caught
        // message needs `type(err) == "string"` to work the same way it
        // would against real Lua.
        let source = r#"
            local ok, err = pcall(next, {b = 1}, "a")
            assert(not ok, "expected next to raise")
            assert(type(err) == "string", "type(err) was " .. type(err))
            assert(err:find("invalid key", 1, true) ~= nil, err)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn next_still_tolerates_a_key_deleted_since_the_last_call() {
        // The fabricated-key rejection above must not regress the
        // documented "clearing the current field" tolerance: a key that
        // *was* a genuine member (and was reached via `next` itself) stays
        // valid as a control value even after being deleted.
        let source = r#"
            local t = {a = 1, b = 2}
            local k = next(t)
            t[k] = nil
            local ok = pcall(next, t, k)
            assert(ok, "a key deleted since the last next() call must still be accepted")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
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
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_does_not_resurrect_a_deleted_key_via_an_index_metamethod() {
        // A table's `__index` fallback must not make a key deleted mid-
        // traversal look "live" again: real `next`/`pairs` only ever
        // traverses raw entries, ignoring `__index` entirely.
        let source = r#"
            local t = setmetatable({a = 1, b = 2}, {
                __index = function(_, _) return "fallback" end,
            })
            local seen = {}
            for k in pairs(t) do
                if k == "a" then
                    t.b = nil
                end
                seen[#seen + 1] = k
            end
            assert(#seen == 1 and seen[1] == "a", table.concat(seen, ","))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
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
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn next_returns_a_single_nil_once_exhausted() {
        // Real Lua 5.4's `next` returns exactly one `nil` when there's
        // nothing left to yield, not a `nil, nil` pair — `select("#", ...)`
        // is how a script would observe the difference.
        let source = r##"
            assert(select("#", next({})) == 1, select("#", next({})))
            local t = {only = 1}
            local k = next(t)
            assert(k == "only", k)
            assert(select("#", next(t, k)) == 1, select("#", next(t, k)))
            function on_tick(observation) return "wait" end
        "##;
        assert!(validate_locked(source).is_ok());
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
        assert!(validate_locked(source).is_ok());
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
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_truncates_a_pairs_metamethod_that_returns_more_than_three_values() {
        // Real Lua 5.4's `pairs` forwards only the first three results of
        // calling `__pairs`; a fourth (or later) leaked value would be
        // mistaken by the generic `for` loop's own protocol for a
        // to-be-closed value, breaking a loop that works fine against real
        // Lua.
        let source = r#"
            local proxy = setmetatable({}, {
                __pairs = function(self)
                    local done = false
                    return function()
                        if done then return nil end
                        done = true
                        return "only", "value"
                    end, self, nil, "leaked fourth value"
                end,
            })
            for k, v in pairs(proxy) do
                assert(k == "only" and v == "value", k .. "=" .. v)
            end
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_pads_a_pairs_metamethod_that_returns_fewer_than_three_values() {
        // Real Lua 5.4 requests exactly three results from `__pairs`; a
        // metamethod returning fewer must be padded with `nil`, not left
        // short — `select("#", pairs(t))` is `3` in real Lua even when
        // `__pairs` only returns one value.
        let source = r##"
            local proxy = setmetatable({}, {
                __pairs = function(self) return next, self end,
            })
            local count = select("#", pairs(proxy))
            assert(count == 3, count)
            function on_tick(observation) return "wait" end
        "##;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_iterator_returns_a_single_nil_once_exhausted_when_called_directly() {
        // The position-based iterator `pairs` returns (absent a custom
        // `__pairs`) must match `next`'s exhaustion arity too, since
        // controller code can call it directly instead of only through a
        // `for` loop.
        let source = r##"
            local f, s, k = pairs({})
            assert(select("#", f(s, k)) == 1, select("#", f(s, k)))
            function on_tick(observation) return "wait" end
        "##;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_returns_the_table_itself_as_the_iterator_state() {
        // Real Lua's `pairs(t)` (absent a custom `__pairs`) returns `next,
        // t, nil` — the table itself as the second result, not `nil`.
        let source = r##"
            local t = {a = 1}
            local f, s, k = pairs(t)
            assert(s == t, s)
            assert(k == nil, k)
            function on_tick(observation) return "wait" end
        "##;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_iterator_honors_the_control_argument_it_is_called_with() {
        // The iterator `pairs` returns must behave like a stateless `next`:
        // calling it twice with the *same* control value must return the
        // *same* key both times, not silently advance an internal
        // position. A generic-iteration helper that restarts or replays a
        // traversal by control value (rather than only driving it forward
        // through an ordinary `for` loop) depends on this.
        let source = r##"
            local t = {a = 1, b = 2}
            local f, s = pairs(t)
            local k1, v1 = f(s, nil)
            local k2, v2 = f(s, nil)
            assert(k1 == k2 and v1 == v2, (k1 or "nil") .. " ~= " .. (k2 or "nil"))
            function on_tick(observation) return "wait" end
        "##;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn tostring_hides_the_process_address_of_a_table() {
        let lua_source = "local text = tostring({})\n\
                           assert(text == 'table: 0x0', text)\n\
                           function on_tick(observation) return 'wait' end";
        assert!(validate_locked(lua_source).is_ok());
    }

    #[test]
    fn tostring_leaves_an_ordinary_string_that_looks_like_the_address_pattern_alone() {
        let source = r#"
            local text = tostring("table: 0xabc")
            assert(text == "table: 0xabc", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn tostring_hides_the_address_of_a_table_with_a_custom_name() {
        let source = r#"
            local named = setmetatable({}, {__name = "sentinel"})
            local text = tostring(named)
            assert(text == "sentinel: 0x0", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn tostring_preserves_a_custom_name_that_itself_contains_the_address_marker() {
        // `__name` is entirely script-controlled text; Lua always appends
        // the real ": 0x<address>" marker *after* it, so only the trailing
        // occurrence is the genuine one to normalize — a name that happens
        // to contain "): 0x" earlier in the string must be preserved as-is.
        let source = r#"
            local named = setmetatable({}, {__name = "literal: 0xabc"})
            local text = tostring(named)
            assert(text == "literal: 0xabc: 0x0", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn validate_rejects_a_script_using_string_pack() {
        for source in [
            "string.pack('I2', 1)",
            "string.unpack('I2', 'xx')",
            "string.packsize('I2')",
        ] {
            let err = validate_locked(source).unwrap_err();
            assert!(
                matches!(err, ControllerError::ScriptInvalid(_)),
                "{source} should fail without string.pack/unpack/packsize"
            );
        }
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
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn tostring_does_not_mangle_custom_tostring_output_that_contains_the_address_marker() {
        // A custom `__tostring` is free to legitimately produce text
        // containing "): 0x" that isn't an address at all; the override
        // must trust it completely rather than searching its output for
        // the same marker it looks for in the *default* representation.
        let source = r#"
            local widget = setmetatable({}, {
                __tostring = function(_) return "widget(id=7): 0x1 of stock" end,
            })
            local text = tostring(widget)
            assert(text == "widget(id=7): 0x1 of stock", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_ignores_an_inherited_pairs_metamethod() {
        // `__pairs` set on a *parent* metatable (reached through the
        // metatable's own `__index`) must not be picked up — real Lua only
        // ever looks at the table's own metatable's raw `__pairs` entry.
        let source = r#"
            local base = {__pairs = function(self)
                error("inherited __pairs must not run")
            end}
            local mt = setmetatable({}, {__index = base})
            local t = setmetatable({a = 1}, mt)
            local seen = {}
            for k, v in pairs(t) do
                seen[#seen + 1] = k .. "=" .. v
            end
            assert(#seen == 1 and seen[1] == "a=1", table.concat(seen, ","))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn validate_rejects_a_script_using_string_dump() {
        let err = validate_locked("string.dump(function() end)").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn validate_rejects_a_script_using_collectgarbage() {
        let err = validate_locked("collectgarbage('count')").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn validate_truncates_an_excessively_large_diagnostic_message() {
        let source = "error(string.rep('x', 5000000))";
        let err = validate_locked(source).unwrap_err();
        let ControllerError::ScriptInvalid(inner) = err else {
            panic!("expected ScriptInvalid, got {err}");
        };
        let message = inner.to_string();
        assert!(
            message.len() < 5_000_000,
            "diagnostic message should be truncated, was {} bytes",
            message.len()
        );
        assert!(message.ends_with("(truncated)"), "{message}");
    }

    #[test]
    fn tostring_rejects_being_called_with_no_arguments() {
        // A single `Value` parameter can't distinguish "no argument" from
        // an explicit `nil` — real Lua's `tostring` explicitly checks and
        // raises for zero arguments, while `tostring(nil)` succeeds.
        let source = r#"
            local ok = pcall(tostring)
            assert(not ok, "tostring() with no arguments should raise")
            assert(tostring(nil) == "nil")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn tostring_error_from_a_custom_tostring_is_a_lua_string_not_userdata() {
        let source = r#"
            local raises = setmetatable({}, {__tostring = function(_) error("boom") end})
            local ok, err = pcall(tostring, raises)
            assert(not ok, "expected tostring to raise")
            assert(type(err) == "string", "type(err) was " .. type(err))
            assert(err:find("boom", 1, true) ~= nil, err)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn pairs_error_from_a_pairs_metamethod_is_a_lua_string_not_userdata() {
        let source = r#"
            local proxy = setmetatable({}, {__pairs = function(_) error("boom") end})
            local ok, err = pcall(pairs, proxy)
            assert(not ok, "expected pairs to raise")
            assert(type(err) == "string", "type(err) was " .. type(err))
            assert(err:find("boom", 1, true) ~= nil, err)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn string_format_error_is_a_lua_string_not_userdata() {
        let source = r#"
            local ok, err = pcall(string.format, "%d", {})
            assert(not ok, "expected string.format to raise")
            assert(type(err) == "string", "type(err) was " .. type(err))
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn tostring_preserves_a_non_utf8_lua_string_unchanged() {
        let source = r#"
            local raw = string.char(255)
            assert(tostring(raw) == raw)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn validate_rejects_a_script_that_calls_load() {
        let err = validate_locked("load('return 1')").unwrap_err();
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
            let err = validate_locked(source).unwrap_err();
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
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn string_format_hides_the_process_address_of_a_table_via_percent_s() {
        let source = r#"
            local text = string.format("%s", {})
            assert(text == "table: 0x0", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn string_format_still_respects_a_custom_tostring_metamethod_via_percent_s() {
        let source = r#"
            local labeled = setmetatable({}, {__tostring = function(_) return "named" end})
            assert(string.format("%s", labeled) == "named")
            assert(string.format("%s and %d", "x", 3) == "x and 3")
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn string_format_does_not_call_a_later_custom_tostring_when_an_earlier_conversion_fails() {
        // Real Lua's `string.format` processes conversions strictly left
        // to right and stops at the first one that fails — a later `%s`
        // argument's own `__tostring` (here, one with an observable side
        // effect) must never run if an earlier `%d` conversion given a
        // non-number already failed, since real Lua never reaches it.
        let source = r#"
            local calls = 0
            local labeled = setmetatable({}, {
                __tostring = function(_) calls = calls + 1 return "named" end,
            })
            local ok = pcall(string.format, "%d %s", {}, labeled)
            assert(not ok, "expected the %d conversion to fail")
            assert(calls == 0, "the later %s argument's __tostring must not have run, calls=" .. calls)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn string_format_accepts_a_non_utf8_format_string() {
        let source = r#"
            local fmt = string.char(255) .. "%d"
            local text = string.format(fmt, 7)
            assert(text == string.char(255) .. "7", text)
            function on_tick(observation) return "wait" end
        "#;
        assert!(validate_locked(source).is_ok());
    }

    #[test]
    fn validate_bounds_excessive_native_allocation() {
        let err = validate_locked("local n = 1 << 30\nlocal s = string.rep('x', n)").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn validate_and_load_controller_agree_on_a_gc_finalizer_that_would_define_on_tick() {
        // An unreachable table's `__gc` finalizer running automatically at
        // some allocation-pacing-dependent point during the top-level
        // chunk — rather than only at state teardown — used to be able to
        // define `on_tick` in `validate` (whose instruction hook is an
        // extra allocation source) without `run`'s unhooked
        // `load_controller` seeing the same thing happen at the same
        // point, reporting `READY`/`MissingCallback` inconsistently for
        // byte-identical source with no memory-introspection API involved.
        // `sandboxed_lua`'s `gc_stop` means neither ever runs the
        // finalizer *during* loading at all, so both must agree here.
        let source = r#"
            local sentinel = {}
            setmetatable(sentinel, {__gc = function()
                function on_tick(observation) return "wait" end
            end})
            sentinel = nil
            for _ = 1, 11 do
                local _ = string.rep("x", 64)
            end
        "#;
        let validate_result = validate_locked(source);
        let load_result = load_controller(&sandboxed_lua(), source);
        assert!(
            matches!(validate_result, Err(ControllerError::MissingCallback)),
            "validate: {validate_result:?}"
        );
        assert!(
            matches!(load_result, Err(ControllerError::MissingCallback)),
            "load_controller: {load_result:?}"
        );
    }

    #[test]
    fn validate_rejects_scripts_that_reach_for_host_capabilities() {
        for source in [
            "os.execute('true')",
            "io.open('/etc/passwd')",
            "require('os')",
        ] {
            let err = validate_locked(source).unwrap_err();
            assert!(
                matches!(err, ControllerError::ScriptInvalid(_)),
                "{source} should fail to load without host library access"
            );
        }
    }

    fn step_to_completion(op: &mut LiveOperation) -> (Vec<TickRecord>, ControllerError) {
        let mut records = Vec::new();
        loop {
            match op.step() {
                Ok(record) if record.outcome == TickOutcome::Running => records.push(record),
                Ok(record) => {
                    records.push(record);
                    panic!("expected step_to_completion to be used only on failing scripts");
                }
                Err(err) => return (records, err),
            }
        }
    }

    #[test]
    fn live_operation_deploy_rejects_a_syntax_error() {
        let err = LiveOperation::deploy("function on_tick( ").unwrap_err();
        assert!(matches!(err, ControllerError::ScriptInvalid(_)));
    }

    #[test]
    fn live_operation_deploy_rejects_a_script_missing_on_tick() {
        let err =
            LiveOperation::deploy("function some_other_function(observation) return 'wait' end")
                .unwrap_err();
        assert!(matches!(err, ControllerError::MissingCallback));
    }

    #[test]
    fn live_operation_step_advances_exactly_one_tick() {
        let mut op = LiveOperation::deploy("function on_tick(observation) return 'wait' end")
            .expect("valid controller");
        assert!(!op.is_finished());

        let first = op.step().expect("wait is always a valid action");
        assert_eq!(first.tick, 1);
        assert_eq!(first.outcome, TickOutcome::Running);
        assert!(!op.is_finished());

        let second = op.step().expect("wait is always a valid action");
        assert_eq!(second.tick, 2);
        assert!(!op.is_finished());
    }

    #[test]
    fn live_operation_reports_a_callback_error_and_finishes() {
        let mut op = LiveOperation::deploy(
            "function on_tick(observation) error('simulated controller malfunction') end",
        )
        .expect("script loads even though on_tick will fail when called");

        let err = op.step().unwrap_err();
        assert!(matches!(err, ControllerError::CallbackFailed(_)), "{err}");
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_reports_an_invalid_action() {
        let mut op = LiveOperation::deploy("function on_tick(observation) return 'north-east' end")
            .expect("script loads");

        let err = op.step().unwrap_err();
        assert!(matches!(err, ControllerError::InvalidAction(_)), "{err}");
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_runs_a_route_to_a_deterministic_success() {
        let source = r#"
            local route = { "north", "east", "east", "east", "east", "north", "north", "north" }
            local step = 0
            function on_tick(observation)
                step = step + 1
                return route[step]
            end
        "#;
        let mut op = LiveOperation::deploy(source).expect("valid controller");

        let mut last_outcome = TickOutcome::Running;
        let mut ticks = 0;
        while last_outcome == TickOutcome::Running {
            let record = op.step().expect("route never returns an invalid action");
            last_outcome = record.outcome;
            ticks += 1;
            assert!(ticks <= 8, "route should reach the uplink within 8 ticks");
        }
        assert_eq!(last_outcome, TickOutcome::Succeeded);
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_running_the_same_source_twice_produces_identical_ticks() {
        let source = r#"
            local route = { "north", "east", "east", "east", "east", "north", "north", "north" }
            local step = 0
            function on_tick(observation)
                step = step + 1
                return route[step]
            end
        "#;

        let run_to_completion = || {
            let mut op = LiveOperation::deploy(source).expect("valid controller");
            let mut records = Vec::new();
            loop {
                let record = op.step().expect("route never returns an invalid action");
                let finished = record.outcome != TickOutcome::Running;
                records.push(record);
                if finished {
                    return records;
                }
            }
        };

        assert_eq!(run_to_completion(), run_to_completion());
    }

    #[test]
    fn live_operation_budget_exhaustion_fails_deterministically() {
        let mut op = LiveOperation::deploy("function on_tick(observation) return 'wait' end")
            .expect("valid controller");

        let mut last_outcome = TickOutcome::Running;
        while last_outcome == TickOutcome::Running {
            last_outcome = op.step().expect("wait is always valid").outcome;
        }
        assert_eq!(
            last_outcome,
            TickOutcome::Failed(FailureReason::BudgetExhausted)
        );
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_step_after_the_operation_finished_still_returns_a_stopped_result() {
        let mut op = LiveOperation::deploy(
            "function on_tick(observation) error('simulated controller malfunction') end",
        )
        .expect("script loads");
        let _ = step_to_completion(&mut op);
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_bounds_a_runaway_on_tick_instead_of_hanging() {
        let mut op = LiveOperation::deploy("function on_tick(observation) while true do end end")
            .expect("script loads; the infinite loop is only inside on_tick");

        let err = op.step().unwrap_err();
        assert!(
            matches!(err, ControllerError::ExecutionLimitExceeded),
            "{err}"
        );
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_bounds_a_runaway_on_tick_wrapped_in_pcall() {
        // Same defense as `validate`'s pcall-wrapped instruction-hook test:
        // the hook's error is an ordinary catchable Lua error, so a script
        // could otherwise absorb it and return a normal-looking action.
        let source = r#"
            function on_tick(observation)
                local ok = pcall(function()
                    while true do end
                end)
                return "wait"
            end
        "#;
        let mut op = LiveOperation::deploy(source).expect("script loads");

        let err = op.step().unwrap_err();
        assert!(
            matches!(err, ControllerError::ExecutionLimitExceeded),
            "{err}"
        );
        assert!(op.is_finished());
    }

    #[test]
    fn live_operation_starting_budget_matches_the_fixed_scenario() {
        let op = LiveOperation::deploy("function on_tick(observation) return 'wait' end")
            .expect("valid controller");
        assert_eq!(op.starting_budget(), 15);
    }
}
