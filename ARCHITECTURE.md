# Architecture

This document explains how `herdr-spreader` is put together: the module boundaries, how data flows from a YAML file to actual `herdr` CLI invocations, and the non-obvious design decisions — most of which exist because of bugs found during review, not because they were planned upfront.

## Module map

```
src/
├── main.rs        thin CLI entry point: wires config → engine → backend together
├── cli.rs         clap argument parsing (`apply --file <path>`, `export --file <path> [--force]`)
├── config.rs       YAML → SpreadFile (a list of Workspaces), plus all path resolution (root/cwd/tilde) — the same types also serialize, so export writes exactly what apply parses
├── engine.rs       pure plan logic (`plan_workspace`, `plan_file` — Calculations) + `execute_plan` Action + `BackendOp`/`PaneHandle` Data types
├── export.rs       session export: `capture_snapshot` (Action) reads the live session, `reconstruct_tab_panes`/`build_spread_file` (Calculations) turn pane rects back into a SpreadFile
├── backend/
│   ├── mod.rs      the HerdrBackend trait and its Opts/Created/Error types (write side) plus the read-side state types (WorkspaceState, TabState, PaneEntry, TabLayout)
│   └── cli.rs      CliBackend: HerdrBackend implemented by spawning the herdr binary
└── lib.rs          re-exports the above as a library, so tests/ can link against it
```

The dependency direction is strictly one-way:

```
main.rs ──▶ cli.rs
main.rs ──▶ config.rs
main.rs ──▶ engine.rs ──▶ backend/mod.rs (trait only)
main.rs ──▶ backend/cli.rs ──implements──▶ backend/mod.rs
```

