//! Integration test for `include:`: drives the real binary, with a real
//! multi-file layout on disk, through `apply --dry-run`.
//!
//! The unit tests in `src/include.rs` cover the loader in isolation. This one
//! exists to prove the wiring end to end — that a global config referencing a
//! repository-local layout file produces a plan whose paths point *into that
//! repository*, without herdr running and without either file naming an
//! absolute path.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn binary() -> PathBuf {
    // The test binary lives in target/<profile>/deps; the CLI is two levels up.
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("herdr-spreader")
}

#[test]
fn should_plan_an_included_repository_layout_relative_to_that_repository() {
    let dir = scratch("include-dry-run");
    let repo = dir.join("my-project");
    fs::create_dir_all(&repo).unwrap();

    // The repository's own layout file: no absolute paths anywhere in it.
    fs::write(
        repo.join(".herdr-spreader.yaml"),
        "workspaces:\n  - name: my-project\n    tabs:\n      - label: server\n        panes:\n          - command: just dev\n",
    )
    .unwrap();

    // The global config: names which repositories take part, nothing more.
    let config = dir.join("config.yaml");
    fs::write(
        &config,
        "workspaces:\n  - include: ./my-project\n  - include: ./not-cloned-here\n    optional: true\n",
    )
    .unwrap();

    let output = Command::new(binary())
        .args(["apply", "--dry-run", "--file"])
        .arg(&config)
        // Invoked from somewhere that is neither the config's directory nor the
        // repository's, to prove the plan does not follow the cwd.
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run herdr-spreader");

    assert!(
        output.status.success(),
        "dry run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let plan = String::from_utf8_lossy(&output.stdout);
    let expected_cwd = repo.display().to_string();
    assert!(
        plan.contains(&expected_cwd),
        "expected the plan to be rooted at {expected_cwd}, got:\n{plan}"
    );
    assert!(
        plan.contains("just dev"),
        "expected the repository's command in the plan, got:\n{plan}"
    );
    assert!(
        !plan.contains("not-cloned-here"),
        "an optional missing include should vanish from the plan, got:\n{plan}"
    );

    fs::remove_dir_all(&dir).unwrap();
}
