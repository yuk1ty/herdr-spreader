# herdr-spreader

**Spin up your whole [herdr](https://herdr.dev) workspace layout — tabs, panes, commands, and all — from a single YAML file.**

If you've used [tmuxinator](https://github.com/tmuxinator/tmuxinator) or [tmuxp](https://github.com/tmux-python/tmuxp) with tmux, this is the same idea for herdr: describe the shape of your project's workspace once, and reproduce it with one command instead of manually splitting panes and typing the same commands every time you sit down to work.

```yaml
workspaces:
  - name: frontend
    root: ~/code/my-project/frontend
    env:
      NODE_ENV: development
    tabs:
      - label: editor
        cwd: ./src
        panes:
          - command: nvim
          - split: down
            ratio: 0.3
            command: npm run dev
  - name: backend
    root: ~/code/my-project/backend
    focus: true
    env:
      RUST_LOG: debug
    tabs:
      - label: editor
        cwd: ./src
        panes:
          - command: nvim
            focus: true
          - split: down
            ratio: 0.3
            command: cargo watch -x test
            wait_for:
              match: "Compiling"
              timeout_ms: 10000
      - label: server
        panes:
          - command: cargo run
```

```
$ herdr-spreader apply
```

...and you get two fully laid-out workspaces: `frontend`, with an `editor` tab running your dev server, and `backend`, with an `editor` tab (your editor plus a live test watcher split underneath it) and a `server` tab running `cargo run` — with focus landing in `backend`, on the pane you marked `focus: true`.

## Features

- **Declarative YAML layouts** — describe tabs and panes once, apply them as many times as you want.
- **Nested pane splits** — split panes `right` or `down` with an optional `ratio`, chained from the previous pane, so you can build arbitrarily deep layouts.
- **Per-pane and per-tab working directories** — set a `root` for the whole layout and override it per tab or per pane; relative paths resolve against their parent, `~` expands to your home directory.
- **Environment variables at every level** — set env vars for the whole workspace or scope them to a single pane.
- **Startup commands with synchronization** — run a command in each pane, and optionally `wait_for` a pattern in its output (with a timeout) before moving on — handy for "don't run the tests until the dev server says it's ready."
- **Explicit focus control** — mark exactly which pane should end up focused after the layout is built.
- **Layouts that live in the repository they describe** — an entry may `include:` another layout file instead of spelling a workspace out inline, so a project's tabs and commands can sit in that project's repo while your global config stays the single entry point.
- **Runs as a herdr plugin or a standalone CLI** — invoke it from herdr's plugin menu, or run the binary directly against any config file.
- **Strict config validation** — unknown YAML keys are rejected at parse time instead of being silently ignored, so typos in your config surface immediately.
- **Dry-run mode** — `--dry-run` prints the operations that *would* be performed (workspace create, tab create, pane split, run, wait…) as a human-readable plan, without invoking `herdr` or touching your session. Useful for previewing a layout before applying it, or for sanity-checking a config you just edited.

## Installation

### As a herdr plugin (recommended)

```bash
herdr plugin install yuk1ty/herdr-spreader
```

This clones the repository, runs `cargo build --release`, and registers the plugin with herdr.

Next, create the plugin's config directory and note where it lives:

```bash
herdr plugin config-dir herdr-spreader
# => ~/.config/herdr/plugins/config/herdr-spreader
```

Place your layout file named `config.yaml` (or `config.yml`) in that directory. The plugin will find it automatically when invoked. From then on, run the layout from within any herdr workspace:

```bash
herdr plugin action invoke herdr-spreader.apply
```

or trigger it from herdr's action menu (`Apply layout`).

### As a standalone CLI

```bash
cargo build --release
./target/release/herdr-spreader apply --file ./config.yaml

# or
cargo install --path .
```

This works outside of a herdr session too, as long as a herdr server is already running (`herdr server` or `brew services start herdr`) and the `herdr` binary is on your `PATH`.

## Usage

```bash
herdr-spreader apply [--file <path>]
```

| Flag | Description |
|---|---|
| `-f, --file <path>` | Path to a layout YAML file. If omitted, searched in `$HERDR_PLUGIN_CONFIG_DIR/` (set automatically when run as a herdr plugin), then `$XDG_CONFIG_HOME/herdr-spreader/`, then `$HOME/.config/herdr-spreader/`. Each directory is checked for `config.yaml` then `config.yml`. Run `herdr plugin config-dir herdr-spreader` to see or create the plugin config directory. |
| `--dry-run` | Print the plan of operations that would be performed (one `BackendOp` per line) without spawning `herdr` or modifying any workspace. Path resolution still runs, so the printed paths reflect your real `root`/`cwd`/`~` expansion — only execution is skipped. |

## Configuration reference

A layout file has four levels: the **file** (top level), **workspaces**, **tabs**, and **panes** (splits within a tab) — mirroring herdr's own workspace → tab → pane model, with the file itself holding a list of workspaces so one YAML file can describe more than one.

### File

| Key | Type | Description |
|---|---|---|
| `workspaces` | list of [Workspace](#workspace) or [Include](#include) (required) | Workspaces to create, in order. |

### Workspace

| Key | Type | Description |
|---|---|---|
| `name` | string (required) | Label for the created workspace. |
| `root` | path | Base working directory for this workspace. Supports `~`. If omitted, defaults to the directory you invoked `herdr-spreader` from (or, when run as a plugin, the workspace/pane you invoked it in — not the plugin's own install directory). |
| `env` | map of string→string | Environment variables applied to the workspace's root pane. |
| `tabs` | list of [Tab](#tab) | Tabs to create, in order. |
| `focus` | boolean | Whether this workspace should be focused after creation. When `true`, the workspace is created with `--focus` (or the workspace's marked pane receives focus via `--focus` on its split/tab operation). Default: `false`. If no workspace or pane has `focus: true`, the user's current focus is preserved. |

### Include

Instead of an inline workspace, an entry in `workspaces` may point at another layout file:

```yaml
# ~/.config/herdr-spreader/config.yaml — the entry point, and nothing more
workspaces:
  - include: ~/code/my-project        # a repo: looks for .herdr-spreader.yaml inside
  - include: ~/code/api/layout.yaml   # or name the file directly
  - include: ~/code/occasional
    optional: true                    # not cloned on this machine? skip it
  - name: scratch                     # inline workspaces still work, mixed freely
    tabs:
      - label: notes
        panes:
          - command: nvim
```

```yaml
# ~/code/my-project/.herdr-spreader.yaml — lives in the repo, names no machine
workspaces:
  - name: my-project
    tabs:
      - label: server
        cwd: ./api          # relative to THIS file, not to wherever you ran the command
        panes:
          - command: just dev
```

| Key | Type | Description |
|---|---|---|
| `include` | path (required) | A layout file, or a directory holding one. Relative paths resolve against the directory of the file doing the including — never the invocation directory — so a checkout can move without the file changing. `~` expands to your home directory. |
| `optional` | boolean | Skip silently when the target does not exist, instead of failing. For a repository that is not checked out on this machine. Default: `false`. |

A worked pair of files is in [`examples/config-with-includes.yaml`](./examples/config-with-includes.yaml) and [`examples/repo-layout.yaml`](./examples/repo-layout.yaml).

The included file is an ordinary layout file: it has its own `workspaces` list, it can include others in turn, and its workspaces are spliced into the including file's list at that position, in order.

**Relative paths inside an included file resolve against that file's own directory.** A workspace with no `root` gets the directory of the file that declared it, and every relative tab/pane `cwd` layers on top of that. This is what lets a repository's layout file contain no absolute paths at all and still work on any machine, and in any worktree of that repository. The *top-level* file keeps the old behaviour — its relative paths resolve against the directory you invoked the command from — so existing configs are unaffected.

Three things are refused rather than guessed at:

- **Cycles.** A file that includes itself, directly or through a chain, is an error naming the loop.
- **Chains deeper than 16 files**, as a backstop.
- **Duplicate workspace names**, including two different repositories that both call their workspace `dev` — the layouts would be indistinguishable once built.

A missing include is an error unless it is marked `optional`, and a parse error names the file it came from rather than the file you invoked.

**On trust:** an included file can run commands on your machine, so treat adding an `include:` the way you would treat `direnv allow` — a deliberate, per-repository decision. Includes are never discovered by walking the filesystem; the only files read are the ones your config names, which makes that config the allowlist.

### Tab

| Key | Type | Description |
|---|---|---|
| `label` | string | Tab name. The first tab renames herdr's default tab instead of creating a new one. |
| `cwd` | path | Working directory for this tab's panes, relative to `root` unless it starts with `~` or `/`. |
| `panes` | list of [Pane](#pane) | Panes to create in this tab, in order. The first pane reuses the tab's root pane; every subsequent pane is created by splitting the previous one. |

### Pane

| Key | Type | Description |
|---|---|---|
| `command` | string | Shell command to run in this pane once it's created. |
| `cwd` | path | Working directory for this pane, relative to the tab's `cwd` (and, transitively, `root`) unless it starts with `~` or `/`. |
| `env` | map of string→string | Environment variables scoped to this pane. |
| `split` | `right` \| `down` | Direction to split from the previous pane. Ignored for a tab's first pane. Default: `right`. |
| `ratio` | float | Size ratio for the split (e.g. `0.3` gives the new pane 30% of the space). |
| `wait_for.match` | string | Substring to wait for in the pane's output after running `command`, before moving on to the next pane. |
| `wait_for.timeout_ms` | integer | How long to wait for the match, in milliseconds. |
| `focus` | boolean | Mark this pane to receive focus when it is created. When `true`, the pane's creation operation (`pane split` or `tab create`) is called with `--focus`, so the intended pane naturally receives focus during layout building. If no pane in the file has `focus: true`, the user's current focus is preserved. |

Setting `wait_for` on a pane with no `command` is a configuration error and `apply` will fail — there's nothing to wait for output from.

### Path resolution

Paths compose top-down: `root` → tab `cwd` → pane `cwd`, each relative override layered on top of the last, with `..` left for the shell to resolve and `~` expanded against your home directory at every level. An absolute path at any level replaces everything above it.

## How it works

`herdr-spreader` doesn't call any private herdr API — it drives the same `herdr` CLI you'd use by hand. Any `include:` entries are expanded first, each file's paths resolved against its own directory, giving one flat list of workspaces; then steps 1-5 below run for each workspace in that list, in order. Focus is not deferred to a final step; instead, when a pane is marked `focus: true`, its creation operation (`workspace create`, `tab create`, or `pane split`) is called with `--focus`, so the intended pane naturally receives focus during layout building:

1. `herdr workspace create` — creates the workspace and its first tab/pane (with `--focus` if the first pane or the workspace is marked for focus).
2. For each subsequent tab, `herdr tab create` — creates a new tab (with `--focus` if the first pane in that tab is marked).
3. For each pane after the first in a tab, `herdr pane split` — splits off a new pane in the requested direction (with `--focus` if that pane is marked).
4. `herdr pane run` — runs the configured command in each pane. A tab's or workspace's first pane can't be created with a working directory or environment variables the way split panes can, so `herdr-spreader` prefixes the command with `cd <dir> && export KEY=VAL && ...` for those panes.
5. `herdr wait output` — for panes with `wait_for`, blocks until the pattern appears before continuing.

Every operation explicitly passes either `--focus` or `--no-focus`, so focus behaviour is deterministic: the final focused pane is the last one created with `--focus` (the pane marked `focus: true`, or the last workspace with `focus: true` and no pane-level override).

Everything is threaded through the IDs each herdr command actually returns — nothing is guessed or hardcoded — so the layout is built correctly however herdr happens to number workspaces, tabs, and panes at runtime.

## Development

See [ARCHITECTURE.md](./ARCHITECTURE.md) for how the codebase is put together — module boundaries, data flow, and the reasoning behind the trickier parts (path resolution in particular).

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt

# Iterate on the plugin against a real herdr instance
herdr plugin link ./
herdr plugin action invoke herdr-spreader.apply
```

The test suite covers YAML parsing, layout expansion (against a mock backend), and CLI argument/response handling, plus one integration test that drives the full pipeline against a fake `herdr` binary fixture.

## License

[MIT](./LICENSE)