`engine.rs` depends only on the `HerdrBackend` **trait**, never on `CliBackend` directly. That's what makes the engine's expansion logic testable by asserting the `Vec<BackendOp>` plan directly, with no mock of the backend (see [Testing strategy](#testing-strategy)).

## Data flow

```
config.yaml (workspaces: list, required; config.yml also accepted)
    │  config::read_config → fs::read_to_string
    ▼
String (raw file contents, ownership kept by the caller)
    │  (apply path) validate::validate_config(SourceFile { yaml, path })
    │  → SpreadFile::from_str + validate (serde_yaml_ng)
    │  — any finding (error or warning) stops processing (Err/exit 1);
    │    only Ok proceeds to path resolution and the engine
    ▼
SpreadFile { workspaces: Vec<Workspace> }  (raw, as written by the user)
    │  HerdrBackend::list_workspaces → herdr workspace list
    │  → ExistingWorkspaces (label → workspace_id index, built once before
    │    anything is created, so skip decisions are settled up front)
    ▼
SpreadFile + ExistingWorkspaces
    │  config::resolve_paths(file, env, invocation_cwd)
    │  — resolves each workspace's root independently; every workspace gets its
    │    own absolute root, defaulted and ~-expanded exactly as a single-workspace
    │    config would be
    ▼
SpreadFile (every workspace's root defaulted + absolute, ~ expanded everywhere)
    │  engine::plan_file(&file, &existing)
    ▼
Vec<BackendOp> — one flat plan for the whole file (SkipWorkspace /
FocusExistingWorkspace for workspaces that already exist by name)
    │  engine::execute_plan(plan, &mut backend)
    ▼
walk the plan in order; each `create_*` / `split_pane` returns an id that is
threaded into a `HashMap<PaneHandle, String>` registry, so later ops can refer
to earlier-created workspaces, tabs, and panes by their stable handles
    ▼
an actual set of herdr workspaces, with tabs, panes, commands, and focused panes
```

Each stage is a separate module and is unit-tested independently; only the final wiring in `main.rs` is untested by design (see [Testing strategy](#testing-strategy)).

## `export.rs`: rebuilding a layout from pane rects

`export` is the inverse pipeline of `apply`:

```
live herdr session
    │  export::capture_snapshot (Action): workspace list, then per workspace
    │    tab list + pane list, then one pane layout per tab (probed with any
    │    of that tab's panes) — HerdrBackend read methods
    ▼
SessionSnapshot { workspaces → tabs → panes (rect + foreground_cwd + focused) }
    │  export::build_spread_file (pure Calculation)
    │  — reconstruct_tab_panes rebuilds each tab's split sequence from the
    │    pane rects alone, then maps it to config panes with id/from/ratio
    ▼
SpreadFile (the same type apply parses — export never emits a shape apply rejects)
    │  export::render_yaml → serde_yaml_ng::to_string on the config types
    ▼
YAML file, consumed by `apply --file` with no manual editing
```

The reconstruction is the subtle part, and it exists because of two measured herdr behaviors (herdr 0.9.1):

1. **The `splits` array of `pane layout` is not in execution order** (the second-executed split can appear before the first). The topology is therefore rebuilt from the pane rects alone.
2. **A split keeps `round(ratio * extent)` cells of its source pane on the left/top** and puts the new pane in the remainder on the right/bottom; splitting the same pane again divides its *remaining* rect.

From those rules, every full-length ("guillotine") cut of an exactly tiled region corresponds to exactly one split of the region's top-left pane, with `ratio = kept_extent / region_extent`. `reconstruct_tab_panes` validates the panes tile their bounding box exactly (no gaps, no overlaps — otherwise the layout is reported as unrepresentable), then recursively partitions: prefer the smallest strictly-inside vertical cut, and only failing that the smallest horizontal one.

**Emission order is pre-order over the cuts.** Each cut's new pane is emitted *at the cut, before recursing into either half*. The order matters because a split's ratio is measured against the region being cut: if a deeper cut of the source pane executed first, the source pane would already be shrunk and the ratio would land on the wrong extent (emitting the source half's panes before the parent cut's pane — a post-order — breaks exactly this way on a 2×2 grid). Pre-order guarantees every split replays while its source pane still covers the whole region, and that every `from` target precedes its user; the round-trip tests in `tests/cli_backend_integration.rs` replay every exported layout through a simulator with herdr's measured semantics and assert the final rects and cwds come out identical to the snapshot.

Config mapping details, all covered by unit tests: pane 0 (the root) carries no split keys; each later pane carries its ratio at full f64 precision and a `split` key only when not `right`; `from`/`id` (`p1`, `p2`, … by creation index) are emitted only when a pane splits something other than its immediate predecessor; `cwd` is the pane's `foreground_cwd` as an absolute path (an absolute pane cwd replaces the root during apply's path resolution, so no workspace `root` is written); `focus: true` lands only on the focused pane of the active tab, and on the focused workspace; tab labels are always written, even when they equal herdr's default number labels, because a rename to `"1"` is indistinguishable from a never-renamed tab.

Anything that cannot be represented fails loudly via `ExportError`: non-guillotine tilings (pinwheel layouts, manually moved panes), gaps or overlaps, duplicate workspace labels (config names must be unique), and empty sessions. `export` never silently writes a different layout than the one it captured — that is the issue's core requirement.


## The herdr model: workspace → tab → pane

herdr's own object model is workspace → tab → pane, and the config types mirror it directly: `SpreadFile` holds a list of `Workspace`s (one YAML file can now describe several workspaces, not just one), each `Workspace` has `tabs`, and each tab has `panes` (→ panes within that tab). This is intentionally the same shape as tmuxinator's `windows`/`panes` (herdr calls the same concept a "tab" rather than a "window"), since that's the layout DSL most users porting a config will already know.

The one wrinkle: `herdr workspace create` and `herdr tab create` don't just create an empty tab — they *also* create that tab's first ("root") pane, in the same call. That single fact drives most of the complexity in `engine.rs`, described next.

## `engine.rs`: why "first panes" are special-cased

`engine::plan_file(&file, &existing)` is a pure Calculation: it loops over `file.workspaces` in order, and for each one first consults the `ExistingWorkspaces` index — a workspace whose label is already taken (in the live session, or earlier in the same file) plans to a `SkipWorkspace` op, plus a `FocusExistingWorkspace` op when that workspace would have been created with focus (workspace-level `focus: true` or a focused first pane). Only workspaces that pass that check are forwarded to `plan_workspace`, which walks `workspace.tabs[*].panes[*]` and, for each pane, decides which `BackendOp` creates it:

- **A tab's first pane** (`pane_index == 0`) is never created directly — it's the root pane that came back from `create_workspace` (for the first tab) or `create_tab` (for every other tab). There is no `HerdrBackend::create_first_pane` call; it already exists.
- **Every other pane** is created by `split_pane`, splitting off its source pane: the previous pane in the tab, or — when the pane sets `from: <id>` — the earlier pane that declared that `id`. `plan_workspace` resolves ids to `PaneHandle`s through a per-tab map (`resolve_split_source`); `validate` rejects unknown/forward/duplicate ids before planning ever runs.

This asymmetry matters because `create_workspace`, `create_tab`, and `split_pane` each accept a `cwd`/`env` at creation time — but a first pane, having no creation call of its own, has no way to receive a pane-specific `cwd` or `env` through the API. The fix (after several review rounds got this wrong — see [History of the cwd/env bug](#history-of-the-cwden-bug)) is: when a first pane needs a `cwd` or `env` beyond what its workspace/tab baseline already gives it, `engine.rs` prefixes its `run` command with a shell snippet:

```
cd '<resolved dir>' && export KEY='value' && <the user's command>
```

built by `cwd_env_prefix` / `wrap_command_with_cwd_and_env`, with `shell_quote` doing POSIX single-quote escaping so paths and values with spaces or special characters survive intact. If the first pane has *no* command but does need a `cwd`/`env`, the bare `cd && export` line is still run — otherwise a `cwd:`-only entry in the YAML would be silently ignored. If the first pane needs neither, no `run` call happens at all, avoiding a pointless extra `cd .` (`needs_cwd_override`, called inside `plan_workspace`, decides this).

Everything else follows directly from that split:

- **Path composition** (`resolve_cwd` / `combine_cwd`) layers `root → tab.cwd → pane.cwd` top-down: each level is joined onto the previous one unless it's already absolute, in which case it replaces everything above it. `..` is deliberately left alone (the shell resolves it at `cd` time); only literal `.` components are stripped (`normalize_path`).
- **`wait_for` on a pane with no `command`** is silently ignored: `plan_workspace` only emits a `Run` (and its companion `WaitOutput`) when the pane has a `command`. A `wait_for`-only pane produces no ops of its own — there's nothing to wait for. `EngineError` has no variant for this case (it only wraps `BackendError`); the silent-drop is intentional.
- **IDs are never invented.** Every `workspace_id`/`tab_id`/`pane_id` used by a later call is one that came back from an earlier `HerdrBackend` response. herdr's own ids get compacted as things are created/closed, so the engine only ever trusts what the backend just told it.

- **Focus on a skipped workspace is workspace-level.** When an existing workspace is skipped but its config entry would have been focused, `execute_plan` calls `herdr workspace focus <id>` with the id from the pre-flight `workspace list` — no attempt is made to focus a specific pane inside it, because the name-only identity rule cannot map config panes onto live panes.

### Focus

Focus is applied per-call via the `--focus`/`--no-focus` flags herdr accepts on creation operations:

- `CreateWorkspace` passes `--focus` when `workspace.focus || first_pane.focus` is true; otherwise `--no-focus`.
- `CreateTab` (for every tab after the first) passes `--focus` when that tab's first pane has `focus: true`; otherwise `--no-focus`.
- `SplitPane` passes `--focus` when the pane being split off has `focus: true`; otherwise `--no-focus`.

**No `focus_pane` call is ever made by the engine.** The `HerdrBackend::focus_pane` trait method, the `choose_focus_strategy` helper, and the socket path plumbing still exist on `CliBackend`, but they are there for other consumers — `execute_plan` never emits a `BackendOp::FocusPane`.

## `config.rs`: path resolution

This is the part of the codebase that took the most iteration to get right, so it's worth understanding as its own subsystem rather than an afterthought of parsing.

`resolve_paths(file, env, invocation_cwd)` runs once, right after loading the YAML and before the engine ever sees the config, and applies the same three rules to *every workspace in the file independently* — a `SpreadFile` with three workspaces resolves three separate `root`s, one per workspace, each against the same `invocation_cwd`:

1. **`root` is defaulted** to `invocation_cwd` if a workspace's YAML doesn't set one, so every workspace has an absolute anchor to resolve relative paths against — a workspace with no `root` and a tab with `cwd: ./logs` still means something well-defined.
2. **`root` is tilde-expanded and forced absolute** (`expand_root_path`): `~` and `~/...` expand against `$HOME`; anything still relative after that is joined onto `invocation_cwd`.
3. **`tab.cwd` and `pane.cwd` are tilde-expanded but *not* forced absolute** (`expand_tilde` only): they're meant to stay relative to their workspace's `root` (that's the whole point of a tab/pane-level override), so only a literal `~` gets special treatment. The actual `root + tab.cwd + pane.cwd` composition happens later, per-pane, inside `plan_workspace`.

### What "invocation cwd" means, and why it isn't `std::env::current_dir()`

The subtle part: when `herdr-spreader` runs as a herdr plugin action, `std::env::current_dir()` returns **the plugin's own install directory**, not the user's project — herdr sets the child process's cwd to wherever the plugin was linked from, not to the workspace or pane that invoked the action. Naively using it as the resolution base would silently root every layout inside `herdr-spreader`'s own checkout.

The fix in `main.rs`: if `HERDR_PANE_ID` is set (i.e. we were invoked by herdr), query that pane's real shell directory via `herdr pane get <id>` (`CliBackend::query_pane_cwd`, reading the `foreground_cwd` field of the response) and use that as the invocation cwd instead. `std::env::current_dir()` is only used as a fallback — for standalone CLI usage outside of any herdr session, where it's correct.

```
HERDR_PANE_ID env var
    │
    ▼
CliBackend::query_pane_cwd  ──▶  `herdr pane get <id>` → foreground_cwd
    │  (None on any failure: no herdr session, pane gone, unexpected response)
    ▼
fallback: std::env::current_dir()
    │
    ▼
config::resolve_paths(..., invocation_cwd)
```

`query_pane_cwd` deliberately collapses every failure mode into `None` rather than propagating an error — a missing or unreachable herdr session is exactly the "standalone CLI" case the fallback exists for, not something worth failing `apply` over.

### History of the cwd/env bug

This subsystem went through six rounds of review, each fixing a real bug the previous round introduced or missed. It's documented here because the same class of mistake is easy to reintroduce:

1. First panes silently dropped `tab.cwd`/`pane.cwd`/`pane.env` entirely (no creation call to pass them to).
2. The fix (a `cd`-prefixed command) broke on `~` and relative `root`, because a single-quoted `cd` can't trigger shell tilde expansion, and a relative root got joined onto itself.
3. `root` got tilde-expanded, but `tab.cwd`/`pane.cwd` had the identical bug.
4. With no `root` at all, relative `tab.cwd`/`pane.cwd` had no absolute base and leaked through unresolved.
5. A guessed JSON schema for reading the invocation directory out of `HERDR_PLUGIN_CONTEXT_JSON` turned out to be unverifiable and likely wrong — replaced by the documented `pane.get`/`foreground_cwd` query described above.
6. (Neutral review, approved) — confirmed the fixes above compose correctly across first panes, split panes, tab 0 vs. tab N, and standalone-vs-plugin invocation.

If you're touching path resolution, add a test for the *specific* combination you changed (root present/absent, tilde/relative/absolute, tab 0 vs. later tabs, first pane vs. split pane) — this area has a track record of looking correct in isolation while being wrong in combination.

## `backend/`: the seam between logic and I/O

`backend/mod.rs` defines `HerdrBackend` — one method per herdr operation the engine or the exporter needs: the write side (`focus_workspace`, `create_workspace`, `create_tab`, `split_pane`, `run`, `wait_output`, `focus_pane`, `rename_tab`) and the read side (`list_workspaces` returning rich `WorkspaceState`, `list_tabs`, `list_panes`, `pane_layout`), plus the `*Opts`/`*Created` structs, the read-side state types, and the `ExistingWorkspaces` label→id index passed to and returned from them. This trait is the entire contract between "what layout to build or read" (`engine.rs`, `export.rs`) and "how to actually talk to herdr" (`backend/cli.rs`).

`backend/cli.rs` implements that trait by shelling out to the `herdr` binary and parsing its JSON stdout. Internally it's split into two halves on purpose:

- **Pure functions** (`workspace_create_args`, `tab_create_args`, `pane_split_args`, `pane_run_args`, `wait_output_args`, `focus_args`, `rename_tab_args`, `pane_get_args`, `tab_list_args`, `pane_list_args`, `pane_layout_args`, `choose_focus_strategy`, and the matching `parse_*` functions) — no I/O, just `Opts → Vec<String>`, `Option<&str> → FocusStrategy`, and `&str (JSON) → Result<T, BackendError>`. These are unit-tested directly, without spawning anything. The read-side parsers use `#[serde(default)]` Options and fall back to list position for the ordering/`number` fields, so responses from older herdr versions still parse.
- **`CliBackend`** itself — the thin `impl HerdrBackend` that calls those pure functions and then actually runs `std::process::Command`. It's spawned via an argv array (`Command::args`, never a shell string), so there's no shell-injection surface at the herdr-invocation boundary — the only place shell syntax appears is inside the *pane's own command string*, which is sent to that pane's interactive shell via `herdr pane run`, exactly as if the user had typed it themselves.

`CliBackend::resolve_bin` picks the `herdr` binary to spawn: `$HERDR_BIN_PATH` if set (mainly for tests), otherwise `herdr` on `$PATH`.

## Testing strategy

Three layers, each targeting a different seam:

1. **`config.rs` unit tests** — YAML parsing and path resolution, as plain data-in/data-out assertions. No filesystem or process access beyond `read_config`'s own file read.
2. **`engine.rs` unit tests** — split into two seams. First, pure `plan_workspace` / `plan_file` tests assert the produced `Vec<BackendOp>` directly: "for this YAML, exactly these backend operations happen, in this order, with these handles." Second, a tiny `RecordingBackend` (a hand-written `HerdrBackend` that records every call and returns canned ids) covers `execute_plan`'s id threading: it verifies that ids returned from earlier `create_*` / `split_pane` calls are fed back into later ops via the `HashMap<PaneHandle, String>` registry.
3. **`tests/cli_backend_integration.rs`** — exercises the full `plan_file` → `execute_plan` → `CliBackend` → subprocess path, against `tests/fixtures/fake-herdr.sh`, a script that logs the argv it's called with and echoes back canned JSON shaped like real herdr responses. This catches integration bugs the plan-level tests can't see (e.g. an argv-building bug in `backend/cli.rs`). It also includes a plan-pinning test that records the exact `Vec<BackendOp>` produced for a sample file and fails if the plan ever changes unexpectedly.

Layer 3 intentionally does *not* go through `config::resolve_paths` — it builds a `SpreadFile` directly and calls `engine::plan_file` + `engine::execute_plan` on it. `main.rs`'s wiring (config-path resolution, the `pane.get` cwd query, `resolve_paths`) is therefore covered only at the unit level, not end-to-end; keep that in mind if a bug ever turns up specifically in how `main.rs` composes those pieces rather than in any one of them. Export adds a fourth layer here: `capture_snapshot` runs against the fake herdr's canned read-side responses, and a layout simulator (encoding herdr's measured split semantics) replays each exported file's plan and asserts the final pane rects and cwds equal the captured snapshot's — the round-trip proof that `export` followed by `apply` reproduces the original layout.
