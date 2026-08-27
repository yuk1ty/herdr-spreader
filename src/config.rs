use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub name: String,
    #[serde(default)]
    pub root: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpreadFile {
    pub workspaces: Vec<Workspace>,
}

impl SpreadFile {
    #[allow(clippy::should_implement_trait)]
    /// Parse a [`SpreadFile`] from a YAML string.
    ///
    /// # Errors
    ///
    /// Returns [`serde_yaml_ng::Error`] if the input is not valid YAML or does
    /// not match the expected schema.
    pub fn from_str(s: &str) -> Result<Self, serde_yaml_ng::Error> {
        serde_yaml_ng::from_str(s)
    }
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub label: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub panes: Vec<Pane>,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pane {
    pub id: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub split: SplitDirection,
    pub from: Option<String>,
    pub ratio: Option<f64>,
    pub wait_for: Option<WaitFor>,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    #[default]
    Right,
    Down,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WaitFor {
    #[serde(rename = "match")]
    pub pattern: String,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("no config file found")]
    NotFound,
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

const CONFIG_FILE_NAMES: &[&str] = &["config.yaml", "config.yml"];

/// Resolve the path to a config file.
///
/// If an explicit path is given, it is returned as-is.
/// Otherwise, candidate paths are tried in priority order:
/// `$HERDR_PLUGIN_CONFIG_DIR` → `$XDG_CONFIG_HOME` → `$HOME/.config`.
/// Each directory is checked for `config.yaml` then `config.yml`.
///
/// # Errors
///
/// Returns [`ConfigError::NotFound`] when no config file is found in any
/// candidate location.
pub fn resolve_config_path(
    explicit: Option<PathBuf>,
    env: &BTreeMap<String, String>,
) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    find_first_existing(&candidate_paths(env)).ok_or(ConfigError::NotFound)
}

pub(crate) fn candidate_paths(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let config_names = CONFIG_FILE_NAMES;

    if let Some(plugin_dir) = env.get("HERDR_PLUGIN_CONFIG_DIR") {
        let dir = PathBuf::from(plugin_dir);
        for name in config_names {
            paths.push(dir.join(name));
        }
    }

    if let Some(xdg_home) = env.get("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(xdg_home).join("herdr-spreader");
        for name in config_names {
            paths.push(dir.join(name));
        }
    }

    let home = env.get("HOME").map_or("", String::as_str);
    let dir = PathBuf::from(home).join(".config").join("herdr-spreader");
    for name in config_names {
        paths.push(dir.join(name));
    }

    paths
}

pub(crate) fn find_first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

/// Read a config file from disk into a `String`.
///
/// # Errors
///
/// Returns [`ConfigError::Io`] if the file cannot be read.
pub fn read_config(path: &Path) -> Result<String, ConfigError> {
    fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[must_use]
pub fn resolve_paths(file: &SpreadFile, env: &BTreeMap<String, String>, cwd: &Path) -> SpreadFile {
    SpreadFile {
        workspaces: file
            .workspaces
            .iter()
            .map(|ws| resolve_workspace_paths(ws, env, cwd))
            .collect(),
    }
}

fn resolve_workspace_paths(
    ws: &Workspace,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Workspace {
    // Default root to the invocation cwd so relative tab/pane cwd values always
    // have an absolute base to resolve against, instead of leaking a bare relative
    // path through to herdr or a shell `cd` when the config has no explicit root.
    let root = ws.root.as_ref().map_or_else(
        || cwd.to_path_buf(),
        |root| expand_root_path(root, env, cwd),
    );

    Workspace {
        name: ws.name.clone(),
        root: Some(root),
        env: ws.env.clone(),
        tabs: ws
            .tabs
            .iter()
            .map(|tab| Tab {
                label: tab.label.clone(),
                cwd: tab.cwd.as_ref().map(|tab_cwd| expand_tilde(tab_cwd, env)),
                panes: tab
                    .panes
                    .iter()
                    .map(|pane| Pane {
                        id: pane.id.clone(),
                        command: pane.command.clone(),
                        cwd: pane
                            .cwd
                            .as_ref()
                            .map(|pane_cwd| expand_tilde(pane_cwd, env)),
                        env: pane.env.clone(),
                        split: pane.split,
                        from: pane.from.clone(),
                        ratio: pane.ratio,
                        wait_for: pane.wait_for.clone(),
                        focus: pane.focus,
                    })
                    .collect(),
            })
            .collect(),
        focus: ws.focus,
    }
}

fn expand_root_path(path: &Path, env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    let expanded = expand_tilde(path, env);
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

fn expand_tilde(path: &Path, env: &BTreeMap<String, String>) -> PathBuf {
    let path_str = path.to_string_lossy();
    let home = env.get("HOME").map_or("", String::as_str);
    if path_str == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = path_str.strip_prefix("~/") {
        PathBuf::from(home).join(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_not_found_error_message() {
        let err = ConfigError::NotFound;
        let msg = err.to_string();
        assert!(
            msg.contains("no config file"),
            "expected 'no config file' in '{msg}'"
        );
    }

    #[test]
    fn should_parse_two_workspaces_given_multi_workspace_yaml() {
        let yaml = r"
workspaces:
  - name: frontend
  - name: backend
";

        let file = SpreadFile::from_str(yaml).unwrap();

        assert_eq!(file.workspaces.len(), 2);
        assert_eq!(file.workspaces[0].name, "frontend");
        assert_eq!(file.workspaces[1].name, "backend");
    }

    #[test]
    fn should_reject_legacy_top_level_single_workspace_yaml() {
        let yaml = "name: demo\ntabs: []";

        let result = SpreadFile::from_str(yaml);

        let err = result.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("name"));
        assert!(message.contains("workspaces"));
    }

    #[test]
    fn should_parse_workspace_level_focus_flag_and_default_it_to_false() {
        let yaml = r"
workspaces:
  - name: frontend
  - name: backend
    focus: true
";

        let file = SpreadFile::from_str(yaml).unwrap();

        assert!(!file.workspaces[0].focus);
        assert!(file.workspaces[1].focus);
    }

    #[test]
    fn should_parse_workspace_name_given_minimal_yaml_with_only_name() {
        let yaml = r"
workspaces:
  - name: demo
";

        let file = SpreadFile::from_str(yaml).unwrap();

        assert_eq!(file.workspaces[0].name, "demo");
        assert!(file.workspaces[0].tabs.is_empty());
        assert_eq!(file.workspaces[0].root, None);
    }

    #[test]
    fn should_parse_tabs_and_panes_given_tmuxinator_style_yaml() {
        let yaml = r"
workspaces:
  - name: demo
    tabs:
      - label: editor
        panes:
          - command: nvim
      - label: server
        panes:
          - command: cargo run
";

        let file = SpreadFile::from_str(yaml).unwrap();

        assert_eq!(file.workspaces[0].tabs[0].label, Some("editor".to_string()));
        assert_eq!(
            file.workspaces[0].tabs[0].panes[0].command,
            Some("nvim".to_string())
        );
        assert_eq!(file.workspaces[0].tabs[1].panes.len(), 1);
    }

    #[test]
    fn should_default_split_to_right_and_ratio_to_none_when_omitted() {
        let yaml = r"
workspaces:
  - name: demo
    tabs:
      - panes:
          - command: nvim
";

        let file = SpreadFile::from_str(yaml).unwrap();

        assert_eq!(
            file.workspaces[0].tabs[0].panes[0].split,
            SplitDirection::Right
        );
        assert!(file.workspaces[0].tabs[0].panes[0].ratio.is_none());
    }

    #[test]
    fn should_reject_config_given_unknown_split_direction() {
        let yaml = r"
workspaces:
  - name: demo
    tabs:
      - panes:
          - command: nvim
            split: left
";

        let result = SpreadFile::from_str(yaml);

        let err = result.unwrap_err();
        assert!(err.to_string().contains("left"));
    }

    #[test]
    fn should_parse_pane_id_and_from_given_branching_layout_yaml() {
        let yaml = r"
workspaces:
  - name: demo
    tabs:
      - panes:
          - id: editor
            command: nvim
          - id: agent
            from: editor
            split: right
            command: claude
          - from: editor
            split: down
            command: lazygit
";

        let file = SpreadFile::from_str(yaml).unwrap();

        let panes = &file.workspaces[0].tabs[0].panes;
        assert_eq!(panes[0].id, Some("editor".to_string()));
        assert_eq!(panes[0].from, None);
        assert_eq!(panes[1].id, Some("agent".to_string()));
        assert_eq!(panes[1].from, Some("editor".to_string()));
        assert_eq!(panes[2].id, None);
        assert_eq!(panes[2].from, Some("editor".to_string()));
    }

    #[test]
    fn should_default_pane_id_and_from_to_none_when_omitted() {
        let yaml = r"
workspaces:
  - name: demo
    tabs:
      - panes:
          - command: nvim
";

        let file = SpreadFile::from_str(yaml).unwrap();

        assert_eq!(file.workspaces[0].tabs[0].panes[0].id, None);
        assert_eq!(file.workspaces[0].tabs[0].panes[0].from, None);
    }

    #[test]
    fn should_parse_wait_for_with_match_and_timeout_given_pane_sync_config() {
        let yaml = r#"
workspaces:
  - name: demo
    tabs:
      - panes:
          - command: nvim
            wait_for:
              match: "ready"
              timeout_ms: 5000
"#;

        let file = SpreadFile::from_str(yaml).unwrap();

        assert_eq!(
            file.workspaces[0].tabs[0].panes[0].wait_for,
            Some(WaitFor {
                pattern: "ready".to_string(),
                timeout_ms: Some(5000),
            })
        );
    }

    #[test]
    fn should_include_file_path_in_error_when_config_file_is_missing() {
        let path = Path::new("/nonexistent/path/to/config.yaml");

        let result = read_config(path);

        let err = result.unwrap_err();
        assert!(err.to_string().contains("/nonexistent/path/to/config.yaml"));
    }

    #[test]
    fn should_load_spread_file_from_disk_given_multi_workspace_yaml() {
        let yaml = r"
workspaces:
  - name: frontend
  - name: backend
";
        let path = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-should_load_spread_file_from_disk_given_multi_workspace_yaml.yml",
            std::process::id()
        ));
        fs::write(&path, yaml).unwrap();

        let yaml = read_config(&path).unwrap();
        let file = SpreadFile::from_str(&yaml).unwrap();

        assert_eq!(file.workspaces.len(), 2);

        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn should_prefer_explicit_path_over_plugin_config_dir_when_resolving_config_path() {
        let explicit = Some(PathBuf::from("/tmp/x.yml"));
        let mut env = BTreeMap::new();
        env.insert("HERDR_PLUGIN_CONFIG_DIR".to_string(), "/cfg".to_string());

        let resolved = resolve_config_path(explicit, &env).unwrap();

        assert_eq!(resolved, PathBuf::from("/tmp/x.yml"));
    }

    #[test]
    fn should_return_plugin_config_yaml_when_it_exists() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-plugin-yaml",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "").unwrap();

        let mut env = BTreeMap::new();
        env.insert(
            "HERDR_PLUGIN_CONFIG_DIR".to_string(),
            dir.to_string_lossy().to_string(),
        );

        let resolved = resolve_config_path(None, &env).unwrap();

        assert_eq!(resolved, dir.join("config.yaml"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_return_plugin_config_yml_when_only_yml_exists() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-plugin-yml",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yml"), "").unwrap();

        let mut env = BTreeMap::new();
        env.insert(
            "HERDR_PLUGIN_CONFIG_DIR".to_string(),
            dir.to_string_lossy().to_string(),
        );

        let resolved = resolve_config_path(None, &env).unwrap();

        assert_eq!(resolved, dir.join("config.yml"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_return_xdg_config_yaml_when_configured() {
        let xdg_home =
            std::env::temp_dir().join(format!("herdr-spreader-test-{}-xdg", std::process::id()));
        let config_dir = xdg_home.join("herdr-spreader");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join("config.yaml"), "").unwrap();

        let mut env = BTreeMap::new();
        env.insert(
            "XDG_CONFIG_HOME".to_string(),
            xdg_home.to_string_lossy().to_string(),
        );
        env.insert("HOME".to_string(), "/home/demo".to_string());

        let resolved = resolve_config_path(None, &env).unwrap();

        assert_eq!(resolved, config_dir.join("config.yaml"));
        fs::remove_dir_all(&xdg_home).unwrap();
    }

    #[test]
    fn should_fall_back_to_home_config_when_xdg_dir_has_no_yaml_or_yml() {
        let xdg_home = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-xdg-empty",
            std::process::id()
        ));
        let home =
            std::env::temp_dir().join(format!("herdr-spreader-test-{}-home", std::process::id()));
        let home_config_dir = home.join(".config").join("herdr-spreader");
        fs::create_dir_all(&home_config_dir).unwrap();
        fs::write(home_config_dir.join("config.yaml"), "").unwrap();

        let mut env = BTreeMap::new();
        env.insert(
            "XDG_CONFIG_HOME".to_string(),
            xdg_home.to_string_lossy().to_string(),
        );
        env.insert("HOME".to_string(), home.to_string_lossy().to_string());

        let resolved = resolve_config_path(None, &env).unwrap();

        assert_eq!(resolved, home_config_dir.join("config.yaml"));
        fs::remove_dir_all(&xdg_home).unwrap_or_default();
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn should_return_home_config_when_xdg_config_home_is_not_set() {
        let home = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-home-no-xdg",
            std::process::id()
        ));
        let home_config_dir = home.join(".config").join("herdr-spreader");
        fs::create_dir_all(&home_config_dir).unwrap();
        fs::write(home_config_dir.join("config.yaml"), "").unwrap();

        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), home.to_string_lossy().to_string());

        let resolved = resolve_config_path(None, &env).unwrap();

        assert_eq!(resolved, home_config_dir.join("config.yaml"));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn should_return_not_found_error_when_no_config_file_exists_in_any_location() {
        let home = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-not-found",
            std::process::id()
        ));
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), home.to_string_lossy().to_string());

        let result = resolve_config_path(None, &env);

        assert!(matches!(result, Err(ConfigError::NotFound)));
        fs::remove_dir_all(&home).unwrap_or_default();
    }

    #[test]
    fn should_list_all_candidate_paths_for_plugin_xdg_and_home_in_order_without_touching_the_filesystem()
     {
        let mut env = BTreeMap::new();
        env.insert("HERDR_PLUGIN_CONFIG_DIR".to_string(), "/cfg".to_string());
        env.insert("XDG_CONFIG_HOME".to_string(), "/xdg".to_string());
        env.insert("HOME".to_string(), "/home/demo".to_string());
        let paths = candidate_paths(&env);
        assert_eq!(
            paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![
                "/cfg/config.yaml",
                "/cfg/config.yml",
                "/xdg/herdr-spreader/config.yaml",
                "/xdg/herdr-spreader/config.yml",
                "/home/demo/.config/herdr-spreader/config.yaml",
                "/home/demo/.config/herdr-spreader/config.yml",
            ]
        );
    }

    #[test]
    fn should_return_first_existing_candidate_from_a_list() {
        let tmp = std::env::temp_dir().join(format!(
            "herdr-spreader-test-{}-find-first-existing",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("config.yaml"), "").unwrap();
        let missing = tmp.join("nonexistent_dir").join("config.yaml");
        let found = find_first_existing(&[
            missing.clone(),
            tmp.join("config.yaml"),
            tmp.join("config.yml"),
        ]);
        assert_eq!(found, Some(tmp.join("config.yaml")));
        let none = find_first_existing(&[missing.clone(), tmp.join("also-missing.yaml")]);
        assert!(none.is_none());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn should_expand_home_relative_root_given_tilde_slash_prefix() {
        let config = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("~/code/my-project")),
            ..Default::default()
        };
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/demo".to_string());

        let resolved = resolve_workspace_paths(&config, &env, Path::new("/irrelevant"));

        assert_eq!(
            resolved.root,
            Some(PathBuf::from("/home/demo/code/my-project"))
        );
    }

    #[test]
    fn should_expand_bare_tilde_root_to_home_directory() {
        let config = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("~")),
            ..Default::default()
        };
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/demo".to_string());

        let resolved = resolve_workspace_paths(&config, &env, Path::new("/irrelevant"));

        assert_eq!(resolved.root, Some(PathBuf::from("/home/demo")));
    }

    #[test]
    fn should_leave_absolute_root_unchanged() {
        let config = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("/proj")),
            ..Default::default()
        };

        let resolved = resolve_workspace_paths(&config, &BTreeMap::new(), Path::new("/irrelevant"));

        assert_eq!(resolved.root, Some(PathBuf::from("/proj")));
    }

    #[test]
    fn should_resolve_relative_root_against_given_cwd() {
        let config = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("proj")),
            ..Default::default()
        };

        let resolved = resolve_workspace_paths(&config, &BTreeMap::new(), Path::new("/home/demo"));

        assert_eq!(resolved.root, Some(PathBuf::from("/home/demo/proj")));
    }

    #[test]
    fn should_expand_tilde_in_tab_and_pane_cwd_without_forcing_them_absolute() {
        let config = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("/proj")),
            tabs: vec![Tab {
                label: None,
                cwd: Some(PathBuf::from("~/logs")),
                panes: vec![
                    Pane {
                        command: None,
                        cwd: Some(PathBuf::from("~")),
                        ..Default::default()
                    },
                    Pane {
                        command: None,
                        cwd: Some(PathBuf::from("./relative")),
                        ..Default::default()
                    },
                ],
            }],
            ..Default::default()
        };
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/demo".to_string());

        let resolved = resolve_workspace_paths(&config, &env, Path::new("/irrelevant"));

        assert_eq!(resolved.tabs[0].cwd, Some(PathBuf::from("/home/demo/logs")));
        assert_eq!(
            resolved.tabs[0].panes[0].cwd,
            Some(PathBuf::from("/home/demo"))
        );
        assert_eq!(
            resolved.tabs[0].panes[1].cwd,
            Some(PathBuf::from("./relative"))
        );
    }

    #[test]
    fn should_default_root_to_invocation_cwd_when_root_is_absent() {
        let config = Workspace {
            name: "demo".to_string(),
            tabs: vec![Tab {
                label: None,
                cwd: Some(PathBuf::from("svc")),
                panes: vec![],
            }],
            ..Default::default()
        };

        let resolved =
            resolve_workspace_paths(&config, &BTreeMap::new(), Path::new("/home/demo/project"));

        assert_eq!(resolved.root, Some(PathBuf::from("/home/demo/project")));
        assert_eq!(resolved.tabs[0].cwd, Some(PathBuf::from("svc")));
    }

    #[test]
    fn should_resolve_workspace_paths_without_mutating_the_input_workspace() {
        let ws = Workspace {
            name: "demo".to_string(),
            root: Some(PathBuf::from("~/code")),
            tabs: vec![Tab {
                cwd: Some(PathBuf::from("./src")),
                panes: vec![Pane {
                    cwd: Some(PathBuf::from("./inner")),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = ws.clone();
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/demo".to_string());
        let resolved = resolve_workspace_paths(&ws, &env, Path::new("/cwd"));
        assert_eq!(ws, snapshot, "input workspace must not be mutated");
        assert_eq!(resolved.root, Some(PathBuf::from("/home/demo/code")));
        assert_eq!(
            resolved.tabs[0].panes[0].cwd,
            Some(PathBuf::from("./inner"))
        );
        assert_eq!(resolved.tabs[0].cwd, Some(PathBuf::from("./src")));
    }

    #[test]
    fn should_resolve_each_workspace_root_independently_given_two_workspaces() {
        let file = SpreadFile {
            workspaces: vec![
                Workspace {
                    name: "ws1".to_string(),
                    root: Some(PathBuf::from("~/a")),
                    ..Default::default()
                },
                Workspace {
                    name: "ws2".to_string(),
                    root: Some(PathBuf::from("proj")),
                    ..Default::default()
                },
            ],
        };
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/demo".to_string());

        let resolved = resolve_paths(&file, &env, Path::new("/home/demo/base"));

        assert_eq!(
            resolved.workspaces[0].root,
            Some(PathBuf::from("/home/demo/a"))
        );
        assert_eq!(
            resolved.workspaces[1].root,
            Some(PathBuf::from("/home/demo/base/proj"))
        );
    }

    #[test]
    fn should_default_all_roots_to_invocation_cwd_when_multiple_workspaces_omit_root() {
        let file = SpreadFile {
            workspaces: vec![
                Workspace {
                    name: "ws1".to_string(),
                    ..Default::default()
                },
                Workspace {
                    name: "ws2".to_string(),
                    ..Default::default()
                },
            ],
        };

        let resolved = resolve_paths(&file, &BTreeMap::new(), Path::new("/home/demo/project"));

        assert_eq!(
            resolved.workspaces[0].root,
            Some(PathBuf::from("/home/demo/project"))
        );
        assert_eq!(
            resolved.workspaces[1].root,
            Some(PathBuf::from("/home/demo/project"))
        );
    }

    #[test]
    fn should_parse_bundled_example_config() {
        let yaml = include_str!("../examples/config.yaml");

        let file = SpreadFile::from_str(yaml).unwrap();

        // Loose on content (the example is free to evolve) but pinned on the
        // schema-level things this test exists to guard: the file parses under
        // `deny_unknown_fields`, describes at least the multi-workspace case,
        // and exercises the `focus: true` feature it's meant to demonstrate.
        assert!(file.workspaces.len() >= 2);
        assert!(file.workspaces.iter().any(|ws| ws.focus));
    }
}
