# Architecture

This document explains how `herdr-spreader` is put together: the module boundaries, how data flows from a YAML file to actual `herdr` CLI invocations, and the non-obvious design decisions — most of which exist because of bugs found during review, not because they were planned upfront.

## Module map

```
src/
├── main.rs        thin CLI entry point: wires config → engine → backend together
├── cli.rs         clap argument parsing (`apply --file <path>`)
├── config.rs       YAML → SpreadFile (a list of Workspaces), plus all path resolution (root/cwd/tilde)
├── engine.rs       pure plan logic (`plan_workspace`, `plan_file`, `summarize` — Calculations) + `execute_plan`/`read_existing_state` Actions + `BackendOp`/`PaneHandle`/`ExistingState`/`OnExisting` Data types
├── backend/
│   ├── mod.rs      the HerdrBackend trait and its Opts/Created/Error types
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
    │  config::resolve_paths(file, env, invocation_cwd)
    │  — resolves each workspace's root independently; every workspace gets its
    │    own absolute root, defaulted and ~-expanded exactly as a single-workspace
    │    config would be
    ▼
SpreadFile (every workspace's root defaulted + absolute, ~ expanded everywhere)
    │  engine::read_existing_state(&file, on_existing, backend)  ── Action
    │  — `workspace list`, plus `tab list` per workspace the file names that
    │    already exists. Reads nothing at all under OnExisting::Create, which is
    │    why the default path still spawns no herdr process for a dry run
    ▼
ExistingState { workspaces, tabs }
    │  engine::plan_file_with_state(&file, &state, on_existing)  ── Calculation
    │  — Create builds everything; Skip emits nothing for a workspace that
    │    exists; Sync binds it with UseWorkspace and adds only the tabs whose
    │    labels are absent
    ▼
Vec<BackendOp> — one flat plan for the whole file
    │  engine::summarize(&file, &state, on_existing)  ── Calculation
    │  — the per-workspace report, classified from the very operations planned,
    │    so it cannot claim something the run does not do
    │  engine::execute_plan(plan, &mut backend)  ── Action
    ▼
walk the plan in order; each `create_*` / `split_pane` returns an id that is
threaded into a `HashMap<PaneHandle, String>` registry, so later ops can refer
to earlier-created workspaces, tabs, and panes by their stable handles.
`UseWorkspace` binds an id that already exists into the same registry, which is
how a synced tab lands in the right workspace
    ▼
an actual set of herdr workspaces, with tabs, panes, commands, and focused panes
```

