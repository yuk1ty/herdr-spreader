use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::backend::{BackendError, HerdrBackend, SplitOpts, TabOpts, WorkspaceOpts};
use crate::config::{SplitDirection, SpreadFile, WaitFor, Workspace};

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaneHandle {
    TabRoot(usize),
    Split(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackendOp {
    CreateWorkspace(WorkspaceOpts),
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
    execute_plan(&plan_file(file), backend)
}

/// Pick the pane a split branches off: the pane named by `from`, or the
/// previous pane in the tab when `from` is omitted.
///
/// Panics if `from` names an id that no earlier pane in the same tab declared;
/// configs that went through [`crate::validate::validate_config`] reject this
/// case before planning.
fn resolve_split_source(
    pane: &crate::config::Pane,
    previous_handle: &PaneHandle,
    id_handles: &HashMap<&str, PaneHandle>,
) -> PaneHandle {
    pane.from.as_deref().map_or_else(
        || previous_handle.clone(),
        |id| {
            id_handles
                .get(id)
                .unwrap_or_else(|| {
                    panic!(
                        "pane `from: {id}` does not name an earlier pane id in the same tab; configs must be validated before planning"
                    )
                })
                .clone()
        },
    )
}

/// # Panics
///
/// Panics if a pane's `from` names an id that no earlier pane in the same tab
/// declared. Configs that went through [`crate::validate::validate_config`]
/// reject this case before planning.
#[must_use]
pub fn plan_workspace(ws: &Workspace) -> Vec<BackendOp> {
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
        let root_handle = PaneHandle::TabRoot(tab_index);

        if tab_index == 0 {
            if let Some(label) = &tab.label {
                ops.push(BackendOp::RenameFirstTab {
                    label: label.clone(),
                });
            }
        } else {
            let first_pane_focus = tab.panes.first().is_some_and(|p| p.focus);
            ops.push(BackendOp::CreateTab {
                index: tab_index,
                opts: TabOpts {
                    label: tab.label.clone(),
                    cwd: resolve_cwd(ws.root.as_deref(), tab.cwd.as_deref(), None),
                    focus: first_pane_focus,
                },
            });
        }

        let mut previous_handle = root_handle;
        let mut id_handles: HashMap<&str, PaneHandle> = HashMap::new();

        for (pane_index, pane) in tab.panes.iter().enumerate() {
            let pane_handle = if pane_index == 0 {
                previous_handle.clone()
            } else {
                let from_handle = resolve_split_source(pane, &previous_handle, &id_handles);
                let new_handle = PaneHandle::Split(next_split);
                next_split += 1;
                ops.push(BackendOp::SplitPane {
                    from: from_handle,
                    into: new_handle.clone(),
                    opts: SplitOpts {
                        direction: pane.split,
                        ratio: pane.ratio,
                        cwd: resolve_cwd(
                            ws.root.as_deref(),
                            tab.cwd.as_deref(),
                            pane.cwd.as_deref(),
                        ),
                        env: pane.env.clone(),
                        focus: pane.focus,
                    },
                });
                new_handle
            };

            let needs_cwd_override = pane.cwd.is_some() || (tab_index == 0 && tab.cwd.is_some());
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

            if let Some(id) = pane.id.as_deref() {
                id_handles.insert(id, pane_handle.clone());
            }
            previous_handle = pane_handle;
        }
    }

    ops
}

#[must_use]
pub fn plan_file(file: &SpreadFile) -> Vec<BackendOp> {
    file.workspaces.iter().flat_map(plan_workspace).collect()
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
        BackendError, HerdrBackend, SplitOpts, TabCreated, TabOpts, WorkspaceCreated, WorkspaceOpts,
    };
    use crate::config::{Pane, SplitDirection, SpreadFile, Tab, WaitFor, Workspace};
    use crate::engine;

    #[derive(Default)]
    struct RecordingBackend {
        log: Vec<String>,
        next_pane: u32,
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
        fn should_split_two_panes_from_the_same_source_pane_given_from_references() {
            // The motivating branching layout: editor filling the left half,
            // agent full-height on the right, git stacked under the editor.
            let ws = Workspace {
                name: "ws".to_string(),
                tabs: vec![Tab {
                    label: Some("main".to_string()),
                    panes: vec![
                        Pane {
                            id: Some("editor".to_string()),
                            command: Some("nvim".to_string()),
                            focus: true,
                            ..Default::default()
                        },
                        Pane {
                            id: Some("agent".to_string()),
                            from: Some("editor".to_string()),
                            split: SplitDirection::Right,
                            ratio: Some(0.5),
                            command: Some("htop".to_string()),
                            ..Default::default()
                        },
                        Pane {
                            id: Some("git".to_string()),
                            from: Some("editor".to_string()),
                            split: SplitDirection::Down,
                            ratio: Some(0.6),
                            command: Some("lazygit".to_string()),
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
                        label: "ws".to_string(),
                        cwd: None,
                        env: BTreeMap::new(),
                        focus: true,
                    }),
                    BackendOp::RenameFirstTab {
                        label: "main".to_string(),
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
                            ratio: Some(0.5),
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(1),
                        command: "htop".to_string(),
                    },
                    BackendOp::SplitPane {
                        from: PaneHandle::TabRoot(0),
                        into: PaneHandle::Split(2),
                        opts: SplitOpts {
                            direction: SplitDirection::Down,
                            ratio: Some(0.6),
                            cwd: None,
                            env: BTreeMap::new(),
                            focus: false,
                        },
                    },
                    BackendOp::Run {
                        pane: PaneHandle::Split(2),
                        command: "lazygit".to_string(),
                    },
                ]
            );
        }

        #[test]
        fn should_chain_from_previous_pane_when_from_is_omitted_after_a_from_split() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![
                        Pane {
                            id: Some("a".to_string()),
                            ..Default::default()
                        },
                        Pane {
                            from: Some("a".to_string()),
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

            // The third pane has no `from`, so it chains from the previous
            // pane in the list — the Split(1) created right above it.
            assert!(plan.contains(&BackendOp::SplitPane {
                from: PaneHandle::Split(1),
                into: PaneHandle::Split(2),
                opts: SplitOpts::default(),
            }));
        }

        #[test]
        fn should_resolve_from_against_ids_of_the_same_tab_when_tabs_reuse_an_id() {
            let branching_panes = |cmd: &str| {
                vec![
                    Pane {
                        id: Some("editor".to_string()),
                        ..Default::default()
                    },
                    Pane {
                        from: Some("editor".to_string()),
                        command: Some(cmd.to_string()),
                        ..Default::default()
                    },
                ]
            };
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![
                    Tab {
                        panes: branching_panes("one"),
                        ..Default::default()
                    },
                    Tab {
                        panes: branching_panes("two"),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            let plan = engine::plan_workspace(&ws);

            // Each tab's `from: editor` resolves to that tab's own root pane.
            assert!(plan.contains(&BackendOp::SplitPane {
                from: PaneHandle::TabRoot(0),
                into: PaneHandle::Split(1),
                opts: SplitOpts::default(),
            }));
            assert!(plan.contains(&BackendOp::SplitPane {
                from: PaneHandle::TabRoot(1),
                into: PaneHandle::Split(2),
                opts: SplitOpts::default(),
            }));
        }

        #[test]
        #[should_panic(expected = "does not name an earlier pane id")]
        fn should_panic_when_from_references_an_unknown_id_in_an_unvalidated_config() {
            let ws = Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![
                        Pane::default(),
                        Pane {
                            from: Some("missing".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };

            let _ = engine::plan_workspace(&ws);
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
}
