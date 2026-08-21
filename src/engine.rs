use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::backend::{
    BackendError, HerdrBackend, SplitOpts, TabOpts, TabSummary, WorkspaceOpts, WorkspaceSummary,
};
use crate::config::{SplitDirection, SpreadFile, Tab, WaitFor, Workspace};

fn resolve_cwd(
    root: Option<&Path>,
    tab_cwd: Option<&Path>,
    pane_cwd: Option<&Path>,
) -> Option<PathBuf> {
    let base = combine_cwd(root, tab_cwd);
    combine_cwd(base.as_deref(), pane_cwd)
}

fn combine_cwd(base: Option<&Path>, overlay: Option<&Path>) -> Option<PathBuf> {
    match (base, overlay) {
        (Some(_base), Some(overlay)) if overlay.is_absolute() => Some(normalize_path(overlay)),
        (Some(base), Some(overlay)) => Some(normalize_path(&base.join(overlay))),
        (Some(base), None) => Some(normalize_path(base)),
        (None, Some(overlay)) => Some(normalize_path(overlay)),
        (None, None) => None,
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn cwd_env_prefix(cwd: Option<&Path>, env: &BTreeMap<String, String>) -> Option<String> {
    let mut prefix_parts = Vec::new();
    if let Some(cwd) = cwd {
        prefix_parts.push(format!("cd {}", shell_quote(&cwd.display().to_string())));
    }
    for (key, value) in env {
        prefix_parts.push(format!("export {key}={}", shell_quote(value)));
    }
    if prefix_parts.is_empty() {
        None
    } else {
        Some(prefix_parts.join(" && "))
    }
}

fn wrap_command_with_cwd_and_env(
    command: &str,
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> String {
    match cwd_env_prefix(cwd, env) {
        Some(prefix) => format!("{prefix} && {command}"),
        None => command.to_string(),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            result.push(component.as_os_str());
        }
    }
    result
}

/// What to do about a workspace whose label already exists on the server.
///
/// The default is [`OnExisting::Create`], which is what this tool has always
/// done: build the layout unconditionally, duplicating anything already there.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum OnExisting {
    /// Build the layout regardless, producing a second workspace with the same
    /// label.
    #[default]
    Create,
    /// Leave an existing workspace exactly as it is, and build nothing for it.
    Skip,
    /// Keep an existing workspace and add only the tabs its layout describes
    /// that are not there already, matched by label.
    Sync,
}

/// The server state a plan is made against: which workspaces exist, and which
/// tabs each of them has.
///
/// Reading this is an Action, done once before planning; planning against it is
/// a Calculation, which is what keeps `--dry-run` able to print exactly the
/// plan that would run.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExistingState {
    pub workspaces: Vec<WorkspaceSummary>,
    /// Tabs per workspace id. Absent means "not looked up", which for planning
    /// purposes is the same as a workspace with no tabs this layout knows.
    pub tabs: HashMap<String, Vec<TabSummary>>,
}

impl ExistingState {
    /// The id of the first existing workspace carrying `label`.
    ///
    /// Herdr permits two workspaces with the same label; a layout cannot tell
    /// them apart, so the first one wins and the rest are left alone.
    #[must_use]
    pub fn workspace_id(&self, label: &str) -> Option<&str> {
        self.workspaces
            .iter()
            .find(|w| w.label.as_deref() == Some(label))
            .map(|w| w.workspace_id.as_str())
    }

