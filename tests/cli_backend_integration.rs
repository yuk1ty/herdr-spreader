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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use herdr_spreader::backend::cli::CliBackend;
use herdr_spreader::backend::{
    ExistingWorkspaces, HerdrBackend, Rect, SplitOpts, TabOpts, WorkspaceOpts, WorkspaceSummary,
};
use herdr_spreader::config::{
    Pane, SplitDirection, SpreadFile, Tab, WaitFor, Workspace, resolve_paths,
};
use herdr_spreader::engine::{self, BackendOp, PaneHandle};
use herdr_spreader::export::{self, PaneSnapshot, SessionSnapshot, TabSnapshot, WorkspaceSnapshot};
use herdr_spreader::validate;

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
    let mut backend = CliBackend::new(fake_herdr_path(), None);

    engine::apply(&file, &ExistingWorkspaces::default(), &mut backend)
        .expect("apply against fake herdr should succeed");

    let log_contents = std::fs::read_to_string(&log_path).expect("fake herdr log should exist");
    let logged_lines: Vec<&str> = log_contents.lines().collect();

    let expected_lines = vec![
        "workspace create --label demo --focus",
        "tab rename wA:t1 editor",
        "pane run wA:p1 nvim",
        "pane split wA:p1 --direction down --ratio 0.3 --no-focus",
        "pane run wA:p3 cargo watch -x test",
        "pane wait-output wA:p3 --match Compiling --timeout 10000",
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
    let mut backend = CliBackend::new(fake_herdr_path(), None);

    engine::apply(&file, &ExistingWorkspaces::default(), &mut backend)
        .expect("apply against fake herdr should succeed");

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

fn branching_log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake_herdr_log_branching_from.txt")
}

fn build_spread_file_with_branching_from() -> SpreadFile {
    SpreadFile {
        workspaces: vec![Workspace {
            name: "ws".to_string(),
            tabs: vec![Tab {
                label: Some("main".to_string()),
                cwd: None,
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
            }],
            ..Default::default()
        }],
    }
}

