//! CLI-level integration test: the real `herdr-spreader` binary, driven via
//! `assert_cmd` against the fake herdr fixture, warns on stderr for every
//! workspace it skips because the name already exists.

use std::path::PathBuf;

use assert_cmd::Command;

fn fake_herdr_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-herdr.sh")
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("herdr-spreader-skip-warning-{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config_with_workspaces(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "workspaces:\n  \
         - name: demo\n    \
         tabs:\n    \
         - panes:\n      \
         - command: nvim\n  \
         - name: demo3\n    \
         tabs:\n    \
         - panes:\n      \
         - command: htop\n",
    )
    .unwrap();
    path
}

#[test]
fn should_warn_on_stderr_for_skipped_workspaces_but_not_created_ones() {
    let dir = scratch_dir("stderr-warning");
    let config = config_with_workspaces(&dir);
    let log = dir.join("fake-herdr.log");

    Command::cargo_bin("herdr-spreader")
        .unwrap()
        .arg("apply")
        .arg("--file")
        .arg(&config)
        .env("HERDR_BIN_PATH", fake_herdr_path())
        .env("FAKE_HERDR_LOG", &log)
        .env("FAKE_HERDR_WORKSPACES", "demo")
        .assert()
        .success()
        .stdout("")
        .stderr("warning: skipping workspace 'demo' (already exists); its tabs, panes, and commands are left untouched\n");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn should_not_warn_when_no_workspace_is_skipped() {
    let dir = scratch_dir("no-warning");
    let config = config_with_workspaces(&dir);
    let log = dir.join("fake-herdr.log");

    Command::cargo_bin("herdr-spreader")
        .unwrap()
        .arg("apply")
        .arg("--file")
        .arg(&config)
        .env("HERDR_BIN_PATH", fake_herdr_path())
        .env("FAKE_HERDR_LOG", &log)
        .env("FAKE_HERDR_WORKSPACES", "")
        .assert()
        .success()
        .stderr("");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn should_reject_duplicate_names_within_one_config_file_at_the_validation_boundary() {
    let dir = scratch_dir("dup-rejected");
    let config = dir.join("config.yaml");
    std::fs::write(
        &config,
        "workspaces:\n  \
         - name: solo\n    \
         tabs:\n    \
         - panes:\n      \
         - command: nvim\n  \
         - name: solo\n    \
         tabs:\n    \
         - panes:\n      \
         - command: nvim\n",
    )
    .unwrap();
    let log = dir.join("fake-herdr.log");

    // Duplicates inside one file are a config error, not a silent skip:
    // validation rejects them before any herdr call is made.
    let output = Command::cargo_bin("herdr-spreader")
        .unwrap()
        .arg("apply")
        .arg("--file")
        .arg(&config)
        .env("HERDR_BIN_PATH", fake_herdr_path())
        .env("FAKE_HERDR_LOG", &log)
        .env("FAKE_HERDR_WORKSPACES", "")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate workspace name"),
        "stderr should name the duplicate: {stderr}"
    );
    assert!(
        !log.exists() || std::fs::read_to_string(&log).unwrap().is_empty(),
        "no herdr call may happen for a rejected config"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}