Each stage is a separate module and is unit-tested independently; only the final wiring in `main.rs` is untested by design (see [Testing strategy](#testing-strategy)).

## The herdr model: workspace → tab → pane

herdr's own object model is workspace → tab → pane, and the config types mirror it directly: `SpreadFile` holds a list of `Workspace`s (one YAML file can now describe several workspaces, not just one), each `Workspace` has `tabs`, and each tab has `panes` (→ panes within that tab). This is intentionally the same shape as tmuxinator's `windows`/`panes` (herdr calls the same concept a "tab" rather than a "window"), since that's the layout DSL most users porting a config will already know.

The one wrinkle: `herdr workspace create` and `herdr tab create` don't just create an empty tab — they *also* create that tab's first ("root") pane, in the same call. That single fact drives most of the complexity in `engine.rs`, described next.

## `engine.rs`: why "first panes" are special-cased

`engine::plan_file(&file)` is a pure Calculation: it loops over `file.workspaces` in order, calling `plan_workspace(workspace)` for each one, and concatenates the resulting `Vec<BackendOp>` into a single flat plan for the whole file. All of the interesting per-layout logic lives in `plan_workspace`, which walks `workspace.tabs[*].panes[*]` and, for each pane, decides which `BackendOp` creates it:

- **A tab's first pane** (`pane_index == 0`) is never created directly — it's the root pane that came back from `create_workspace` (for the first tab) or `create_tab` (for every other tab). There is no `HerdrBackend::create_first_pane` call; it already exists.
- **Every other pane** is created by `split_pane`, splitting off the previous pane in the tab.

This asymmetry matters because `create_workspace`, `create_tab`, and `split_pane` each accept a `cwd`/`env` at creation time — but a first pane, having no creation call of its own, has no way to receive a pane-specific `cwd` or `env` through the API. The fix (after several review rounds got this wrong — see [History of the cwd/env bug](#history-of-the-cwden-bug)) is: when a first pane needs a `cwd` or `env` beyond what its workspace/tab baseline already gives it, `engine.rs` prefixes its `run` command with a shell snippet:

```
cd '<resolved dir>' && export KEY='value' && <the user's command>
```

built by `cwd_env_prefix` / `wrap_command_with_cwd_and_env`, with `shell_quote` doing POSIX single-quote escaping so paths and values with spaces or special characters survive intact. If the first pane has *no* command but does need a `cwd`/`env`, the bare `cd && export` line is still run — otherwise a `cwd:`-only entry in the YAML would be silently ignored. If the first pane needs neither, no `run` call happens at all, avoiding a pointless extra `cd .` (`needs_cwd_override`, called inside `plan_workspace`, decides this).

Everything else follows directly from that split:

- **Path composition** (`resolve_cwd` / `combine_cwd`) layers `root → tab.cwd → pane.cwd` top-down: each level is joined onto the previous one unless it's already absolute, in which case it replaces everything above it. `..` is deliberately left alone (the shell resolves it at `cd` time); only literal `.` components are stripped (`normalize_path`).
- **`wait_for` on a pane with no `command`** is silently ignored: `plan_workspace` only emits a `Run` (and its companion `WaitOutput`) when the pane has a `command`. A `wait_for`-only pane produces no ops of its own — there's nothing to wait for. `EngineError` has no variant for this case (it only wraps `BackendError`); the silent-drop is intentional.
- **IDs are never invented.** Every `workspace_id`/`tab_id`/`pane_id` used by a later call is one that came back from an earlier `HerdrBackend` response. herdr's own ids get compacted as things are created/closed, so the engine only ever trusts what the backend just told it.

### Focus

Focus is applied per-call via the `--focus`/`--no-focus` flags herdr accepts on creation operations:

- `CreateWorkspace` passes `--focus` when `workspace.focus || first_pane.focus` is true; otherwise `--no-focus`.
- `CreateTab` (for every tab after the first) passes `--focus` when that tab's first pane has `focus: true`; otherwise `--no-focus`.
- `SplitPane` passes `--focus` when the pane being split off has `focus: true`; otherwise `--no-focus`.

**No `focus_pane` call is ever made by the engine.** The `HerdrBackend::focus_pane` trait method, the `choose_focus_strategy` helper, and the socket path plumbing still exist on `CliBackend`, but they are there for other consumers — `execute_plan` never emits a `BackendOp::FocusPane`.

## `engine.rs`: planning against what already exists

`apply` originally began, unconditionally, with `herdr workspace create`. That made a second run build the layout a second time, which is fine for a one-shot setup and wrong for a command you run whenever you sit down. `--on-existing` decides what happens when a workspace's `name` matches a label already on the server:

- `Create` (default) — build regardless, duplicating. The behaviour every existing config relies on.
- `Skip` — emit nothing for that workspace.
- `Sync` — bind it and add only the tabs whose labels are absent.

The split between reading and deciding is deliberate and load-bearing:

- **`read_existing_state`** is the only Action. It costs one `workspace list`, plus one `tab list` per workspace the file names that already exists — and nothing at all under `Create`.
- **`plan_workspace_with_state`** is a Calculation over `(layout, ExistingState, OnExisting)`.

Because deciding is pure, `--dry-run` prints exactly the plan that would run under `Skip`/`Sync`, and under `Create` it still spawns no herdr process — the promise the README makes.

Two operations exist only for this:

- **`UseWorkspace`** binds a workspace id that already exists, so the `CreateTab` operations after it target it. Without it the executor's `workspace_id` could only ever come from a `CreateWorkspace`.
- **`FocusWorkspace`** focuses one that already exists — the counterpart, for a workspace that is not being built, of the `--focus` flag a created one would carry. Without it a `focus: true` on an existing workspace would silently do nothing.

Three rules in `plan_sync_workspace` are decisions, not oversights:

- **An existing tab is never touched** — not its panes, not its commands. A pane may be part-way through a build and nothing here can tell.
- **Labels are compared with any leading `[N] ` numbering prefix stripped** (`tab_label_key`). Tab-numbering plugins rewrite every label to `[1] name` after a layout has been applied, and strip the same prefix before re-adding it; comparing the raw strings finds nothing, decides every tab is missing, and duplicates the lot — the exact failure `--on-existing` exists to prevent. Only `[digits]` is stripped, so a user's own `[wip] notes` keeps its identity.
- **An unlabelled tab is skipped.** It could not be recognised on the next run, so syncing it would add another copy every time.
- **The first tab is created like any other.** In a fresh build, tab 0 is the pane that came back from `workspace create` and therefore carries the *workspace* cwd, which is why its command is prefixed with a `cd` (see [why "first panes" are special-cased](#enginers-why-first-panes-are-special-cased)). A synced tab is created by `tab create --cwd`, so it needs no prefix — hence the `root_from_workspace_create` flag threaded into `plan_tab_panes` rather than a bare `tab_index == 0` test.

`summarize` classifies each workspace from the operations planned for it, rather than from the layout or from watching the run. That is what keeps the printed report (`created` / `updated (+N tabs)` / `unchanged` / `skipped`) honest: it is derived from the same plan that executes.

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

`backend/mod.rs` defines `HerdrBackend` — one method per herdr operation the engine needs (`create_workspace`, `create_tab`, `split_pane`, `run`, `wait_output`, `focus_pane`, `rename_tab`, and the three that let a plan account for what is already there: `list_workspaces`, `list_tabs`, `focus_workspace`), plus the `*Opts`/`*Created`/`*Summary` structs passed to and returned from them. This trait is the entire contract between "what layout to build" (`engine.rs`) and "how to actually build it" (`backend/cli.rs`).

`backend/cli.rs` implements that trait by shelling out to the `herdr` binary and parsing its JSON stdout. Internally it's split into two halves on purpose:

- **Pure functions** (`workspace_create_args`, `tab_create_args`, `pane_split_args`, `pane_run_args`, `wait_output_args`, `focus_args`, `rename_tab_args`, `pane_get_args`, `workspace_list_args`, `tab_list_args`, `workspace_focus_args`, `choose_focus_strategy`, and the matching `parse_*` functions) — no I/O, just `Opts → Vec<String>`, `Option<&str> → FocusStrategy`, and `&str (JSON) → Result<T, BackendError>`. These are unit-tested directly, without spawning anything.
- **`CliBackend`** itself — the thin `impl HerdrBackend` that calls those pure functions and then actually runs `std::process::Command`. It's spawned via an argv array (`Command::args`, never a shell string), so there's no shell-injection surface at the herdr-invocation boundary — the only place shell syntax appears is inside the *pane's own command string*, which is sent to that pane's interactive shell via `herdr pane run`, exactly as if the user had typed it themselves.

`CliBackend::resolve_bin` picks the `herdr` binary to spawn: `$HERDR_BIN_PATH` if set (mainly for tests), otherwise `herdr` on `$PATH`.

## Testing strategy

Three layers, each targeting a different seam:

1. **`config.rs` unit tests** — YAML parsing and path resolution, as plain data-in/data-out assertions. No filesystem or process access beyond `read_config`'s own file read.
2. **`engine.rs` unit tests** — split into two seams. First, pure `plan_workspace` / `plan_file` tests assert the produced `Vec<BackendOp>` directly: "for this YAML, exactly these backend operations happen, in this order, with these handles." Second, a tiny `RecordingBackend` (a hand-written `HerdrBackend` that records every call, returns canned ids, and reports back whatever existing workspaces and tabs a test gives it) covers `execute_plan`'s id threading, and the `read_existing_state` → `apply_with_policy` path: it verifies that ids returned from earlier `create_*` / `split_pane` calls are fed back into later ops via the `HashMap<PaneHandle, String>` registry.
3. **`tests/cli_backend_integration.rs`** — exercises the full `plan_file` → `execute_plan` → `CliBackend` → subprocess path, against `tests/fixtures/fake-herdr.sh`, a script that logs the argv it's called with and echoes back canned JSON shaped like real herdr responses. This catches integration bugs the plan-level tests can't see (e.g. an argv-building bug in `backend/cli.rs`). It also includes a plan-pinning test that records the exact `Vec<BackendOp>` produced for a sample file and fails if the plan ever changes unexpectedly.

Layer 3 intentionally does *not* go through `config::resolve_paths` — it builds a `SpreadFile` directly and calls `engine::plan_file` + `engine::execute_plan` on it. `main.rs`'s wiring (config-path resolution, the `pane.get` cwd query, `resolve_paths`) is therefore covered only at the unit level, not end-to-end; keep that in mind if a bug ever turns up specifically in how `main.rs` composes those pieces rather than in any one of them.
