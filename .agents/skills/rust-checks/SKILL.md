---
name: rust-checks
description: Mandatory post-implementation verification suite for herdr-spreader's Rust code — cargo fmt --check, clippy with ZERO warnings, cargo build, and cargo test. Run this AUTONOMOUSLY after any edit to Rust sources (src/, tests/, examples/), before declaring the work done. Use when the user says "check the build", "verify it compiles", or as the final gate of any coding task. For behavioral verification of the real CLI (apply/validate/export against a live or fake herdr), use the verify-herdr-spreader skill instead — this skill covers compile-level quality only.
---

# Rust checks for herdr-spreader

After you finish implementing or editing Rust code, you MUST run the full
check suite yourself — never ask, never skip. The work is only done when all
four checks pass. A suite that was not run is an unfinished task, not a done
task.

## The suite (run it, don't describe it)

From the repository root, run the helper script:

```sh
bash .agents/skills/rust-checks/scripts/rust-check.sh
```

The script is check-only (it never mutates your code) and exits 0 only when
everything passes. It runs, in fast-fail order:

| # | Command | Gate |
|---|---------|------|
| 1 | `cargo fmt --check` | exit 0, no diff output |
| 2 | `cargo clippy --all-targets -- -D warnings` | exit 0 — **zero warnings**, pedantic included |
| 3 | `cargo build --all-targets` | exit 0 |
| 4 | `cargo test` | exit 0, all test targets pass |

The clippy gate is deliberately zero-warnings: the baseline is clean, so any
new warning is a regression introduced by the current change. Escalating
warnings with `-D warnings` turns every one of them into a red exit code, so
"clippy passed" always means exactly one thing.

## When a check fails

1. **fmt fails** — run `cargo fmt` (apply), inspect the diff with
   `git diff`, and re-run the suite. Never hand-format to dodge rustfmt.
2. **clippy fails** — fix the code the way the lint message suggests.
   `cargo clippy --fix --allow-dirty --all-targets` applies only
   machine-applicable suggestions; verify its diff and re-run the suite.
   Suppressing a lint with `#[allow(...)]` needs a written justification in a
   comment on the same line; don't allow a lint just to get to green.
3. **build fails** — fix the compile error; it is always yours if the
   baseline was green.
4. **tests fail** — first read the failure honestly: is the *code* wrong or
   the *test's expectation* outdated by an intended behavior change? Fix the
   code when the test was right; update the test only when the spec changed,
   and say so in your report.

Fix, then re-run the **whole suite** (a clippy fix can break fmt; a test fix
can break clippy). If the same check fails three times after your fixes,
stop and report: the failing command, the exact error output, and what you
tried. Do not loop forever and do not weaken the gates.

## Rules

- Run the suite from the repository root (the script handles `cd` for you).
- Do not report "done" with a partially run suite (fmt only, or tests only).
- Do not bypass the gates with `--offline` skips, feature surgery, or
  `#[allow]` sprinkling.
- Recording evidence: keep the script's final summary (it prints one
  PASS/FAIL line per check) in your report to the user.
- If a failure is caused by something outside this change (e.g. the baseline
  itself is broken on this branch), report that fact with the baseline
  commit — do not silently expand the diff to repair unrelated code.

## What this skill does not cover

`cargo test` proves calculations and CLI contract against fixture fakes.
It does not prove the CLI works against a real herdr session. When a change
touches user-facing behavior (`apply`, `validate`, `export`, plugin paths),
also drive the real CLI per the **verify-herdr-spreader** skill. Both green
together = the change is done.
