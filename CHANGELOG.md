# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/yuk1ty/herdr-spreader/releases/tag/v0.1.0) - 2026-07-06

### Added

- support XDG_CONFIG_HOME for config file discovery with config.yaml/config.yml
- initial implementation

### Fixed

- use socket API for pane focus to match herdr v0.7.0 CLI syntax

### Other

- add cargo install alternative installation method
- add release-plz workflow for automated releases
- update installation guide with plugin config-dir setup
- pin exact versions for all dependencies
- add patch version guidance for dependencies
- simplify combine_cwd match arm
- improve code idiomaticity and documentation
- add CI pipeline and Rust toolchain configuration
