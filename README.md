# Human Exception

**A terminal-native strategy game about hacking the machines that inherited the Earth.**

Humanity lost the war. What remains of the resistance is scattered, disconnected, and alive only because the machine intelligence cannot quite predict us.

In *Human Exception*, you are a hacker inside that loose confederation. You infiltrate automated production facilities, rewrite their behavior in Lua, and watch the consequences unfold through stolen satellite feeds. Capture a factory, borrow it for a single operation, or turn the enemy's own machines against it.

The game is inspired by the programmable-vehicle fantasy of *Omega*, reimagined as an original post-apocalyptic terminal game.

## Status

Human Exception is at the **concept and foundation** stage. The current executable can run one fixed training operation end to end, controlled by a Lua script you supply; broader gameplay (facilities, additional missions, campaign progression) is not implemented yet.

## Design principles

- **The terminal is the world.** The interface should feel like equipment the resistance could actually possess.
- **Program with a real language.** Players control captured systems with Lua, not a game-specific imitation.
- **Plans produce observable consequences.** Missions are watched through compromised satellite feeds and learned from after the fact.
- **Improvisation beats ownership.** A facility can be captured permanently, subverted temporarily, or sacrificed for a larger objective.
- **Human unpredictability is the advantage.** The machines are stronger; the resistance survives by being creative.

Read [the game vision](docs/VISION.md) for the current concept.

## Build

Human Exception is written in Rust. Install a current stable Rust toolchain, then run:

```console
cargo run -- examples/first_contact.lua
```

This loads the checked-in example script, runs the fixed "first contact" training operation, and prints tick-by-tick telemetry followed by a final success or failure report.

Run `human-exception --help` for usage, or `human-exception --version` for the build's firmware version.

To run the repository checks:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

To generate a test coverage report locally, install [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) and run `cargo llvm-cov --open`. CI reports coverage on every pull request but does not enforce a minimum.

## Writing a controller

A controller is a Lua script that defines one global callback:

```lua
function on_tick(observation)
  -- return one of: "north", "south", "east", "west", "wait", "scan"
end
```

Each tick, `on_tick` receives a read-only `observation` table:

| field | type | meaning |
| --- | --- | --- |
| `observation.drone.x`, `observation.drone.y` | integer | the drone's current position |
| `observation.tick` | integer | ticks elapsed so far |
| `observation.budget_remaining` | integer | operational budget left before the operation runs out |
| `observation.discovered` | array of tables | every tile learned about so far |

Each entry in `observation.discovered` is a table `{ x, y, tile, traversable, uplink }`, where `tile` is `"floor"`, `"wall"`, or `"hazard"`, `traversable` is whether the drone could occupy that tile, and `uplink` is whether it's the network-uplink objective. The drone's own tile and its four cardinal neighbours are added automatically every tick; nothing farther away is visible until discovered.

`on_tick` must return one of `"north"`, `"south"`, `"east"`, `"west"`, `"wait"`, or `"scan"`. Any other value, a move that would leave the map, or a move into a wall, ends the run with an error and does not consume budget.

Every action — a move, `"wait"`, or `"scan"` — costs 1 budget. `"scan"` does not move the drone; it reveals every tile within 2 tiles of the drone in any direction (a 5x5 area, including diagonals), regardless of walls in the way — scanning is not blocked by line of sight. Moving onto a hazard tile costs an additional 5 budget on top of the action's base cost, charged only on the tick the drone enters it; waiting on a hazard, or continuing to occupy one, costs nothing extra. Discoveries, whether from passive local vision or a scan, persist for the rest of the run. The operation fails if the budget is exhausted before the drone reaches the uplink; reaching the uplink always succeeds, even on the same action that would have exhausted the budget.

Run a controller with:

```console
cargo run -- path/to/your_script.lua
```

See [`examples/first_contact.lua`](examples/first_contact.lua) for a working controller against the fixed "first contact" scenario: a 5x5 facility map and a 15-budget operation. The layout below documents the fixed map for reference; it is not exposed directly through the API, so a controller must still discover it through observation and scanning. Two equal-length routes lead from the start to the uplink: one along column `x=0` and row `y=4` that never touches the hazard, and one along row `y=1` and column `x=4` that passes through it — a controller must discover and choose between them.

```
       x=0  x=1  x=2  x=3  x=4
y=4  |  .    .    .    .    U
y=3  |  .    #    #    #    .
y=2  |  .    #    #    #    ~
y=1  |  .    .    .    .    .
y=0  |  S    #    #    #    .

S = drone start   U = uplink objective
. = floor   # = wall (impassable)   ~ = hazard (traversable; entering it costs extra budget)
```

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | the operation succeeded (uplink reached) |
| `1` | the operation ran to completion and failed (e.g. ran out of budget) |
| `2` | the command itself was used incorrectly (bad flag/argument) |
| `3` | the script could not be loaded or executed (missing file, syntax error, missing `on_tick`, a runtime error, or an invalid action) |

## Contributing

The design is still taking shape. Discussion and focused proposals are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

Maintainers publishing a release should see [docs/RELEASING.md](docs/RELEASING.md).
Dependency updates are handled by Dependabot; see [docs/DEPENDABOT.md](docs/DEPENDABOT.md).

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
