//! Integration test for Unit 3B / Step 21: `CliBackend` against a fake
//! `herdr` CLI.
//!
//! This is an external `tests/` crate, so it links against the *library*
//! target of `herdr-spreader` (see `src/lib.rs`) rather than the binary.
//!
//! The fake herdr (`tests/fixtures/fake-herdr.sh`) echoes canned JSON
//! matching the real herdr response shapes (see
//! `.claude/user/plan/task-graph.md`, section "3. 共有コンテキスト") based on
//! the subcommand it receives, and appends the argv it was called with to a
//! log file named by `$FAKE_HERDR_LOG`. We drive a `SpreadFile` with two
//! workspaces (a 2-tab/3-pane `demo` workspace and a minimal single-pane
//! `demo2` workspace) through `engine::apply` against a `CliBackend` pointed
//! at that fake script, then assert the logged argv sequence matches the
//! expected order: ids are threaded correctly within each workspace (e.g.
//! the second tab's `pane run` targets the pane id returned by that tab's
//! own `create_tab` response, and the mid-tab split's `pane run` targets the
//! pane id returned by `split_pane`, not a fabricated one), the fake herdr
//! script hands out a fresh workspace id (`wA` then `wB`) per `workspace
//! create` call, and focus is applied via `--focus` on `workspace create`
//! (since `demo2` sets workspace-level `focus: true` and the first workspace
//! sets pane-level `focus: true` on its first pane).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use herdr_spreader::backend::cli::CliBackend;
use herdr_spreader::backend::{SplitOpts, TabOpts, WorkspaceOpts};
use herdr_spreader::config::{Pane, SplitDirection, SpreadFile, Tab, WaitFor, Workspace};
use herdr_spreader::engine::{self, BackendOp, PaneHandle};

/// Serialises integration tests that write to the process-global
/// `FAKE_HERDR_LOG` env var so they do not race.
static FAKE_HERDR_LOCK: Mutex<()> = Mutex::new(());

fn fake_herdr_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-herdr.sh")
}

fn log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake_herdr_log_cli_backend_integration.txt")
}

fn build_spread_file() -> SpreadFile {
    SpreadFile {
        workspaces: vec![
            Workspace {
                name: "demo".to_string(),
                tabs: vec![
                    Tab {
                        label: Some("editor".to_string()),
                        cwd: None,
                        panes: vec![
                            Pane {
                                command: Some("nvim".to_string()),
                                focus: true,
                                ..Default::default()
                            },
                            Pane {
                                command: Some("cargo watch -x test".to_string()),
                                split: SplitDirection::Down,
                                ratio: Some(0.3),
                                wait_for: Some(WaitFor {
                                    pattern: "Compiling".to_string(),
                                    timeout_ms: Some(10000),
                                }),
                                ..Default::default()
                            },
                        ],
                    },
                    Tab {
                        label: Some("server".to_string()),
                        cwd: None,
                        panes: vec![Pane {
                            command: Some("cargo run".to_string()),
                            ..Default::default()
                        }],
                    },
                ],
                ..Default::default()
            },
            Workspace {
                name: "demo2".to_string(),
                focus: true,
                tabs: vec![Tab {
                    label: None,
                    cwd: None,
                    panes: vec![Pane {
                        command: Some("htop".to_string()),
                        ..Default::default()
                    }],
                }],
                ..Default::default()
            },
        ],
    }
}

#[test]
fn should_thread_ids_and_apply_focus_via_create_flags_across_two_workspaces_against_fake_herdr() {
    let _lock = FAKE_HERDR_LOCK.lock().unwrap();
    let log_path = log_path();
    let _ = std::fs::remove_file(&log_path);

    unsafe {
        std::env::set_var("FAKE_HERDR_LOG", &log_path);
    }

    let file = build_spread_file();
    let mut backend = CliBackend::new(fake_herdr_path(), None)
        .with_ready_settings(Duration::ZERO, Duration::ZERO);

    engine::apply(&file, &mut backend).expect("apply against fake herdr should succeed");

    let log_contents = std::fs::read_to_string(&log_path).expect("fake herdr log should exist");
    let logged_lines: Vec<&str> = log_contents.lines().collect();

    let expected_lines = vec![
        "workspace create --label demo --focus",
        "tab rename wA:t1 editor",
        "pane run wA:p1 nvim",
        "pane split wA:p1 --direction down --ratio 0.3 --no-focus",
        "pane run wA:p3 cargo watch -x test",
        "wait output wA:p3 --match Compiling --timeout 10000",
        "tab create --workspace wA --label server --no-focus",
        "pane run wA:p2 cargo run",
        "workspace create --label demo2 --focus",
        "pane run wB:p1 htop",
    ];

    assert_eq!(logged_lines, expected_lines);

    let _ = std::fs::remove_file(&log_path);
}

