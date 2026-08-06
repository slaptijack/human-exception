# AGENTS.md

This file contains repository-wide instructions for coding agents working on Human Exception.

## Project

Human Exception is an open-source, terminal-native strategy game about hackers in a post-apocalyptic resistance infiltrating and reprogramming machine infrastructure.

The player writes real Lua to control captured systems and observes deterministic results through resistance consoles, telemetry, and compromised satellite feeds. Read `README.md` and `docs/VISION.md` before making product or architectural decisions.

The project is early. Prefer the smallest implementation that proves the current issue's player-visible outcome. Do not build generalized systems for hypothetical future requirements.

## Source of work

Every code or repository change must begin with an open GitHub issue.

- Work only on the issue in scope.
- Treat its acceptance criteria and explicit exclusions as the contract.
- If the issue is ambiguous, internally inconsistent, or requires a product decision it does not make, stop and ask in the issue before implementing.
- Do not quietly expand scope or bundle unrelated cleanup.
- If you discover unrelated work, propose a separate issue.
- Check parent, child, and blocked-by relationships before starting. Do not implement a blocked issue.

## Git workflow

All changes land through pull requests. Never commit or push directly to `main`.

- Create a focused branch for the issue.
- Keep commits and the PR limited to that issue.
- Link the issue in the PR and use `Closes #<issue>` when the PR fully completes it.
- Describe what changed, why, how it was tested, and any remaining limitations.
- Do not merge a PR until required checks pass and review feedback is resolved.
- Do not rewrite or discard changes made by others unless the issue explicitly requires it.

## Toolchain and commands

Human Exception uses stable Rust, edition 2024.

Run the application with:

```console
cargo run
```

Before requesting review, run all repository checks:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

These commands must remain aligned with `.github/workflows/ci.yml`. If the project adds another required check, update CI and this file together.

## Engineering principles

- Keep the simulation core deterministic. The same initial state and ordered inputs must produce the same outputs.
- Keep wall-clock time, uncontrolled randomness, terminal I/O, filesystem access, and other external side effects outside deterministic simulation logic.
- Model game state and actions explicitly. Validate actions at system boundaries and return useful errors.
- Keep the Lua API small, intentional, documented, and compatible with deterministic replay.
- Treat Lua programs as untrusted input. Do not expose host capabilities unless an approved issue explicitly requires them.
- Keep presentation separate from simulation behavior. Terminal output and telemetry should observe state rather than define it.
- Prefer clear Rust and simple data structures over premature abstractions.
- Avoid new dependencies unless they materially simplify the issue. Explain the tradeoff in the PR.
- Do not use `unsafe` Rust without explicit issue scope and a documented safety argument.
- Preserve original terminology, mechanics, code, and assets; inspiration from other games is not permission to copy them.

## Testing

- Add or update tests for every behavior change and bug fix.
- Prefer unit tests for pure simulation rules and integration tests for player-visible flows and subsystem boundaries.
- Test success, invalid input, and meaningful failure paths.
- Do not rely on real time, network access, nondeterministic ordering, or uncontrolled randomness in tests.
- Assert structured state and events where possible; use exact terminal-output assertions only when the text itself is part of the interface.
- A change is not complete while tests are skipped, flaky, or weakened to make the build pass.

## Player-facing behavior

- The terminal is the game's native interface, not a graphical game with a text skin.
- Lua is the player-facing programming language; do not introduce a custom imitation language.
- Favor observable cause and effect: player programs should yield understandable actions, telemetry, and failures.
- Error messages should help a player learn and recover without concealing the underlying Lua model.
- Update examples and documentation whenever commands, terminal behavior, configuration, or the Lua API changes.

## Definition of done

Work is complete when:

- the issue's acceptance criteria are satisfied;
- exclusions and dependency boundaries are respected;
- relevant tests cover the behavior;
- formatting, Clippy, and all tests pass;
- player-facing and contributor documentation is current;
- the PR links and, when appropriate, closes the issue;
- the PR explains important design choices and known limitations.

When correctness, product intent, or scope is uncertain, stop and ask. Do not guess past a consequential ambiguity.
