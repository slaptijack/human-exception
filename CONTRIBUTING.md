# Contributing

Thanks for your interest in Human Exception.

The project is currently defining its first playable slice. Before investing in a large change, open an issue describing the player problem, the proposed behavior, and why it belongs in that slice.

## Development

Install a current stable Rust toolchain, then run:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Keep commits focused. Add tests for behavior changes, and update player-facing documentation when a change affects the interface or Lua API.

## Design proposals

For gameplay or architecture proposals, explain:

- the player experience being enabled;
- the constraints and tradeoffs;
- the smallest version that can be tested;
- alternatives considered.

Participation in this project is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