#[test]
fn should_split_both_from_panes_off_the_tab_root_pane_against_fake_herdr() {
    let _lock = FAKE_HERDR_LOCK.lock().unwrap();
    let log_path = branching_log_path();
    let _ = std::fs::remove_file(&log_path);

    unsafe {
        std::env::set_var("FAKE_HERDR_LOG", &log_path);
    }

    let file = build_spread_file_with_branching_from();
    let mut backend = CliBackend::new(fake_herdr_path(), None);

    engine::apply(&file, &ExistingWorkspaces::default(), &mut backend)
        .expect("apply against fake herdr should succeed");

    let log_contents = std::fs::read_to_string(&log_path).expect("fake herdr log should exist");
    let logged_lines: Vec<&str> = log_contents.lines().collect();

    // Both splits target wA:p1 (the pane with `id: editor`), instead of the
    // second split chaining off the first split's pane.
    let expected_lines = vec![
        "workspace create --label ws --focus",
        "tab rename wA:t1 main",
        "pane run wA:p1 nvim",
        "pane split wA:p1 --direction right --ratio 0.5 --no-focus",
        "pane run wA:p3 htop",
        "pane split wA:p1 --direction down --ratio 0.6 --no-focus",
        "pane run wA:p3 lazygit",
    ];

    assert_eq!(logged_lines, expected_lines);

    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn should_produce_expected_plan_for_two_workspace_fixture_before_execution() {
    let file = build_spread_file();
    let plan = engine::plan_file(&file, &ExistingWorkspaces::default());
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
    let plan = engine::plan_file(&file, &ExistingWorkspaces::default());
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

fn idempotent_log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake_herdr_log_idempotent.txt")
}

#[test]
fn should_skip_existing_workspaces_and_create_only_missing_ones() {
    let _lock = FAKE_HERDR_LOCK.lock().unwrap();
    let log_path = idempotent_log_path();
    let _ = std::fs::remove_file(&log_path);

    unsafe {
        std::env::set_var("FAKE_HERDR_LOG", &log_path);
        std::env::set_var("FAKE_HERDR_WORKSPACES", "demo,demo2");
    }

    // `demo` and `demo2` already exist (seeded above); only the config's
    // third workspace, `demo3`, is new. `demo2` carries focus, so the
    // existing `demo2` workspace is focused instead of created.
    let file = SpreadFile {
        workspaces: vec![
            build_spread_file().workspaces[0].clone(),
            build_spread_file().workspaces[1].clone(),
            Workspace {
                name: "demo3".to_string(),
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
    };

    let mut backend = CliBackend::new(fake_herdr_path(), None);
    let existing = ExistingWorkspaces::from_list(
        backend
            .list_workspaces()
            .expect("workspace list against fake herdr should succeed")
            .into_iter()
            .map(WorkspaceSummary::from),
    );

    engine::apply(&file, &existing, &mut backend).expect("apply should succeed");

    let log_contents = std::fs::read_to_string(&log_path).expect("fake herdr log should exist");
    let logged_lines: Vec<&str> = log_contents.lines().collect();

    let expected_lines = vec![
        "workspace list",
        // `demo` is skipped but its first pane has focus: true, and `demo2`
        // has workspace-level focus: both existing workspaces are focused in
        // order; the last one (demo2, x1) ends up focused.
        "workspace focus x0",
        "workspace focus x1",
        "workspace create --label demo3 --no-focus",
        "pane run wA:p1 htop",
    ];

    assert_eq!(logged_lines, expected_lines);

    unsafe {
        std::env::remove_var("FAKE_HERDR_WORKSPACES");
    }
    let _ = std::fs::remove_file(&log_path);
}

// ---------------------------------------------------------------------------
// export: session capture → spread file → YAML → apply compatibility
// ---------------------------------------------------------------------------

fn export_log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake_herdr_log_export.txt")
}

#[test]
fn should_export_the_fake_herdr_session_to_an_apply_compatible_spread_file() {
    let _lock = FAKE_HERDR_LOCK.lock().unwrap();
    let log_path = export_log_path();
    let _ = std::fs::remove_file(&log_path);

    unsafe {
        std::env::set_var("FAKE_HERDR_LOG", &log_path);
        std::env::set_var("FAKE_HERDR_WORKSPACES", "demo");
    }

    let mut backend = CliBackend::new(fake_herdr_path(), None);
    let snapshot =
        export::capture_snapshot(&mut backend).expect("capture against fake herdr succeeds");
    let spread =
        export::build_spread_file(&snapshot).expect("the fake herdr session is representable");
    let yaml = export::render_yaml(&spread).expect("rendering the spread file succeeds");

    validate::validate_config(validate::SourceFile {
        yaml: &yaml,
        path: Path::new("exported.yaml"),
    })
    .expect("exported yaml must pass apply's validation");

    let parsed = SpreadFile::from_str(&yaml).unwrap();
    assert_eq!(parsed.workspaces.len(), 1);
    assert_eq!(parsed.workspaces[0].name, "demo");
    assert_eq!(parsed.workspaces[0].tabs.len(), 2);

    let first_tab = &parsed.workspaces[0].tabs[0];
    assert_eq!(first_tab.label, Some("1".to_string()));
    assert_eq!(first_tab.panes.len(), 2);
    assert_eq!(
        first_tab.panes[0].cwd,
        Some(PathBuf::from("/home/demo/project"))
    );
    assert_eq!(first_tab.panes[0].ratio, None);
    assert_eq!(first_tab.panes[1].ratio, Some(119.0 / 396.0));
    assert_eq!(
        first_tab.panes[1].cwd,
        Some(PathBuf::from("/home/demo/project/logs"))
    );
    assert!(first_tab.panes[1].focus, "the active tab's focused pane");

    let second_tab = &parsed.workspaces[0].tabs[1];
    assert_eq!(second_tab.label, Some("editor".to_string()));
    assert_eq!(second_tab.panes.len(), 1);
    assert_eq!(
        second_tab.panes[0].cwd,
        Some(PathBuf::from("/home/demo/other"))
    );
    assert!(!second_tab.panes[0].focus);

    let log_contents = std::fs::read_to_string(&log_path).expect("fake herdr log exists");
    let logged_lines: Vec<&str> = log_contents.lines().collect();
    assert_eq!(
        logged_lines,
        vec![
            "workspace list",
            "tab list --workspace x0",
            "pane list --workspace x0",
            "pane layout --pane wA:p1",
            "pane layout --pane wA:p3",
        ]
    );

    unsafe {
        std::env::remove_var("FAKE_HERDR_WORKSPACES");
    }
    let _ = std::fs::remove_file(&log_path);
}

/// Replay a plan the way herdr executes it (measured on herdr 0.9.1): each
/// split keeps `round(ratio * extent)` cells of its source pane on the
/// left/top and puts the new pane in the remainder on the right/bottom, and
/// a first pane inherits the cwd of its creation call. Returns each pane's
/// final rect with its resolved cwd.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn simulate_plan_panes(plan: &[BackendOp], tab_areas: &[Rect]) -> Vec<(Rect, Option<PathBuf>)> {
    let mut rects: HashMap<PaneHandle, Rect> = HashMap::new();
    let mut cwds: HashMap<PaneHandle, Option<PathBuf>> = HashMap::new();

    for op in plan {
        match op {
            BackendOp::CreateWorkspace(opts) => {
                rects.insert(PaneHandle::TabRoot(0), tab_areas[0]);
                cwds.insert(PaneHandle::TabRoot(0), opts.cwd.clone());
            }
            BackendOp::CreateTab { index, opts } => {
                rects.insert(PaneHandle::TabRoot(*index), tab_areas[*index]);
                cwds.insert(PaneHandle::TabRoot(*index), opts.cwd.clone());
            }
            BackendOp::SplitPane { from, into, opts } => {
                let source = *rects
                    .get(from)
                    .expect("a split source must already have been created");
                let ratio = opts.ratio.expect("exported splits always carry a ratio");
                let (source_rect, new_rect) = match opts.direction {
                    SplitDirection::Right => {
                        let keep = (ratio * f64::from(source.width)).round() as u32;
                        (
                            Rect {
                                width: keep,
                                ..source
                            },
                            Rect {
                                x: source.x + keep,
                                width: source.width - keep,
                                ..source
                            },
                        )
                    }
                    SplitDirection::Down => {
                        let keep = (ratio * f64::from(source.height)).round() as u32;
                        (
                            Rect {
                                height: keep,
                                ..source
                            },
                            Rect {
                                y: source.y + keep,
                                height: source.height - keep,
                                ..source
                            },
                        )
                    }
                };
                rects.insert(from.clone(), source_rect);
                rects.insert(into.clone(), new_rect);
                cwds.insert(into.clone(), opts.cwd.clone());
            }
            BackendOp::Run { pane, command } => {
                if let Some(dir) = command
                    .strip_prefix("cd '")
                    .and_then(|rest| rest.strip_suffix('\''))
                {
                    cwds.insert(pane.clone(), Some(PathBuf::from(dir)));
                }
            }
            _ => {}
        }
    }

    rects
        .keys()
        .map(|handle| (rects[handle], cwds.get(handle).cloned().flatten()))
        .collect()
}

/// The acceptance proof for export: building the spread file from a
/// snapshot, then planning and replaying it with herdr's measured split
/// semantics, reproduces the snapshot's pane rects and working directories
/// exactly. Panes the snapshot could not capture a directory for land in
/// the invocation directory, exactly as a fresh `apply` run would put them.
fn assert_layout_round_trips(snapshot: &SessionSnapshot, tab_area: Rect) {
    let spread = export::build_spread_file(snapshot).expect("the snapshot layout is representable");
    let yaml = export::render_yaml(&spread).expect("rendering succeeds");

    validate::validate_config(validate::SourceFile {
        yaml: &yaml,
        path: Path::new("round-trip.yaml"),
    })
    .expect("an exported layout must pass apply's validation");

    let invocation_dir = Path::new("/round-trip-invocation");
    let parsed = SpreadFile::from_str(&yaml).unwrap();
    let resolved = resolve_paths(&parsed, &BTreeMap::new(), invocation_dir);
    let plan = engine::plan_file(&resolved, &ExistingWorkspaces::default());

    let tab_count: usize = snapshot.workspaces.iter().map(|ws| ws.tabs.len()).sum();
    let simulated = simulate_plan_panes(&plan, &vec![tab_area; tab_count]);

    let expected: Vec<(Rect, Option<PathBuf>)> = snapshot
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| {
            (
                pane.rect,
                Some(
                    pane.cwd
                        .clone()
                        .unwrap_or_else(|| invocation_dir.to_path_buf()),
                ),
            )
        })
        .collect();

    let mut simulated = simulated;
    let mut expected = expected;
    let sort_key = |(rect, cwd): &(Rect, Option<PathBuf>)| {
        (
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            cwd.clone().map(|p| p.display().to_string()),
        )
    };
    simulated.sort_by_key(sort_key);
    expected.sort_by_key(sort_key);
    assert_eq!(
        simulated, expected,
        "replaying the exported layout must reproduce the captured rects and cwds"
    );
}

