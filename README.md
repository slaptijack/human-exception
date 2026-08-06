# Human Exception

**A terminal-native strategy game about hacking the machines that inherited the Earth.**

Humanity lost the war. What remains of the resistance is scattered, disconnected, and alive only because the machine intelligence cannot quite predict us.

In *Human Exception*, you are a hacker inside that loose confederation. You infiltrate automated production facilities, rewrite their behavior in Lua, and watch the consequences unfold through stolen satellite feeds. Capture a factory, borrow it for a single operation, or turn the enemy's own machines against it.

The game is inspired by the programmable-vehicle fantasy of *Omega*, reimagined as an original post-apocalyptic terminal game.

## Status

Human Exception is at the **concept and foundation** stage. The current executable is only a project bootstrap; gameplay is not implemented yet.

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
cargo run
```

To run the repository checks:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

## Contributing

The design is still taking shape. Discussion and focused proposals are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
