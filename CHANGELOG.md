# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1](https://github.com/slaptijack/human-exception/compare/v0.4.0...v0.4.1) - 2026-08-26

### Added

- *(controller)* route focused Controller commands into the editor ([#118](https://github.com/slaptijack/human-exception/pull/118))
- *(controller)* render Controller source through the editor widget ([#117](https://github.com/slaptijack/human-exception/pull/117))
- *(controller)* introduce the authoritative Controller document adapter ([#116](https://github.com/slaptijack/human-exception/pull/116))
- *(controller)* adopt ratatui-code-editor as the editor foundation ([#115](https://github.com/slaptijack/human-exception/pull/115))

### Fixed

- *(controller)* sanitize control characters before they reach ratatui ([#123](https://github.com/slaptijack/human-exception/pull/123))

### Other

- *(tui)* describe the editor contract as shipped, not pending ([#124](https://github.com/slaptijack/human-exception/pull/124))
- *(controller)* prove comfortable editing at supported terminal sizes ([#121](https://github.com/slaptijack/human-exception/pull/121))
- *(controller)* prove the editor swap preserves gameplay and provenance ([#120](https://github.com/slaptijack/human-exception/pull/120))
- *(controller)* close bracketed-paste acceptance-criteria gaps ([#119](https://github.com/slaptijack/human-exception/pull/119))
- prove and select ratatui-textarea as the editor foundation ([#114](https://github.com/slaptijack/human-exception/pull/114))
- define the integrated Controller editor contract ([#113](https://github.com/slaptijack/human-exception/pull/113))
- move LCOV reporting out of the blocking CI check ([#111](https://github.com/slaptijack/human-exception/pull/111))

## [0.4.0](https://github.com/slaptijack/human-exception/compare/v0.3.0...v0.4.0) - 2026-08-18

### Added

- render a non-color focus marker on the active pane's title ([#103](https://github.com/slaptijack/human-exception/pull/103))
- key scroll state and clamping by pane identity ([#102](https://github.com/slaptijack/human-exception/pull/102))
- route pane-local input through focused panes ([#101](https://github.com/slaptijack/human-exception/pull/101))
- make F8 move focus and drive responsive pane visibility ([#100](https://github.com/slaptijack/human-exception/pull/100))
- model semantic pane identity and per-view focus ([#99](https://github.com/slaptijack/human-exception/pull/99))

### Fixed

- [**breaking**] restrict console module to its console::run entry point ([#108](https://github.com/slaptijack/human-exception/pull/108))
- keep focus affordances inert on single-content console surfaces ([#105](https://github.com/slaptijack/human-exception/pull/105))

### Other

- codify interaction-state and public-API review guidance ([#110](https://github.com/slaptijack/human-exception/pull/110))
- define the console pane-focus interaction contract ([#98](https://github.com/slaptijack/human-exception/pull/98))

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