fn snapshot_pane(
    pane_id: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    cwd: &str,
) -> PaneSnapshot {
    PaneSnapshot {
        pane_id: pane_id.to_string(),
        rect: Rect {
            x,
            y,
            width,
            height,
        },
        cwd: Some(PathBuf::from(cwd)),
        focused: false,
    }
}

fn snapshot_tab(tab_id: &str, label: &str, panes: Vec<PaneSnapshot>) -> TabSnapshot {
    TabSnapshot {
        tab_id: tab_id.to_string(),
        label: label.to_string(),
        panes,
    }
}

fn single_workspace_snapshot(tabs: Vec<TabSnapshot>) -> SessionSnapshot {
    SessionSnapshot {
        workspaces: vec![WorkspaceSnapshot {
            label: "round-trip".to_string(),
            focused: false,
            active_tab_id: None,
            tabs,
        }],
    }
}

#[test]
fn should_round_trip_a_two_pane_chain_from_export_through_plan_and_replay() {
    let snapshot = single_workspace_snapshot(vec![snapshot_tab(
        "t1",
        "main",
        vec![
            snapshot_pane("a", 0, 0, 119, 73, "/proj"),
            snapshot_pane("b", 119, 0, 277, 73, "/proj/logs"),
        ],
    )]);
    assert_layout_round_trips(
        &snapshot,
        Rect {
            x: 0,
            y: 0,
            width: 396,
            height: 73,
        },
    );
}