fn second_pane_log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake_herdr_log_second_pane_focus.txt")
}

fn build_spread_file_with_focus_on_second_pane() -> SpreadFile {
    SpreadFile {
        workspaces: vec![Workspace {
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
        }],
    }
}

#[test]
fn should_focus_second_pane_when_focus_true_is_on_second_pane() {
    let _lock = FAKE_HERDR_LOCK.lock().unwrap();
    let log_path = second_pane_log_path();
    let _ = std::fs::remove_file(&log_path);

    unsafe {
        std::env::set_var("FAKE_HERDR_LOG", &log_path);
    }

    let file = build_spread_file_with_focus_on_second_pane();
    let mut backend = CliBackend::new(fake_herdr_path(), None)
        .with_ready_settings(Duration::ZERO, Duration::ZERO);

    engine::apply(&file, &mut backend).expect("apply against fake herdr should succeed");

    let log_contents = std::fs::read_to_string(&log_path).expect("fake herdr log should exist");
    let logged_lines: Vec<&str> = log_contents.lines().collect();

    let expected_lines = vec![
        "workspace create --label demo --no-focus",
        "tab rename wA:t1 editor",
        "pane run wA:p1 nvim",
        "pane split wA:p1 --direction right --focus",
        "pane run wA:p3 lazygit",
    ];

    assert_eq!(logged_lines, expected_lines);

    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn should_produce_expected_plan_for_two_workspace_fixture_before_execution() {
    let file = build_spread_file();
    let plan = engine::plan_file(&file);
    let expected: Vec<BackendOp> = vec![
        // --- demo ---
        BackendOp::CreateWorkspace(WorkspaceOpts {
            label: "demo".into(),
            cwd: None,
            env: BTreeMap::new(),
            focus: true, // ws.focus == false OR first_pane.focus == true
        }),
        BackendOp::RenameFirstTab {
            label: "editor".into(),
        },
        BackendOp::Run {
            pane: PaneHandle::TabRoot(0),
            command: "nvim".into(),
        },
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
            command: "cargo watch -x test".into(),
        },
        BackendOp::WaitOutput {
            pane: PaneHandle::Split(1),
            wait: WaitFor {
                pattern: "Compiling".into(),
                timeout_ms: Some(10000),
            },
        },
        BackendOp::CreateTab {
            index: 1,
            opts: TabOpts {
                label: Some("server".into()),
                cwd: None,
                focus: false,
            },
        },
        BackendOp::Run {
            pane: PaneHandle::TabRoot(1),
            command: "cargo run".into(),
        },
        // --- demo2 ---
        BackendOp::CreateWorkspace(WorkspaceOpts {
            label: "demo2".into(),
            cwd: None,
            env: BTreeMap::new(),
            focus: true, // ws.focus == true
        }),
        BackendOp::Run {
            pane: PaneHandle::TabRoot(0),
            command: "htop".into(),
        },
    ];
    assert_eq!(plan, expected);
}

#[test]
fn should_produce_expected_plan_for_second_pane_focus_fixture_before_execution() {
    let file = build_spread_file_with_focus_on_second_pane();
    let plan = engine::plan_file(&file);
    let expected: Vec<BackendOp> = vec![
        BackendOp::CreateWorkspace(WorkspaceOpts {
            label: "demo".into(),
            cwd: None,
            env: BTreeMap::new(),
            focus: false, // ws.focus == false AND first_pane.focus == false
        }),
        BackendOp::RenameFirstTab {
            label: "editor".into(),
        },
        BackendOp::Run {
            pane: PaneHandle::TabRoot(0),
            command: "nvim".into(),
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
            command: "lazygit".into(),
        },
    ];
    assert_eq!(plan, expected);
}
