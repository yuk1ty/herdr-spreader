# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--on-existing <create|skip|sync>`, making a re-run idempotent. `skip` leaves a
  workspace whose label already exists untouched; `sync` keeps it and adds only
  the tabs it is missing, matched by label, without disturbing the ones already
  there. The default stays `create`, the previous build-regardless behaviour.

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
