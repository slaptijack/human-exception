# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/slaptijack/human-exception/compare/v0.2.0...v0.3.0) - 2026-08-17

### Added

- align CLI First Contact report with TUI outcome hierarchy ([#75](https://github.com/slaptijack/human-exception/pull/75))
- apply the outcome hierarchy to unsuccessful First Contact runs ([#74](https://github.com/slaptijack/human-exception/pull/74))
- give successful First Contact clear operation closure ([#73](https://github.com/slaptijack/human-exception/pull/73))
- represent terminal operation conclusions as structured console state ([#72](https://github.com/slaptijack/human-exception/pull/72))
- preserve multiline text pasted into the controller editor ([#64](https://github.com/slaptijack/human-exception/pull/64))
- show F4 recovery hint on failed-operation screen ([#62](https://github.com/slaptijack/human-exception/pull/62))

### Fixed

- add blank-line spacing to the successful After Action report ([#79](https://github.com/slaptijack/human-exception/pull/79))
- advertise Ctrl+V, not Ctrl+Enter, as the Validate shortcut ([#60](https://github.com/slaptijack/human-exception/pull/60))

### Other

- extract a view-keyed pane-scroll mechanism from Help ([#78](https://github.com/slaptijack/human-exception/pull/78))
- define the First Contact outcome and closure contract ([#71](https://github.com/slaptijack/human-exception/pull/71))
- codify small-issue, one-PR delivery guidance ([#55](https://github.com/slaptijack/human-exception/pull/55))

## [0.2.0](https://github.com/slaptijack/human-exception/compare/v0.1.1...v0.2.0) - 2026-08-12

### Added

- complete the edit-deploy-observe-retry gameplay loop ([#53](https://github.com/slaptijack/human-exception/pull/53))
- run and observe reconnaissance live inside the resistance console ([#52](https://github.com/slaptijack/human-exception/pull/52))
- add an in-console Lua editor for the operation controller ([#51](https://github.com/slaptijack/human-exception/pull/51))
- surface resistance signals and target intelligence ([#50](https://github.com/slaptijack/human-exception/pull/50))
- build the interactive resistance-console session shell ([#49](https://github.com/slaptijack/human-exception/pull/49))

### Other

- define resistance console TUI ([#48](https://github.com/slaptijack/human-exception/pull/48))
- fix the v0.1.1 changelog entry and document the gap that caused it ([#39](https://github.com/slaptijack/human-exception/pull/39))

## [0.1.1](https://github.com/slaptijack/human-exception/compare/v0.1.0...v0.1.1) - 2026-08-07

### Added

- deliver the complete reconnaissance operation ([#35](https://github.com/slaptijack/human-exception/pull/35))
- render the discovered facility as a satellite view ([#34](https://github.com/slaptijack/human-exception/pull/34))
- add deterministic hazards and an operational budget ([#33](https://github.com/slaptijack/human-exception/pull/33))
- expose local observations and scanning to Lua ([#32](https://github.com/slaptijack/human-exception/pull/32))
- model a deterministic reconnaissance facility map ([#31](https://github.com/slaptijack/human-exception/pull/31))

### Fixed

- never pass a literal "false" dry_run to release-plz/action ([#37](https://github.com/slaptijack/human-exception/pull/37))

### Other

- enforce Conventional Commits PR titles ([#30](https://github.com/slaptijack/human-exception/pull/30))
- Add code coverage reporting to CI ([#28](https://github.com/slaptijack/human-exception/pull/28))
- Add the first playable command-line operation ([#21](https://github.com/slaptijack/human-exception/pull/21))
- Add the Lua controller boundary for the training simulation ([#19](https://github.com/slaptijack/human-exception/pull/19))
- Add deterministic drone training simulation ([#18](https://github.com/slaptijack/human-exception/pull/18))
- Add an immersive command-line interface with clap ([#17](https://github.com/slaptijack/human-exception/pull/17))
- *(deps)* bump release-plz/action from 1a0c5beec2fe4e91727549b5d2d0715ed28684bb to 2eb1d8bcb770b4c48ccfaad919734b38b51958c9 ([#12](https://github.com/slaptijack/human-exception/pull/12))
- enforce locked dependency resolution ([#15](https://github.com/slaptijack/human-exception/pull/15))
- *(deps)* bump actions/checkout from 4 to 7 ([#13](https://github.com/slaptijack/human-exception/pull/13))
- enable Dependabot for Cargo and GitHub Actions ([#11](https://github.com/slaptijack/human-exception/pull/11))
