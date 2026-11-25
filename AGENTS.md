# AGENTS.md

## General Coding Guide
- Please do not worry about backward compatibility until I provide further instructions.
- Specify patch version if you add a new Rust crate to `Cargo.toml`.
- Follow functional programming style.
  - Prefer to make data immutable.
  - Specify three components: Actions, Calculation, Data (This principle is written in the book "Grokking Simplicity"). Specifically, carefully isolate Actions.
    - Actions: Depend on how many times or when it is run. Also called functions with side-effects, side-effecting functions, impure functions. Examples: Send an email, read from a database, including I/O operations.
    - Calculations: Computations from input to output. Also called pure functions, mathematical functions. Examples: Find the maximum number, check if an email address is valid.
    - Data: Facts about events. Examples: The email address a user gave us, the dollar amount read from a bank’s API.

## Post-Implementation Checks
- After finishing any change to Rust sources (`src/`, `tests/`, `examples/`), run `bash .agents/skills/rust-checks/scripts/rust-check.sh` autonomously and only report the work as done when all four checks pass: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (zero warnings), `cargo build --all-targets`, and `cargo test`. See `.agents/skills/rust-checks/SKILL.md` for the fix loop and escalation rules.