#[test]
fn should_round_trip_a_2x2_grid_from_export_through_plan_and_replay() {
    let snapshot = single_workspace_snapshot(vec![snapshot_tab(
        "t1",
        "main",
        vec![
            snapshot_pane("a", 0, 0, 50, 50, "/proj/a"),
            snapshot_pane("b", 50, 0, 50, 50, "/proj/b"),
            snapshot_pane("c", 0, 50, 50, 50, "/proj/c"),
            snapshot_pane("d", 50, 50, 50, 50, "/proj/d"),
        ],
    )]);
    assert_layout_round_trips(
        &snapshot,
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        },
    );
}

#[test]
fn should_round_trip_the_measured_asymmetric_probe_layout_from_export_through_replay() {
    // Live herdr 0.9.1 probe: 396x73 tab, editor column split at 119 cells
    // and then once more vertically on each side.
    let snapshot = single_workspace_snapshot(vec![snapshot_tab(
        "t1",
        "main",
        vec![
            snapshot_pane("p1", 0, 0, 119, 37, "/proj/p1"),
            snapshot_pane("p4", 0, 37, 119, 36, "/proj/p4"),
            snapshot_pane("p2", 119, 0, 277, 51, "/proj/p2"),
            snapshot_pane("p3", 119, 51, 277, 22, "/proj/p3"),
        ],
    )]);
    assert_layout_round_trips(
        &snapshot,
        Rect {
            x: 0,
            y: 0,
            width: 396,
            height: 73,
        },
    );
}

#[test]
fn should_round_trip_a_deeply_nested_layout_from_export_through_plan_and_replay() {
    // Root split right at 0.6, left column split down at 0.5, its bottom
    // pane split right at 0.5, right column split down at 0.25.
    let snapshot = single_workspace_snapshot(vec![snapshot_tab(
        "t1",
        "main",
        vec![
            snapshot_pane("a", 0, 0, 60, 50, "/proj/a"),
            snapshot_pane("b", 60, 0, 40, 25, "/proj/b"),
            snapshot_pane("c", 0, 50, 30, 50, "/proj/c"),
            snapshot_pane("d", 30, 50, 30, 50, "/proj/d"),
            snapshot_pane("e", 60, 25, 40, 75, "/proj/e"),
        ],
    )]);
    assert_layout_round_trips(
        &snapshot,
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        },
    );
}

#[test]
fn should_round_trip_the_captured_fake_herdr_session_through_plan_and_replay() {
    let _lock = FAKE_HERDR_LOCK.lock().unwrap();
    let log_path = export_log_path();
    let _ = std::fs::remove_file(&log_path);

    unsafe {
        std::env::set_var("FAKE_HERDR_LOG", &log_path);
        std::env::set_var("FAKE_HERDR_WORKSPACES", "demo");
    }

    let mut backend = CliBackend::new(fake_herdr_path(), None);
    let snapshot =
        export::capture_snapshot(&mut backend).expect("capture against fake herdr succeeds");

    assert_layout_round_trips(
        &snapshot,
        Rect {
            x: 0,
            y: 0,
            width: 396,
            height: 73,
        },
    );

    unsafe {
        std::env::remove_var("FAKE_HERDR_WORKSPACES");
    }
    let _ = std::fs::remove_file(&log_path);
}