    /// Whether `workspace_id` already has a tab labelled `label`.
    #[must_use]
    pub fn has_tab(&self, workspace_id: &str, label: &str) -> bool {
        self.tabs
            .get(workspace_id)
            .is_some_and(|tabs| tabs.iter().any(|t| t.label.as_deref() == Some(label)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaneHandle {
    TabRoot(usize),
    Split(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackendOp {
    CreateWorkspace(WorkspaceOpts),
    /// Continue into a workspace that already exists, instead of creating one.
    /// Binds the workspace id the following `CreateTab` operations target.
    UseWorkspace {
        workspace_id: String,
    },
    /// Focus a workspace that already exists — the counterpart, for a skipped
    /// or synced workspace, of the `--focus` flag a created one would carry.
    FocusWorkspace {
        workspace_id: String,
    },
    RenameFirstTab {
        label: String,
    },
    CreateTab {
        index: usize,
        opts: TabOpts,
    },
    SplitPane {
        from: PaneHandle,
        into: PaneHandle,
        opts: SplitOpts,
    },
    Run {
        pane: PaneHandle,
        command: String,
    },
    WaitOutput {
        pane: PaneHandle,
        wait: WaitFor,
    },
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Backend(#[from] BackendError),
}

#[derive(Default)]
struct Executor {
    workspace_id: Option<String>,
    tab0_id: Option<String>,
    panes: HashMap<PaneHandle, String>,
}

/// Execute a plan of backend operations, threading the real ids returned by the
/// backend into the panes referenced by later operations.
///
/// # Errors
///
/// Returns [`EngineError::Backend`] if any backend operation fails.
///
/// # Panics
///
/// Panics if the plan orders operations such that a handle or workspace/tab id
/// is used before it is produced (for example, `RenameFirstTab` before
/// `CreateWorkspace`).
pub fn execute_plan(plan: &[BackendOp], backend: &mut dyn HerdrBackend) -> Result<(), EngineError> {
    let mut ex = Executor::default();
    for op in plan {
        match op {
            BackendOp::CreateWorkspace(opts) => {
                let c = backend.create_workspace(opts)?;
                ex.workspace_id = Some(c.workspace_id);
                ex.tab0_id = Some(c.tab_id);
                ex.panes.insert(PaneHandle::TabRoot(0), c.root_pane_id);
            }
            BackendOp::UseWorkspace { workspace_id } => {
                ex.workspace_id = Some(workspace_id.clone());
                // No tab came with it, so there is no first tab to rename.
                ex.tab0_id = None;
            }
            BackendOp::FocusWorkspace { workspace_id } => {
                backend.focus_workspace(workspace_id)?;
            }
            BackendOp::RenameFirstTab { label } => {
                backend.rename_tab(
                    ex.tab0_id
                        .as_ref()
                        .expect("RenameFirstTab before CreateWorkspace"),
                    label,
                )?;
            }
            BackendOp::CreateTab { index, opts } => {
                let c = backend.create_tab(
                    ex.workspace_id
                        .as_ref()
                        .expect("CreateTab before CreateWorkspace"),
                    opts,
                )?;
                ex.panes.insert(PaneHandle::TabRoot(*index), c.root_pane_id);
            }
            BackendOp::SplitPane { from, into, opts } => {
                let src = ex
                    .panes
                    .get(from)
                    .expect("SplitPane from unknown handle")
                    .clone();
                let new_id = backend.split_pane(&src, opts)?;
                ex.panes.insert(into.clone(), new_id);
            }
            BackendOp::Run { pane, command } => {
                let id = ex.panes.get(pane).expect("Run unknown handle");
                backend.run(id, command)?;
            }
            BackendOp::WaitOutput { pane, wait } => {
                let id = ex.panes.get(pane).expect("Wait unknown handle");
                backend.wait_output(id, wait)?;
            }
        }
    }
    Ok(())
}

/// Apply the workspace layout described by `file`, creating workspaces, tabs,
/// and panes.
///
/// During creation, each `create_workspace`, `create_tab`, and `split_pane`
/// call passes `focus: true` when the corresponding pane has `focus: true` in
/// the config, so the intended pane naturally receives focus. No separate
/// `focus_pane` call is made at the end.
///
/// # Errors
///
/// Returns [`EngineError::Backend`] if any backend operation fails.
pub fn apply(file: &SpreadFile, backend: &mut dyn HerdrBackend) -> Result<(), EngineError> {
    apply_with_policy(file, OnExisting::Create, backend)
}

/// Apply a file, deciding what to do about workspaces that already exist.
///
/// # Errors
///
/// Returns [`EngineError::Backend`] if reading the server state or any
/// operation fails.
pub fn apply_with_policy(
    file: &SpreadFile,
    on_existing: OnExisting,
    backend: &mut dyn HerdrBackend,
) -> Result<(), EngineError> {
    let state = read_existing_state(file, on_existing, backend)?;
    execute_plan(&plan_file_with_state(file, &state, on_existing), backend)
}

#[must_use]
pub fn plan_workspace(ws: &Workspace) -> Vec<BackendOp> {
    plan_workspace_with_state(ws, &ExistingState::default(), OnExisting::Create)
}

/// Plan one workspace against what already exists on the server.
///
/// A Calculation: given the layout, the server's current shape and the chosen
/// policy, it returns the operations to run. Nothing here performs I/O, which
/// is what lets `--dry-run` print the plan that would actually run.
#[must_use]
pub fn plan_workspace_with_state(
    ws: &Workspace,
    state: &ExistingState,
    on_existing: OnExisting,
) -> Vec<BackendOp> {
    let existing = match on_existing {
        OnExisting::Create => None,
        OnExisting::Skip | OnExisting::Sync => state.workspace_id(&ws.name),
    };

    match (on_existing, existing) {
        // Nothing there yet (or the caller asked for the old behaviour): build
        // the whole layout, exactly as before.
        (OnExisting::Create, _) | (_, None) => plan_new_workspace(ws),
        // Already there: leave every pane of it alone. A layout that asks for
        // focus still gets it — the workspace it names does exist, after all.
        (OnExisting::Skip, Some(id)) => {
            if ws.focus {
                vec![BackendOp::FocusWorkspace {
                    workspace_id: id.to_string(),
                }]
            } else {
                Vec::new()
            }
        }
        (OnExisting::Sync, Some(id)) => plan_sync_workspace(ws, state, id),
    }
}

/// The whole layout, built from nothing.
fn plan_new_workspace(ws: &Workspace) -> Vec<BackendOp> {
    let first_pane_focus = ws
        .tabs
        .first()
        .and_then(|t| t.panes.first())
        .is_some_and(|p| p.focus);

    let mut ops = Vec::new();
    ops.push(BackendOp::CreateWorkspace(WorkspaceOpts {
        label: ws.name.clone(),
        cwd: ws.root.clone(),
        env: ws.env.clone(),
        focus: ws.focus || first_pane_focus,
    }));

    let mut next_split = 1usize;

    for (tab_index, tab) in ws.tabs.iter().enumerate() {
        if tab_index == 0 {
            // The workspace already came with a tab and a pane; name it rather
            // than leaving an unnamed stray beside a tab we would create.
            if let Some(label) = &tab.label {
                ops.push(BackendOp::RenameFirstTab {
                    label: label.clone(),
                });
            }
        } else {
            ops.push(BackendOp::CreateTab {
                index: tab_index,
                opts: TabOpts {
                    label: tab.label.clone(),
                    cwd: resolve_cwd(ws.root.as_deref(), tab.cwd.as_deref(), None),
                    focus: tab.panes.first().is_some_and(|p| p.focus),
                },
            });
        }

        plan_tab_panes(
            ws,
            tab,
            tab_index,
            tab_index == 0,
            &mut next_split,
            &mut ops,
        );
    }

    ops
}

/// Only what the existing workspace is missing.
///
/// Additive by design: tabs are matched by label and the ones already there are
/// left untouched, panes and all. A pane in an existing tab may be part-way
/// through a build; nothing here can tell, so nothing here disturbs it.
fn plan_sync_workspace(
    ws: &Workspace,
    state: &ExistingState,
    workspace_id: &str,
) -> Vec<BackendOp> {
    let mut ops = vec![BackendOp::UseWorkspace {
        workspace_id: workspace_id.to_string(),
    }];

    if ws.focus {
        // Emitted before the tab creations so that a pane marked `focus: true`
        // in one of the new tabs still wins, matching how focus resolves when
        // the whole workspace is built at once.
        ops.push(BackendOp::FocusWorkspace {
            workspace_id: workspace_id.to_string(),
        });
    }

    let mut next_split = 1usize;

    for (tab_index, tab) in ws.tabs.iter().enumerate() {
        // An unlabelled tab cannot be recognised on a later run, so syncing it
        // would add another copy every time. Leave it to a full build.
        let Some(label) = tab.label.as_ref() else {
            continue;
        };
        if state.has_tab(workspace_id, label) {
            continue;
        }

        ops.push(BackendOp::CreateTab {
            index: tab_index,
            opts: TabOpts {
                label: Some(label.clone()),
                cwd: resolve_cwd(ws.root.as_deref(), tab.cwd.as_deref(), None),
                focus: tab.panes.first().is_some_and(|p| p.focus),
            },
        });

        // Unlike a fresh build, even tab 0 is created here rather than inherited
        // from `workspace create`, so its root pane already carries the tab cwd
        // and needs no `cd` prefix.
        plan_tab_panes(ws, tab, tab_index, false, &mut next_split, &mut ops);
    }

    ops
}

/// Panes within one tab: the splits, the commands, and the waits.
///
/// `root_from_workspace_create` marks the one case where the tab's first pane
/// was not created with a cwd of its own — the first tab of a freshly created
/// workspace, whose pane came from `workspace create` and so carries the
/// *workspace* cwd. That pane, and only that pane, needs its command prefixed
/// with a `cd`.
fn plan_tab_panes(
    ws: &Workspace,
    tab: &Tab,
    tab_index: usize,
    root_from_workspace_create: bool,
    next_split: &mut usize,
    ops: &mut Vec<BackendOp>,
) {
    let mut previous_handle = PaneHandle::TabRoot(tab_index);

    for (pane_index, pane) in tab.panes.iter().enumerate() {
        let pane_handle = if pane_index == 0 {
            previous_handle.clone()
        } else {
            let new_handle = PaneHandle::Split(*next_split);
            *next_split += 1;
            ops.push(BackendOp::SplitPane {
                from: previous_handle.clone(),
                into: new_handle.clone(),
                opts: SplitOpts {
                    direction: pane.split,
                    ratio: pane.ratio,
                    cwd: resolve_cwd(ws.root.as_deref(), tab.cwd.as_deref(), pane.cwd.as_deref()),
                    env: pane.env.clone(),
                    focus: pane.focus,
                },
            });
            new_handle
        };

        let needs_cwd_override =
            pane.cwd.is_some() || (root_from_workspace_create && tab.cwd.is_some());
        let resolved_cwd = if pane_index == 0 && needs_cwd_override {
            resolve_cwd(ws.root.as_deref(), tab.cwd.as_deref(), pane.cwd.as_deref())
        } else {
            None
        };

        if let Some(command) = &pane.command {
            let command_to_run = if pane_index == 0 {
                wrap_command_with_cwd_and_env(command, resolved_cwd.as_deref(), &pane.env)
            } else {
                command.clone()
            };
            ops.push(BackendOp::Run {
                pane: pane_handle.clone(),
                command: command_to_run,
            });
            if let Some(wait_for) = &pane.wait_for {
                ops.push(BackendOp::WaitOutput {
                    pane: pane_handle.clone(),
                    wait: wait_for.clone(),
                });
            }
        } else if pane_index == 0
            && let Some(prefix) = cwd_env_prefix(resolved_cwd.as_deref(), &pane.env)
        {
            ops.push(BackendOp::Run {
                pane: pane_handle.clone(),
                command: prefix,
            });
        }

        previous_handle = pane_handle;
    }
}

#[must_use]
pub fn plan_file(file: &SpreadFile) -> Vec<BackendOp> {
    plan_file_with_state(file, &ExistingState::default(), OnExisting::Create)
}

/// What `apply` did to one workspace, for reporting back to the person who ran it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Built from nothing, with this many tabs.
    Created(usize),
    /// Already existed; this many tabs were added to it.
    Synced(usize),
    /// Already existed and matched the layout; nothing was done.
    Unchanged,
    /// Already existed and was deliberately left alone.
    Skipped,
}

/// One line of the report: a workspace and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceOutcome {
    pub name: String,
    pub outcome: Outcome,
}

impl WorkspaceOutcome {
    /// The one-line form printed after an apply.
    #[must_use]
    pub fn render(&self) -> String {
        match self.outcome {
            Outcome::Created(tabs) => {
                format!("  created    {} ({})", self.name, plural(tabs, "tab"))
            }
            Outcome::Synced(added) => {
                format!("  updated    {} (+{})", self.name, plural(added, "tab"))
            }
            Outcome::Unchanged => format!("  unchanged  {}", self.name),
            Outcome::Skipped => format!("  skipped    {} (already up)", self.name),
        }
    }
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Describe what a plan will do (or did), workspace by workspace.
///
/// A Calculation over the same inputs the planner takes, so the report can be
/// produced without watching the run — and cannot drift from it, because it is
/// derived from the very operations that run.
#[must_use]
pub fn summarize(
    file: &SpreadFile,
    state: &ExistingState,
    on_existing: OnExisting,
) -> Vec<WorkspaceOutcome> {
    file.workspaces
        .iter()
        .map(|ws| {
            let ops = plan_workspace_with_state(ws, state, on_existing);
            let created_tabs = ops
                .iter()
                .filter(|op| {
                    matches!(
                        op,
                        BackendOp::CreateTab { .. } | BackendOp::RenameFirstTab { .. }
                    )
                })
                .count();
            let outcome = if ops
                .iter()
                .any(|op| matches!(op, BackendOp::CreateWorkspace(_)))
            {
                // A workspace whose first tab carries no label gets no
                // RenameFirstTab, so count the layout rather than the ops.
                Outcome::Created(ws.tabs.len())
            } else if ops
                .iter()
                .any(|op| matches!(op, BackendOp::UseWorkspace { .. }))
            {
                if created_tabs == 0 {
                    Outcome::Unchanged
                } else {
                    Outcome::Synced(created_tabs)
                }
            } else {
                Outcome::Skipped
            };
            WorkspaceOutcome {
                name: ws.name.clone(),
                outcome,
            }
        })
        .collect()
}

/// Plan every workspace in a file against what already exists on the server.
#[must_use]
pub fn plan_file_with_state(
    file: &SpreadFile,
    state: &ExistingState,
    on_existing: OnExisting,
) -> Vec<BackendOp> {
    file.workspaces
        .iter()
        .flat_map(|ws| plan_workspace_with_state(ws, state, on_existing))
        .collect()
}

/// Read the server state a plan needs: the workspaces that exist, and the tabs
/// of the ones this file names.
///
/// The Action half of idempotence. Only the workspaces the layout mentions are
/// looked up, so the cost is one `workspace list` plus one `tab list` per
/// workspace that already exists — and none at all under [`OnExisting::Create`],
/// which is why that path still spawns no herdr process for a dry run.
///
/// # Errors
///
/// Returns [`EngineError::Backend`] if the backend cannot be queried.
pub fn read_existing_state(
    file: &SpreadFile,
    on_existing: OnExisting,
    backend: &mut dyn HerdrBackend,
) -> Result<ExistingState, EngineError> {
    if on_existing == OnExisting::Create {
        return Ok(ExistingState::default());
    }

    let workspaces = backend.list_workspaces()?;
    let mut state = ExistingState {
        workspaces,
        tabs: HashMap::new(),
    };

    if on_existing == OnExisting::Sync {
        let ids: Vec<String> = file
            .workspaces
            .iter()
            .filter_map(|ws| state.workspace_id(&ws.name).map(ToString::to_string))
            .collect();
        for id in ids {
            let tabs = backend.list_tabs(&id)?;
            state.tabs.insert(id, tabs);
        }
    }

    Ok(state)
}

#[must_use]
pub fn render_op(op: &BackendOp) -> String {
    let mut s = String::new();
    match op {
        BackendOp::CreateWorkspace(opts) => {
            write!(s, "workspace create --label {}", shell_quote(&opts.label)).unwrap();
            if let Some(cwd) = &opts.cwd {
                write!(s, " --cwd {}", cwd.display()).unwrap();
            }
            for (k, v) in &opts.env {
                write!(s, " --env {k}={}", shell_quote(v)).unwrap();
            }
            s.push_str(if opts.focus {
                " --focus"
            } else {
                " --no-focus"
            });
        }
        BackendOp::UseWorkspace { workspace_id } => {
            write!(s, "workspace use {workspace_id} (already exists)").unwrap();
        }
        BackendOp::FocusWorkspace { workspace_id } => {
            write!(s, "workspace focus {workspace_id}").unwrap();
        }
        BackendOp::RenameFirstTab { label } => {
            write!(s, "tab rename --label {}", shell_quote(label)).unwrap();
        }
        BackendOp::CreateTab { index, opts } => {
            write!(s, "tab create --index {index}").unwrap();
            if let Some(label) = &opts.label {
                write!(s, " --label {}", shell_quote(label)).unwrap();
            }
            if let Some(cwd) = &opts.cwd {
                write!(s, " --cwd {}", cwd.display()).unwrap();
            }
            s.push_str(if opts.focus {
                " --focus"
            } else {
                " --no-focus"
            });
        }
        BackendOp::SplitPane { from, into, opts } => {
            write!(
                s,
                "pane split {from:?} -> {into:?} --direction {}",
                direction_str(opts.direction)
            )
            .unwrap();
            if let Some(ratio) = opts.ratio {
                write!(s, " --ratio {ratio}").unwrap();
            }
            if let Some(cwd) = &opts.cwd {
                write!(s, " --cwd {}", cwd.display()).unwrap();
            }
            for (k, v) in &opts.env {
                write!(s, " --env {k}={}", shell_quote(v)).unwrap();
            }
            s.push_str(if opts.focus {
                " --focus"
            } else {
                " --no-focus"
            });
        }
        BackendOp::Run { pane, command } => {
            write!(s, "pane run {pane:?} {command}").unwrap();
        }
        BackendOp::WaitOutput { pane, wait } => {
            write!(
                s,
                "wait output {pane:?} --match {}",
                shell_quote(&wait.pattern)
            )
            .unwrap();
            if let Some(timeout) = wait.timeout_ms {
                write!(s, " --timeout {timeout}").unwrap();
            }
        }
    }
    s
}

fn direction_str(d: SplitDirection) -> &'static str {
    match d {
        SplitDirection::Right => "right",
        SplitDirection::Down => "down",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::backend::{
        BackendError, HerdrBackend, SplitOpts, TabCreated, TabOpts, TabSummary, WorkspaceCreated,
        WorkspaceOpts, WorkspaceSummary,
    };
    use crate::config::{Pane, SplitDirection, SpreadFile, Tab, WaitFor, Workspace};
    use crate::engine::{self, BackendOp};

    #[derive(Default)]
    struct RecordingBackend {
        log: Vec<String>,
        next_pane: u32,
        /// Server state the backend reports back, for the idempotence paths.
        existing_workspaces: Vec<WorkspaceSummary>,
        existing_tabs: Vec<TabSummary>,
    }

    impl HerdrBackend for RecordingBackend {
        fn create_workspace(
            &mut self,
            opts: &WorkspaceOpts,
        ) -> Result<WorkspaceCreated, BackendError> {
            self.log.push(format!(
                "create_workspace label={} focus={}",
                opts.label, opts.focus
            ));
            self.next_pane = 2;
            Ok(WorkspaceCreated {
                workspace_id: "w1".into(),
                tab_id: "w1:t1".into(),
                root_pane_id: "w1:p1".into(),
            })
        }

        fn rename_tab(&mut self, tab_id: &str, label: &str) -> Result<(), BackendError> {
            self.log.push(format!("rename_tab {tab_id} -> {label}"));
            Ok(())
        }

        fn create_tab(
            &mut self,
            workspace_id: &str,
            opts: &TabOpts,
        ) -> Result<TabCreated, BackendError> {
            let p = format!("{workspace_id}:p{}", self.next_pane);
            self.next_pane += 1;
            self.log.push(format!(
                "create_tab ws={workspace_id} label={:?} cwd={:?}",
                opts.label, opts.cwd
            ));
            Ok(TabCreated {
                tab_id: format!("{workspace_id}:t2"),
                root_pane_id: p,
            })
        }

        fn split_pane(&mut self, from: &str, opts: &SplitOpts) -> Result<String, BackendError> {
            let p = format!("w1:p{}", self.next_pane);
            self.next_pane += 1;
            self.log.push(format!(
                "split_pane from={from} -> {p} dir={:?} focus={}",
                opts.direction, opts.focus
            ));
            Ok(p)
        }

        fn run(&mut self, pane_id: &str, command: &str) -> Result<(), BackendError> {
            self.log.push(format!("run {pane_id} {command}"));
            Ok(())
        }

        fn wait_output(&mut self, pane_id: &str, wait: &WaitFor) -> Result<(), BackendError> {
            self.log
                .push(format!("wait_output {pane_id} pattern={}", wait.pattern));
            Ok(())
        }

        fn list_workspaces(&mut self) -> Result<Vec<WorkspaceSummary>, BackendError> {
            self.log.push("list_workspaces".to_string());
            Ok(self.existing_workspaces.clone())
        }

        fn list_tabs(&mut self, workspace_id: &str) -> Result<Vec<TabSummary>, BackendError> {
            self.log.push(format!("list_tabs {workspace_id}"));
            Ok(self.existing_tabs.clone())
        }

        fn focus_workspace(&mut self, workspace_id: &str) -> Result<(), BackendError> {
            self.log.push(format!("focus_workspace {workspace_id}"));
            Ok(())
        }

        fn focus_pane(&mut self, _pane_id: &str) -> Result<(), BackendError> {
            unreachable!("execute_plan must never call focus_pane")
        }
    }

    mod execute_tests {
        use super::*;
        use crate::engine::{BackendOp, PaneHandle};

        #[test]
        fn should_execute_create_workspace_and_run_calls_in_plan_order_threading_ids_from_backend()
        {
            let plan = vec![
                BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                }),
                BackendOp::Run {
                    pane: PaneHandle::TabRoot(0),
                    command: "nvim".into(),
                },
            ];
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert_eq!(
                rec.log,
                vec![
                    "create_workspace label=demo focus=false".to_string(),
                    "run w1:p1 nvim".to_string(),
                ]
            );
        }

        #[test]
        fn should_thread_root_pane_id_from_create_workspace_into_a_run_on_tabroot_zero() {
            let plan = vec![
                BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                }),
                BackendOp::Run {
                    pane: PaneHandle::TabRoot(0),
                    command: "nvim".into(),
                },
            ];
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert_eq!(
                rec.log,
                vec![
                    "create_workspace label=demo focus=false".to_string(),
                    "run w1:p1 nvim".to_string(),
                ]
            );
        }

        #[test]
        fn should_thread_split_pane_returned_id_into_subsequent_run() {
            let plan = vec![
                BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                }),
                BackendOp::SplitPane {
                    from: PaneHandle::TabRoot(0),
                    into: PaneHandle::Split(1),
                    opts: SplitOpts::default(),
                },
                BackendOp::Run {
                    pane: PaneHandle::Split(1),
                    command: "watch".into(),
                },
            ];
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert_eq!(
                rec.log,
                vec![
                    "create_workspace label=demo focus=false".to_string(),
                    "split_pane from=w1:p1 -> w1:p2 dir=Right focus=false".to_string(),
                    "run w1:p2 watch".to_string(),
                ]
            );
        }

        #[test]
        fn should_thread_create_tab_indexed_handle_into_run_on_tabroot_index() {
            let plan = vec![
                BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                }),
                BackendOp::CreateTab {
                    index: 1,
                    opts: TabOpts {
                        label: None,
                        cwd: None,
                        focus: false,
                    },
                },
                BackendOp::Run {
                    pane: PaneHandle::TabRoot(1),
                    command: "cargo run".into(),
                },
            ];
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert_eq!(
                rec.log,
                vec![
                    "create_workspace label=demo focus=false".to_string(),
                    "create_tab ws=w1 label=None cwd=None".to_string(),
                    "run w1:p2 cargo run".to_string(),
                ]
            );
        }

        #[test]
        fn should_rename_first_tab_via_rename_first_tab_op_between_create_workspace_and_run() {
            let plan = vec![
                BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                }),
                BackendOp::RenameFirstTab {
                    label: "editor".into(),
                },
                BackendOp::Run {
                    pane: PaneHandle::TabRoot(0),
                    command: "nvim".into(),
                },
            ];
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert_eq!(
                rec.log,
                vec![
                    "create_workspace label=demo focus=false".to_string(),
                    "rename_tab w1:t1 -> editor".to_string(),
                    "run w1:p1 nvim".to_string(),
                ]
            );
        }

        #[test]
        fn should_emit_wait_output_op_after_run_op_in_plan_order() {
            let plan = vec![
                BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                }),
                BackendOp::Run {
                    pane: PaneHandle::TabRoot(0),
                    command: "watch".into(),
                },
                BackendOp::WaitOutput {
                    pane: PaneHandle::TabRoot(0),
                    wait: WaitFor {
                        pattern: "ready".into(),
                        timeout_ms: None,
                    },
                },
            ];
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert_eq!(
                rec.log,
                vec![
                    "create_workspace label=demo focus=false".to_string(),
                    "run w1:p1 watch".to_string(),
                    "wait_output w1:p1 pattern=ready".to_string(),
                ]
            );
        }

        #[test]
        fn should_make_no_backend_calls_given_empty_workspaces_list() {
            let file = SpreadFile { workspaces: vec![] };
            let plan = engine::plan_file(&file);
            assert!(plan.is_empty());
            let mut rec = RecordingBackend::default();
            engine::execute_plan(&plan, &mut rec).unwrap();
            assert!(rec.log.is_empty());
        }
    }

    mod plan_tests {
        use super::*;
        use crate::engine::{BackendOp, PaneHandle};

        #[test]
        fn should_render_minimal_single_tab_workspace_as_create_workspace_then_run() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_rename_first_tab_op_when_workspace_first_tab_has_a_label() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: Some("editor".to_string()),
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::RenameFirstTab {
                        label: "editor".to_string(),
                    },
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_create_tab_op_for_second_tab_threading_tab_index_and_resolved_cwd() {
            let ws = Workspace {
                name: "demo".to_string(),
                root: Some(PathBuf::from("/proj")),
                tabs: vec![
                    Tab {
                        panes: vec![Pane {
                            command: None,
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    Tab {
                        label: Some("server".to_string()),
                        cwd: Some(PathBuf::from("./svc")),
                        panes: vec![Pane {
                            command: Some("cargo run".to_string()),
                            ..Default::default()
                        }],
                    },
                ],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: Some(PathBuf::from("/proj")),
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::CreateTab {
                        index: 1,
                        opts: TabOpts {
                            label: Some("server".to_string()),
                            cwd: Some(PathBuf::from("/proj/svc")),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(1),
                        command: "cargo run".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_split_pane_op_with_direction_ratio_and_handles_for_multi_pane_tab() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![
                        Pane {
                            command: None,
                            ..Default::default()
                        },
                        Pane {
                            command: Some("watch".to_string()),
                            split: SplitDirection::Down,
                            ratio: Some(0.3),
                            ..Default::default()
                        },
                        Pane {
                            command: Some("logs".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Down,
                            ratio: Some(0.3),
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "watch".to_string(),
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::Split(1),
                        into: PaneHandle::Split(2),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(2),
                        command: "logs".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_thread_pane_cwd_and_env_into_split_opts() {
            let mut pane_env = BTreeMap::new();
            pane_env.insert("FOO".to_string(), "bar".to_string());

            let ws = Workspace {
                name: "demo".to_string(),
                root: Some(PathBuf::from("/proj")),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![
                        Pane {
                            command: None,
                            ..Default::default()
                        },
                        Pane {
                            command: Some("watch".to_string()),
                            cwd: Some(PathBuf::from("./sub")),
                            env: pane_env.clone(),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: Some(PathBuf::from("/proj")),
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: Some(PathBuf::from("/proj/sub")),
                            env: pane_env,
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "watch".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_wait_output_op_after_run_op_when_pane_has_wait_for() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![
                        Pane {
                            command: Some("watch".to_string()),
                            wait_for: Some(WaitFor {
                                pattern: "ready".to_string(),
                                timeout_ms: None,
                            }),
                            ..Default::default()
                        },
                        Pane {
                            command: Some("logs".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "watch".to_string(),
                    },
                    BackendOp::WaitOutput {
                        pane: PaneHandle::TabRoot(0),
                        wait: WaitFor {
                            pattern: "ready".to_string(),
                            timeout_ms: None,
                        },
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "logs".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_wrap_first_pane_command_with_cd_and_env_export_given_overrides() {
            let mut pane_env = BTreeMap::new();
            pane_env.insert("FOO".to_string(), "bar".to_string());

            let ws = Workspace {
                name: "demo".to_string(),
                root: Some(PathBuf::from("/proj")),
                tabs: vec![Tab {
                    label: None,
                    cwd: Some(PathBuf::from("./sub")),
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        cwd: Some(PathBuf::from("./inner")),
                        env: pane_env,
                        ..Default::default()
                    }],
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: Some(PathBuf::from("/proj")),
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "cd '/proj/sub/inner' && export FOO='bar' && nvim".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_run_op_with_bare_cd_and_export_when_first_pane_has_cwd_env_but_no_command() {
            let mut pane_env = BTreeMap::new();
            pane_env.insert("FOO".to_string(), "bar".to_string());

            let ws = Workspace {
                name: "demo".to_string(),
                root: Some(PathBuf::from("/proj")),
                tabs: vec![Tab {
                    label: None,
                    cwd: Some(PathBuf::from("./sub")),
                    panes: vec![Pane {
                        command: None,
                        env: pane_env,
                        ..Default::default()
                    }],
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: Some(PathBuf::from("/proj")),
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "cd '/proj/sub' && export FOO='bar'".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_no_run_op_when_first_pane_has_no_command_cwd_or_env() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![Pane {
                        command: None,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".to_string(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                })]
            );
        }

        #[test]
        fn should_resolve_tab_cwd_against_root_in_create_tab_opts_for_second_tab() {
            let ws = Workspace {
                name: "demo".to_string(),
                root: Some(PathBuf::from("/proj")),
                tabs: vec![
                    Tab {
                        panes: vec![Pane {
                            command: None,
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    Tab {
                        label: Some("server".to_string()),
                        cwd: Some(PathBuf::from("./svc")),
                        panes: vec![Pane {
                            command: Some("cargo run".to_string()),
                            ..Default::default()
                        }],
                    },
                ],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: Some(PathBuf::from("/proj")),
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::CreateTab {
                        index: 1,
                        opts: TabOpts {
                            label: Some("server".to_string()),
                            cwd: Some(PathBuf::from("/proj/svc")),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(1),
                        command: "cargo run".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_pass_focus_true_to_create_tab_opts_when_pane_in_non_first_tab_has_focus() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![
                    Tab {
                        panes: vec![Pane {
                            command: None,
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    Tab {
                        label: Some("server".to_string()),
                        panes: vec![Pane {
                            command: Some("cargo run".to_string()),
                            focus: true,
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::CreateTab {
                        index: 1,
                        opts: TabOpts {
                            label: Some("server".to_string()),
                            cwd: None,
                            focus: true,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(1),
                        command: "cargo run".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_pass_no_focus_when_neither_pane_nor_workspace_has_focus() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![
                        Pane {
                            command: Some("nvim".to_string()),
                            ..Default::default()
                        },
                        Pane {
                            command: Some("watch".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "watch".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_one_create_workspace_per_workspace_with_no_focus_for_unfocused_workspaces() {
            let ws1 = Workspace {
                name: "alpha".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let ws2 = Workspace {
                name: "beta".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("cargo run".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let file = SpreadFile {
                workspaces: vec![ws1, ws2],
            };

            let plan = engine::plan_file(&file);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "alpha".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "beta".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "cargo run".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_or_workspace_focus_and_first_pane_focus_into_create_workspace_focus() {
            let ws1 = Workspace {
                name: "alpha".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let ws2 = Workspace {
                name: "beta".to_string(),
                focus: true,
                tabs: vec![Tab {
                    panes: vec![
                        Pane {
                            command: None,
                            ..Default::default()
                        },
                        Pane {
                            command: Some("cargo run".to_string()),
                            focus: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let file = SpreadFile {
                workspaces: vec![ws1, ws2],
            };

            let plan = engine::plan_file(&file);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "alpha".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "beta".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: true,
                    }),
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: true,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "cargo run".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_focus_true_on_create_workspace_for_every_workspace_with_focus_true() {
            let ws1 = Workspace {
                name: "alpha".to_string(),
                focus: true,
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let ws2 = Workspace {
                name: "beta".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("cargo run".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let ws3 = Workspace {
                name: "gamma".to_string(),
                focus: true,
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("logs".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let file = SpreadFile {
                workspaces: vec![ws1, ws2, ws3],
            };

            let plan = engine::plan_file(&file);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "alpha".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: true,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "beta".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "cargo run".to_string(),
                    },
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "gamma".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: true,
                    }),
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "logs".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_pass_focus_true_to_split_pane_opts_when_split_pane_has_focus() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: Some("editor".to_string()),
                    panes: vec![
                        Pane {
                            command: Some("nvim".to_string()),
                            ..Default::default()
                        },
                        Pane {
                            command: Some("lazygit".to_string()),
                            split: SplitDirection::Right,
                            focus: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::RenameFirstTab {
                        label: "editor".to_string(),
                    },
                    BackendOp::Run {
                        pane: PaneHandle::TabRoot(0),
                        command: "nvim".to_string(),
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: true,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "lazygit".to_string(),
                    },
                ]
            );
        }

        #[allow(clippy::too_many_lines)]
        #[test]
        fn should_assign_distinct_split_handles_to_each_split_within_a_workspace() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![
                    Tab {
                        panes: vec![
                            Pane {
                                command: None,
                                ..Default::default()
                            },
                            Pane {
                                command: Some("top".to_string()),
                                ..Default::default()
                            },
                            Pane {
                                command: Some("bottom".to_string()),
                                split: SplitDirection::Down,
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    },
                    Tab {
                        panes: vec![
                            Pane {
                                command: None,
                                ..Default::default()
                            },
                            Pane {
                                command: Some("side".to_string()),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![
                    BackendOp::CreateWorkspace(WorkspaceOpts {
                        label: "demo".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: false,
                    }),
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(1),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "top".to_string(),
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::Split(1),
                        into: PaneHandle::Split(2),
                        opts: SplitOpts {
                            direction: SplitDirection::Down,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(2),
                        command: "bottom".to_string(),
                    },
                    BackendOp::CreateTab {
                        index: 1,
                        opts: TabOpts {
                            label: None,
                            cwd: None,
                            focus: false,
                        },
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(1),
                        into: PaneHandle::Split(3),
                        opts: SplitOpts {
                            direction: SplitDirection::Right,
                            ratio: None,
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(3),
                        command: "side".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_emit_only_create_workspace_when_workspace_has_no_tabs() {
            let ws = Workspace {
                name: "demo".to_string(),
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            assert_eq!(
                plan,
                vec![BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".to_string(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                })]
            );
        }
    }

    mod render_tests {
        use super::*;
        use crate::engine::{BackendOp, PaneHandle, render_op};

        #[test]
        fn should_render_create_workspace_as_human_readable_line() {
            assert_eq!(
                render_op(&BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "demo".into(),
                    cwd: Some(PathBuf::from("/proj")),
                    env: BTreeMap::from([("FOO".to_string(), "bar".to_string())]),
                    focus: true,
                })),
                "workspace create --label 'demo' --cwd /proj --env FOO='bar' --focus"
            );
        }

        #[test]
        fn should_render_create_workspace_without_optional_fields() {
            assert_eq!(
                render_op(&BackendOp::CreateWorkspace(WorkspaceOpts {
                    label: "minimal".into(),
                    cwd: None,
                    env: BTreeMap::new(),
                    focus: false,
                })),
                "workspace create --label 'minimal' --no-focus"
            );
        }

        #[test]
        fn should_render_rename_first_tab_as_human_readable_line() {
            assert_eq!(
                render_op(&BackendOp::RenameFirstTab {
                    label: "editor".into(),
                }),
                "tab rename --label 'editor'"
            );
        }

        #[test]
        fn should_render_create_tab_as_human_readable_line() {
            assert_eq!(
                render_op(&BackendOp::CreateTab {
                    index: 2,
                    opts: TabOpts {
                        label: Some("server".into()),
                        cwd: Some(PathBuf::from("/proj/svc")),
                        focus: true,
                    },
                }),
                "tab create --index 2 --label 'server' --cwd /proj/svc --focus"
            );
        }

        #[test]
        fn should_render_create_tab_without_optional_fields() {
            assert_eq!(
                render_op(&BackendOp::CreateTab {
                    index: 1,
                    opts: TabOpts {
                        label: None,
                        cwd: None,
                        focus: false,
                    },
                }),
                "tab create --index 1 --no-focus"
            );
        }

        #[test]
        fn should_render_split_pane_as_human_readable_line() {
            let mut env = BTreeMap::new();
            env.insert("KEY".to_string(), "value".to_string());
            assert_eq!(
                render_op(&BackendOp::SplitPane {
                    from: PaneHandle::TabRoot(0),
                    into: PaneHandle::Split(1),
                    opts: SplitOpts {
                        direction: SplitDirection::Down,
                        ratio: Some(0.3),
                        cwd: Some(PathBuf::from("/proj/sub")),
                        env,
                        focus: true,
                    },
                }),
                "pane split TabRoot(0) -> Split(1) --direction down --ratio 0.3 --cwd /proj/sub --env KEY='value' --focus"
            );
        }

        #[test]
        fn should_render_split_pane_without_optional_fields() {
            assert_eq!(
                render_op(&BackendOp::SplitPane {
                    from: PaneHandle::TabRoot(0),
                    into: PaneHandle::Split(1),
                    opts: SplitOpts::default(),
                }),
                "pane split TabRoot(0) -> Split(1) --direction right --no-focus"
            );
        }

        #[test]
        fn should_render_run_as_human_readable_line() {
            assert_eq!(
                render_op(&BackendOp::Run {
                    pane: PaneHandle::Split(1),
                    command: "cargo run".into(),
                }),
                "pane run Split(1) cargo run"
            );
        }

        #[test]
        fn should_render_wait_output_as_human_readable_line() {
            assert_eq!(
                render_op(&BackendOp::WaitOutput {
                    pane: PaneHandle::TabRoot(0),
                    wait: WaitFor {
                        pattern: "ready".into(),
                        timeout_ms: Some(5000),
                    },
                }),
                "wait output TabRoot(0) --match 'ready' --timeout 5000"
            );
        }

        #[test]
        fn should_render_wait_output_without_timeout() {
            assert_eq!(
                render_op(&BackendOp::WaitOutput {
                    pane: PaneHandle::TabRoot(0),
                    wait: WaitFor {
                        pattern: "done".into(),
                        timeout_ms: None,
                    },
                }),
                "wait output TabRoot(0) --match 'done'"
            );
        }

        #[test]
        fn should_quote_shell_special_characters_in_rendered_strings() {
            assert_eq!(
                render_op(&BackendOp::RenameFirstTab {
                    label: "it's ok".into(),
                }),
                "tab rename --label 'it'\\''s ok'"
            );
        }
    }

    fn workspace_named(name: &str) -> Workspace {
        Workspace {
            name: name.to_string(),
            root: Some(PathBuf::from("/repo")),
            tabs: vec![
                Tab {
                    label: Some("editor".to_string()),
                    cwd: None,
                    panes: vec![Pane {
                        command: Some("nvim".to_string()),
                        ..Pane::default()
                    }],
                },
                Tab {
                    label: Some("server".to_string()),
                    cwd: None,
                    panes: vec![Pane {
                        command: Some("cargo run".to_string()),
                        ..Pane::default()
                    }],
                },
            ],
            ..Workspace::default()
        }
    }

    fn state_with(label: &str, id: &str, tabs: &[&str]) -> engine::ExistingState {
        let mut state = engine::ExistingState {
            workspaces: vec![WorkspaceSummary {
                workspace_id: id.to_string(),
                label: Some(label.to_string()),
            }],
            ..engine::ExistingState::default()
        };
        state.tabs.insert(
            id.to_string(),
            tabs.iter()
                .map(|t| TabSummary {
                    tab_id: format!("{id}:{t}"),
                    label: Some((*t).to_string()),
                })
                .collect(),
        );
        state
    }

    #[test]
    fn should_plan_the_whole_workspace_when_nothing_with_that_label_exists() {
        let ws = workspace_named("demo");
        let state = state_with("something-else", "wA", &[]);

        for mode in [engine::OnExisting::Skip, engine::OnExisting::Sync] {
            let plan = engine::plan_workspace_with_state(&ws, &state, mode);
            assert!(
                matches!(plan.first(), Some(BackendOp::CreateWorkspace(_))),
                "{mode:?} should build from scratch, got {plan:?}"
            );
        }
    }

    #[test]
    fn should_duplicate_an_existing_workspace_under_the_default_policy() {
        // The behaviour every existing config relies on, kept as the default.
        let ws = workspace_named("demo");
        let state = state_with("demo", "wA", &["editor", "server"]);

        let plan = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Create);

        assert!(matches!(plan.first(), Some(BackendOp::CreateWorkspace(_))));
    }

    #[test]
    fn should_plan_nothing_for_an_existing_workspace_under_skip() {
        let ws = workspace_named("demo");
        let state = state_with("demo", "wA", &["editor"]);

        let plan = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Skip);

        assert!(plan.is_empty(), "expected no operations, got {plan:?}");
    }

    #[test]
    fn should_focus_an_existing_workspace_under_skip_when_the_layout_asks_for_focus() {
        let mut ws = workspace_named("demo");
        ws.focus = true;
        let state = state_with("demo", "wA", &["editor"]);

        let plan = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Skip);

        assert_eq!(
            plan,
            vec![BackendOp::FocusWorkspace {
                workspace_id: "wA".to_string()
            }]
        );
    }

    #[test]
    fn should_add_only_the_missing_tabs_under_sync() {
        let ws = workspace_named("demo");
        let state = state_with("demo", "wA", &["editor"]);

        let plan = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Sync);

        assert_eq!(
            plan[0],
            BackendOp::UseWorkspace {
                workspace_id: "wA".to_string()
            }
        );
        assert!(
            !plan
                .iter()
                .any(|op| matches!(op, BackendOp::CreateWorkspace(_))),
            "sync must never create a second workspace: {plan:?}"
        );
        let created: Vec<&str> = plan
            .iter()
            .filter_map(|op| match op {
                BackendOp::CreateTab { opts, .. } => opts.label.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(created, vec!["server"], "only the missing tab");
        assert!(
            plan.iter().any(|op| matches!(
                op,
                BackendOp::Run { command, .. } if command == "cargo run"
            )),
            "the new tab's command should still run: {plan:?}"
        );
        assert!(
            !plan.iter().any(|op| matches!(
                op,
                BackendOp::Run { command, .. } if command == "nvim"
            )),
            "an existing tab's command must not be re-run: {plan:?}"
        );
    }

    #[test]
    fn should_plan_only_the_binding_under_sync_when_every_tab_already_exists() {
        let ws = workspace_named("demo");
        let state = state_with("demo", "wA", &["editor", "server"]);

        let plan = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Sync);

        assert_eq!(
            plan,
            vec![BackendOp::UseWorkspace {
                workspace_id: "wA".to_string()
            }]
        );
    }

    #[test]
    fn should_leave_an_unlabelled_tab_alone_under_sync() {
        // It could not be recognised on the next run, so syncing it would add
        // another copy every time.
        let mut ws = workspace_named("demo");
        ws.tabs.push(Tab {
            label: None,
            cwd: None,
            panes: vec![Pane {
                command: Some("htop".to_string()),
                ..Pane::default()
            }],
        });
        let state = state_with("demo", "wA", &["editor", "server"]);

        let plan = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Sync);

        assert!(
            !plan.iter().any(|op| matches!(
                op,
                BackendOp::Run { command, .. } if command == "htop"
            )),
            "unlabelled tab should be skipped: {plan:?}"
        );
    }

    #[test]
    fn should_give_a_synced_tabs_first_pane_its_own_cwd_without_a_cd_prefix() {
        // A tab created by `tab create` carries --cwd already; only the first
        // tab of a freshly created workspace inherits the workspace cwd and
        // needs the prefix.
        let ws = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("/repo")),
            tabs: vec![Tab {
                label: Some("api".to_string()),
                cwd: Some(PathBuf::from("./api")),
                panes: vec![Pane {
                    command: Some("just dev".to_string()),
                    ..Pane::default()
                }],
            }],
            ..Workspace::default()
        };
        let state = state_with("demo", "wA", &[]);

        let synced = engine::plan_workspace_with_state(&ws, &state, engine::OnExisting::Sync);
        let fresh = engine::plan_workspace_with_state(
            &ws,
            &engine::ExistingState::default(),
            engine::OnExisting::Create,
        );

        let run_of = |plan: &[BackendOp]| {
            plan.iter()
                .find_map(|op| match op {
                    BackendOp::Run { command, .. } => Some(command.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(run_of(&synced), "just dev");
        assert_eq!(run_of(&fresh), "cd '/repo/api' && just dev");
    }

    #[test]
    fn should_read_no_server_state_at_all_under_the_default_policy() {
        let file = SpreadFile {
            workspaces: vec![workspace_named("demo")],
        };
        let mut backend = RecordingBackend::default();

        engine::read_existing_state(&file, engine::OnExisting::Create, &mut backend).unwrap();

        assert!(
            backend.log.is_empty(),
            "the default path must not query herdr: {:?}",
            backend.log
        );
    }

    #[test]
    fn should_read_tabs_only_for_workspaces_that_already_exist() {
        let file = SpreadFile {
            workspaces: vec![workspace_named("demo"), workspace_named("absent")],
        };
        let mut backend = RecordingBackend {
            existing_workspaces: vec![WorkspaceSummary {
                workspace_id: "wA".to_string(),
                label: Some("demo".to_string()),
            }],
            ..RecordingBackend::default()
        };

        let state =
            engine::read_existing_state(&file, engine::OnExisting::Sync, &mut backend).unwrap();

        assert_eq!(backend.log, vec!["list_workspaces", "list_tabs wA"]);
        assert_eq!(state.workspace_id("demo"), Some("wA"));
        assert_eq!(state.workspace_id("absent"), None);
    }

    #[test]
    fn should_apply_sync_end_to_end_without_creating_a_second_workspace() {
        let file = SpreadFile {
            workspaces: vec![workspace_named("demo")],
        };
        let mut backend = RecordingBackend {
            existing_workspaces: vec![WorkspaceSummary {
                workspace_id: "wA".to_string(),
                label: Some("demo".to_string()),
            }],
            existing_tabs: vec![TabSummary {
                tab_id: "wA:t1".to_string(),
                label: Some("editor".to_string()),
            }],
            ..RecordingBackend::default()
        };

        engine::apply_with_policy(&file, engine::OnExisting::Sync, &mut backend).unwrap();

        assert!(
            !backend
                .log
                .iter()
                .any(|l| l.starts_with("create_workspace")),
            "no workspace should have been created: {:?}",
            backend.log
        );
        assert!(
            backend.log.iter().any(|l| l.contains("create_tab")),
            "the missing tab should have been created: {:?}",
            backend.log
        );
    }

    #[test]
    fn should_match_the_first_of_two_workspaces_sharing_a_label() {
        // herdr allows duplicate labels; a layout cannot tell them apart, so
        // the first wins and the rest are left alone.
        let state = engine::ExistingState {
            workspaces: vec![
                WorkspaceSummary {
                    workspace_id: "wA".to_string(),
                    label: Some("demo".to_string()),
                },
                WorkspaceSummary {
                    workspace_id: "wB".to_string(),
                    label: Some("demo".to_string()),
                },
            ],
            ..engine::ExistingState::default()
        };

        assert_eq!(state.workspace_id("demo"), Some("wA"));
    }

    #[test]
    fn should_ignore_an_unlabelled_existing_workspace_when_matching() {
        let state = engine::ExistingState {
            workspaces: vec![WorkspaceSummary {
                workspace_id: "wA".to_string(),
                label: None,
            }],
            ..engine::ExistingState::default()
        };

        assert_eq!(state.workspace_id("demo"), None);
    }

    #[test]
    fn should_report_what_happened_to_each_workspace() {
        let file = SpreadFile {
            workspaces: vec![
                workspace_named("already-current"),
                workspace_named("needs-a-tab"),
                workspace_named("brand-new"),
            ],
        };
        let mut state = state_with("already-current", "wA", &["editor", "server"]);
        state.workspaces.push(WorkspaceSummary {
            workspace_id: "wB".to_string(),
            label: Some("needs-a-tab".to_string()),
        });
        state.tabs.insert(
            "wB".to_string(),
            vec![TabSummary {
                tab_id: "wB:t1".to_string(),
                label: Some("editor".to_string()),
            }],
        );

        let report = engine::summarize(&file, &state, engine::OnExisting::Sync);

        assert_eq!(
            report.iter().map(|o| o.outcome.clone()).collect::<Vec<_>>(),
            vec![
                engine::Outcome::Unchanged,
                engine::Outcome::Synced(1),
                engine::Outcome::Created(2),
            ]
        );
    }

    #[test]
    fn should_report_an_existing_workspace_as_skipped_under_skip() {
        let file = SpreadFile {
            workspaces: vec![workspace_named("demo")],
        };
        let state = state_with("demo", "wA", &["editor"]);

        let report = engine::summarize(&file, &state, engine::OnExisting::Skip);

        assert_eq!(report[0].outcome, engine::Outcome::Skipped);
        assert!(report[0].render().contains("skipped"));
    }

    #[test]
    fn should_report_every_workspace_as_created_under_the_default_policy() {
        let file = SpreadFile {
            workspaces: vec![workspace_named("demo")],
        };
        let state = state_with("demo", "wA", &["editor", "server"]);

        let report = engine::summarize(&file, &state, engine::OnExisting::Create);

        assert_eq!(report[0].outcome, engine::Outcome::Created(2));
    }

    #[test]
    fn should_render_singular_and_plural_tab_counts() {
        let one = engine::WorkspaceOutcome {
            name: "demo".to_string(),
            outcome: engine::Outcome::Synced(1),
        };
        let many = engine::WorkspaceOutcome {
            name: "demo".to_string(),
            outcome: engine::Outcome::Created(3),
        };

        assert!(one.render().contains("+1 tab)"), "{}", one.render());
        assert!(many.render().contains("(3 tabs)"), "{}", many.render());
    }
}
