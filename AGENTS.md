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
- If a consequential product decision instead emerges *during* implementation (for example, which of several plausible states should carry a UI affordance), record the decision and its rationale in the issue or a linked design document before continuing — not only in the PR summary. Review should be able to trace a decision to where it was made, rather than trusting the PR's account of it.
- Do not quietly expand scope or bundle unrelated cleanup.
- If you discover unrelated work, propose a separate issue.
- Check parent, child, and blocked-by relationships before starting. Do not implement a blocked issue.

## Delivery shape

Prefer small, independently mergeable implementation issues and usually one PR per issue, over minimizing issue or PR count.

Preferred delivery shape:

**Epic → many small implementation issues → usually one PR per issue**

rather than:

**Epic → a few medium implementation issues → large multi-layer PRs**

- Story points describe product scope; issue and PR boundaries also reflect review scope. A story-point size alone does not imply a change is appropriately sized for one PR.
- 1–3 story points is the normal target for an implementation issue.
- A 4–5 point implementation issue is a decomposition smell: explicitly consider splitting it before implementation begins.
- An implementation issue should have one dominant implementation/review question.
- Every implementation issue should be independently landable and leave `main` healthy.
- Prefer creating additional small issues over planning multiple large PRs under one implementation issue. Multiple PRs for one implementation issue remain possible when circumstances require it, but should be an exception rather than the planned default.
- Prefer prerequisite/seam/refactoring issues when they allow subsequent behavior to land safely and independently.
- Keep tests with the behavior they validate. Avoid opportunistic cleanup or unrelated refactoring in feature PRs.
- Do not use a hard LOC or changed-file limit as a proxy for reviewability. Size metrics may be useful signals, but architectural and behavioral review complexity is the real concern.

If the assigned implementation issue appears too large to satisfy these delivery rules, do not simply implement it. Propose a smaller issue split before proceeding.

Before requesting review, confirm the PR:

- has one dominant review question;
- was considered for further decomposition;
- contains no unrelated cleanup;
- leaves `main` healthy if merged independently.

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
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

These commands must remain aligned with `.github/workflows/ci.yml`. If the project adds another required check, update CI and this file together.

To generate a coverage report locally:

```console
cargo install cargo-llvm-cov
cargo llvm-cov --open
```

CI runs the same tool and posts a coverage summary on pull requests; coverage is informational only and is not a required check.

## Engineering principles

- Keep the simulation core deterministic. The same initial state and ordered inputs must produce the same outputs.
- Keep wall-clock time, uncontrolled randomness, terminal I/O, filesystem access, and other external side effects outside deterministic simulation logic.
- Model game state and actions explicitly. Validate actions at system boundaries and return useful errors.
- Keep the Lua API small, intentional, documented, and compatible with deterministic replay.
- Treat Lua programs as untrusted input. Do not expose host capabilities unless an approved issue explicitly requires them.
- Keep presentation separate from simulation behavior. Terminal output and telemetry should observe state rather than define it.
- When a rendered cue, an advertised shortcut, and the input handling that acts on it all describe the same capability, derive them from one shared availability rule where practical, so they cannot silently disagree about whether that capability is currently available. If they must intentionally differ, state the reason where the divergence is implemented.
- Before a change to publicly reachable Rust symbols (anything reachable through `src/lib.rs`'s public surface) goes to review, assess and note its semver impact (patch, minor, or major) in the issue or PR. Most changes do not touch public API and need no such note.
- Prefer clear Rust and simple data structures over premature abstractions.
- Avoid new dependencies unless they materially simplify the issue. Explain the tradeoff in the PR.
- Do not use `unsafe` Rust without explicit issue scope and a documented safety argument.
- Preserve original terminology, mechanics, code, and assets; inspiration from other games is not permission to copy them.

## Testing

- Add or update tests for every behavior change and bug fix.
- Prefer unit tests for pure simulation rules and integration tests for player-visible flows and subsystem boundaries.
- Test success, invalid input, and meaningful failure paths.
- For state-dependent interaction work, define and verify the meaningful negative and inert states alongside the happy path — for example unfocused panes, narrow layouts, empty or placeholder surfaces, confirmation dialogs, and undersized terminals — not only the case where the interaction succeeds.
- When a compact interaction-state matrix (visible affordance × advertised shortcut × accepted input × permitted state mutation) would materially clarify which combinations are intended, include one in the issue or PR. Simple changes with one obvious state do not need one.
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

- the issue's acceptance criteria are satisfied — before requesting review, trace each acceptance criterion to the specific code or test that demonstrates it, rather than relying on the PR summary's account of what was done;
- exclusions and dependency boundaries are respected;
- relevant tests cover the behavior;
- formatting, Clippy, and all tests pass;
- player-facing and contributor documentation is current;
- the PR links and, when appropriate, closes the issue;
- the PR explains important design choices and known limitations.

When correctness, product intent, or scope is uncertain, stop and ask. Do not guess past a consequential ambiguity.
