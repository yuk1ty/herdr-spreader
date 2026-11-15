# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(config)* optional pane `id` and `from` keys, so a pane can split off any earlier pane in its tab instead of only chaining from the previous one — branching layouts like a full-height sidebar or a 2×2 grid become expressible

## [0.3.0](https://github.com/yuk1ty/herdr-spreader/compare/v0.2.2...v0.3.0) - 2026-11-15

### Added

- *(cli)* warn on stderr for each skipped workspace
- *(engine)* skip existing workspaces by name to make apply idempotent

### Other

- Merge remote-tracking branch 'origin/main' into feat/idempotent-apply
- ignore coding agents related files
- add rust analyzer

### Added

- *(engine)* skip workspaces that already exist by name, making `apply` idempotent: running the same config twice no longer duplicates workspaces. Skipped workspaces that request focus are focused in place via `herdr workspace focus`.
- *(cli)* warn on stderr for every skipped workspace so a run that re-uses existing workspaces is never silent.

### Changed

- *(engine)* `plan_file`/`apply` now take an `ExistingWorkspaces` index; `main.rs` builds it from `herdr workspace list` before planning, so a conflicting pre-flight state is detected before anything is created.

## [0.2.2](https://github.com/yuk1ty/herdr-spreader/compare/v0.2.1...v0.2.2) - 2026-11-14

### Other

- 1.100.0

## [0.2.1](https://github.com/yuk1ty/herdr-spreader/compare/v0.2.0...v0.2.1) - 2026-08-16

### Fixed

- call `herdr pane wait-output` instead of `herdr wait output`

## [0.2.0](https://github.com/yuk1ty/herdr-spreader/compare/v0.1.1...v0.2.0) - 2026-07-16

### Added

- *(cli)* add --dry-run flag to print plan without executing
- *(engine)* introduce BackendOp/PaneHandle Data types and plan_workspace Calculation
- add validation checking phase and remove validate plugin

### Other

- Document --dry-run flag in README
- Add functional programming guidelines with Actions/Calculations/Data principle
- *(ARCHITECTURE)* sync to refactored Actions/Calculations/Data architecture
- *(integration)* add plan-pinning tests for end-to-end plan verification
- *(cli)* thread HERDR_SOCKET_PATH into CliBackend; extract choose_focus_strategy
- *(config)* make path resolution immutable

## [0.1.1](https://github.com/yuk1ty/herdr-spreader/compare/v0.1.0...v0.1.1) - 2026-07-07

### Fixed

- fixed focus true not working on non-first panes
