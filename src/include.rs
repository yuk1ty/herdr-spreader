//! Expansion of `include:` entries into a flat [`SpreadFile`].
//!
//! A layout file may reference another layout file instead of spelling a
//! workspace out inline. That keeps a project's layout in the repository whose
//! commands it names, while the global config stays the entry point that
//! decides which repositories take part.
//!
//! The work splits the way `AGENTS.md` asks for:
//!
//! - **Actions** — [`load_flat`] and its recursion read files from disk.
//! - **Calculations** — [`include_candidates`] and [`resolve_include_path`]
//!   turn `(raw path, including directory, env)` into the paths to try, with
//!   no I/O; [`crate::config::resolve_paths`] resolves a parsed file's paths
//!   against a base directory.
//! - **Data** — [`crate::config::Include`] and [`crate::config::WorkspaceEntry`].
//!
//! Each file's paths are resolved against **its own** directory before its
//! workspaces are appended, so an included file can say `cwd: ./src` and mean
//! its own `src`. That is what makes a repository-local layout portable
//! between machines and between worktrees of the same repository.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{
    CONFIG_FILE_NAMES, Include, SpreadFile, SpreadSource, WorkspaceEntry, expand_tilde,
    normalize_path, resolve_workspace_paths,
};
use crate::validate::{Severity, ValidationFinding, validate};

/// How deep `include:` chains may nest before the loader gives up.
///
/// A cycle is caught exactly by the ancestor check in [`load_into`]; this cap
/// only bounds a legitimately absurd chain, so it can be generous.
pub const MAX_INCLUDE_DEPTH: usize = 16;

/// File names looked for when an `include:` points at a directory, in order.
///
/// The dotted names come first so a repository can carry its layout as a
/// hidden file alongside its other tooling config.
pub const DIRECTORY_INCLUDE_FILE_NAMES: &[&str] = &[
    ".herdr-spreader.yaml",
    ".herdr-spreader.yml",
    "herdr-spreader.yaml",
    "herdr-spreader.yml",
];

/// Load a layout file and every file it includes, as one flat [`SpreadFile`].
///
/// `base` is the directory relative paths in the *root* file resolve against —
/// the invocation directory, matching the behaviour of a file with no includes.
/// Included files resolve their own relative paths against their own directory
/// instead.
///
/// # Errors
///
/// Returns the accumulated [`ValidationFinding`]s when a file cannot be read or
/// parsed, when a non-optional include is missing, when includes form a cycle
/// or nest deeper than [`MAX_INCLUDE_DEPTH`], or when the flattened result
/// fails semantic validation.
pub fn load_flat(
    path: &Path,
    base: &Path,
    env: &BTreeMap<String, String>,
) -> Result<SpreadFile, Vec<ValidationFinding>> {
    let mut workspaces = Vec::new();
    let mut findings = Vec::new();
    let mut ancestors: Vec<PathBuf> = Vec::new();

    load_into(
        path,
        base,
        env,
        0,
        &mut ancestors,
        &mut workspaces,
        &mut findings,
    );

    let file = SpreadFile { workspaces };
    findings.extend(validate(&file));

    if findings.is_empty() {
        Ok(file)
    } else {
        Err(findings)
    }
}

/// Read one file, resolve its paths against `base`, and append its workspaces,
/// recursing into every `include:` it carries.
fn load_into(
    path: &Path,
    base: &Path,
    env: &BTreeMap<String, String>,
    depth: usize,
    ancestors: &mut Vec<PathBuf>,
    workspaces: &mut Vec<crate::config::Workspace>,
    findings: &mut Vec<ValidationFinding>,
) {
    if depth > MAX_INCLUDE_DEPTH {
        findings.push(error(format!(
            "include chain deeper than {MAX_INCLUDE_DEPTH} files, giving up at {}",
            path.display()
        )));
        return;
    }

    // Canonicalise so two spellings of the same file — a symlink, a `..`, a
    // relative path — are recognised as one. A file that cannot be
    // canonicalised is about to fail its read anyway; fall back to the path as
    // written so the error names something the reader recognises.
    let identity = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if ancestors.contains(&identity) {
        findings.push(error(format!(
            "include cycle: {} includes itself, via {}",
            identity.display(),
            ancestors
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" -> ")
        )));
        return;
    }

    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => {
            findings.push(error(format!(
                "failed to read config file {}: {err}",
                path.display()
            )));
            return;
        }
    };

    let source = match SpreadSource::from_str(&contents) {
        Ok(source) => source,
        Err(err) => {
            findings.push(error(format!(
                "failed to parse config file {}: {err}",
                path.display()
            )));
            return;
        }
    };

    // The directory an included file's own relative paths resolve against.
    let here = path.parent().unwrap_or(base).to_path_buf();

    ancestors.push(identity);
    for entry in source.workspaces {
        match entry {
            WorkspaceEntry::Inline(ws) => {
                workspaces.push(resolve_workspace_paths(&ws, env, base));
            }
            WorkspaceEntry::Include(inc) => match resolve_include_path(&inc, &here, env) {
                Some(target) => {
                    let target_base = target.parent().unwrap_or(&here).to_path_buf();
                    load_into(
                        &target,
                        &target_base,
                        env,
                        depth + 1,
                        ancestors,
                        workspaces,
                        findings,
                    );
                }
                None => {
                    if !inc.optional {
                        findings.push(error(format!(
                            "include target not found: {} (referenced by {}){}",
                            inc.include.display(),
                            path.display(),
                            directory_hint(&inc, &here, env)
                        )));
                    }
                }
            },
        }
    }
    ancestors.pop();
}

/// Find the file an [`Include`] names, or `None` when nothing exists there.
///
/// A path naming a directory is probed for the well-known layout file names in
/// [`DIRECTORY_INCLUDE_FILE_NAMES`], then [`CONFIG_FILE_NAMES`], so an include
/// can name a repository root rather than a file inside it.
fn resolve_include_path(
    inc: &Include,
    including_dir: &Path,
    env: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    include_candidates(&inc.include, including_dir, env)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Every path an include might resolve to, in priority order. Pure: the
/// directory probe is expressed as extra candidates rather than as I/O.
#[must_use]
pub fn include_candidates(
    raw: &Path,
    including_dir: &Path,
    env: &BTreeMap<String, String>,
) -> Vec<PathBuf> {
    let expanded = expand_tilde(raw, env);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        including_dir.join(expanded)
    };

    let absolute = normalize_path(&absolute);
    let mut candidates = vec![absolute.clone()];
    for name in DIRECTORY_INCLUDE_FILE_NAMES.iter().chain(CONFIG_FILE_NAMES) {
        candidates.push(absolute.join(name));
    }
    candidates
}

/// Name the files that were looked for, when an include pointed at a directory
/// that exists but holds no layout file — otherwise "not found" is misleading.
fn directory_hint(inc: &Include, including_dir: &Path, env: &BTreeMap<String, String>) -> String {
    let candidates = include_candidates(&inc.include, including_dir, env);
    match candidates.first() {
        Some(first) if first.is_dir() => format!(
            "; it is a directory containing none of: {}",
            DIRECTORY_INCLUDE_FILE_NAMES
                .iter()
                .chain(CONFIG_FILE_NAMES)
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => String::new(),
    }
}

fn error(message: String) -> ValidationFinding {
    ValidationFinding {
        severity: Severity::Error,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Tests write real files because the loader's whole job is reading them;
    /// each gets its own directory named after the test so a failure leaves
    /// evidence behind that names itself.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("herdr-spreader-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env_with_home(home: &Path) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), home.display().to_string());
        env
    }

    #[test]
    fn should_splice_included_workspaces_in_place_keeping_file_order() {
        let dir = scratch("splice-order");
        fs::write(dir.join("repo.yaml"), "workspaces:\n  - name: from-repo\n").unwrap();
        fs::write(
            dir.join("config.yaml"),
            "workspaces:\n  - name: before\n  - include: ./repo.yaml\n  - name: after\n",
        )
        .unwrap();

        let file = load_flat(&dir.join("config.yaml"), &dir, &env_with_home(&dir)).unwrap();

        let names: Vec<&str> = file.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["before", "from-repo", "after"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_default_an_included_workspaces_root_to_the_included_files_directory() {
        // The point of the feature: a repository's layout file names no
        // absolute paths, and still resolves to that repository — not to
        // wherever the command happened to be run.
        let dir = scratch("included-root");
        let repo = dir.join("some-repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            repo.join(".herdr-spreader.yaml"),
            // A pane is needed only because an empty tab is a validation
            // warning, and warnings are fatal in this codebase.
            "workspaces:\n  - name: repo\n    tabs:\n      - label: src\n        cwd: ./src\n        panes:\n          - command: ls\n",
        )
        .unwrap();
        fs::write(
            dir.join("config.yaml"),
            "workspaces:\n  - include: ./some-repo\n",
        )
        .unwrap();

        let elsewhere = dir.join("invoked-from-here");
        fs::create_dir_all(&elsewhere).unwrap();
        let file = load_flat(&dir.join("config.yaml"), &elsewhere, &env_with_home(&dir)).unwrap();

        assert_eq!(file.workspaces[0].root.as_deref(), Some(repo.as_path()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_resolve_the_root_files_own_paths_against_the_invocation_directory() {
        // Unchanged behaviour for a file with no includes, which is what makes
        // this addition safe for existing configs.
        let dir = scratch("root-file-base");
        fs::write(dir.join("config.yaml"), "workspaces:\n  - name: plain\n").unwrap();
        let invoked_from = dir.join("cwd");
        fs::create_dir_all(&invoked_from).unwrap();

        let file = load_flat(
            &dir.join("config.yaml"),
            &invoked_from,
            &env_with_home(&dir),
        )
        .unwrap();

        assert_eq!(
            file.workspaces[0].root.as_deref(),
            Some(invoked_from.as_path())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_resolve_a_relative_include_against_the_including_file_not_the_cwd() {
        let dir = scratch("relative-include");
        let nested = dir.join("layouts");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("inner.yaml"), "workspaces:\n  - name: inner\n").unwrap();
        fs::write(
            nested.join("outer.yaml"),
            "workspaces:\n  - include: ./inner.yaml\n",
        )
        .unwrap();

        let file = load_flat(&nested.join("outer.yaml"), &dir, &env_with_home(&dir)).unwrap();

        assert_eq!(file.workspaces[0].name, "inner");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_report_a_missing_include_and_name_the_file_that_referenced_it() {
        let dir = scratch("missing-include");
        fs::write(
            dir.join("config.yaml"),
            "workspaces:\n  - include: ./not-here.yaml\n",
        )
        .unwrap();

        let findings = load_flat(&dir.join("config.yaml"), &dir, &env_with_home(&dir)).unwrap_err();

        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("not-here.yaml"));
        assert!(findings[0].message.contains("config.yaml"));
        assert_eq!(findings[0].severity, Severity::Error);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_skip_a_missing_include_silently_when_it_is_marked_optional() {
        let dir = scratch("optional-include");
        fs::write(
            dir.join("config.yaml"),
            "workspaces:\n  - name: present\n  - include: ./not-here.yaml\n    optional: true\n",
        )
        .unwrap();

        let file = load_flat(&dir.join("config.yaml"), &dir, &env_with_home(&dir)).unwrap();

        assert_eq!(file.workspaces.len(), 1);
        assert_eq!(file.workspaces[0].name, "present");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_reject_an_include_cycle_instead_of_recursing_forever() {
        let dir = scratch("include-cycle");
        fs::write(dir.join("a.yaml"), "workspaces:\n  - include: ./b.yaml\n").unwrap();
        fs::write(dir.join("b.yaml"), "workspaces:\n  - include: ./a.yaml\n").unwrap();

        let findings = load_flat(&dir.join("a.yaml"), &dir, &env_with_home(&dir)).unwrap_err();

        assert!(
            findings.iter().any(|f| f.message.contains("include cycle")),
            "expected a cycle finding, got {findings:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_give_up_on_an_include_chain_deeper_than_the_cap() {
        let dir = scratch("include-depth");
        let links = MAX_INCLUDE_DEPTH + 2;
        for i in 0..links {
            fs::write(
                dir.join(format!("f{i}.yaml")),
                format!("workspaces:\n  - include: ./f{}.yaml\n", i + 1),
            )
            .unwrap();
        }
        fs::write(
            dir.join(format!("f{links}.yaml")),
            "workspaces:\n  - name: bottom\n",
        )
        .unwrap();

        let findings = load_flat(&dir.join("f0.yaml"), &dir, &env_with_home(&dir)).unwrap_err();

        assert!(
            findings.iter().any(|f| f.message.contains("deeper than")),
            "expected a depth finding, got {findings:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_report_duplicate_workspace_names_across_included_files() {
        // Two repositories that both call their workspace "dev" would otherwise
        // produce two identically named workspaces and no way to tell them apart.
        let dir = scratch("duplicate-across-files");
        fs::write(dir.join("one.yaml"), "workspaces:\n  - name: dev\n").unwrap();
        fs::write(dir.join("two.yaml"), "workspaces:\n  - name: dev\n").unwrap();
        fs::write(
            dir.join("config.yaml"),
            "workspaces:\n  - include: ./one.yaml\n  - include: ./two.yaml\n",
        )
        .unwrap();

        let findings = load_flat(&dir.join("config.yaml"), &dir, &env_with_home(&dir)).unwrap_err();

        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("duplicate workspace name")),
            "expected a duplicate-name finding, got {findings:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_report_a_parse_error_naming_the_included_file_not_the_root_file() {
        let dir = scratch("included-parse-error");
        fs::write(dir.join("broken.yaml"), "workspaces:\n  - nome: typo\n").unwrap();
        fs::write(
            dir.join("config.yaml"),
            "workspaces:\n  - include: ./broken.yaml\n",
        )
        .unwrap();

        let findings = load_flat(&dir.join("config.yaml"), &dir, &env_with_home(&dir)).unwrap_err();

        assert!(
            findings.iter().any(|f| f.message.contains("broken.yaml")),
            "expected the broken file to be named, got {findings:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn should_offer_directory_layout_file_names_as_include_candidates() {
        let env = env_with_home(Path::new("/home/demo"));
        let candidates = include_candidates(Path::new("./repo"), Path::new("/cfg"), &env);

        assert_eq!(candidates[0], PathBuf::from("/cfg/repo"));
        assert!(candidates.contains(&PathBuf::from("/cfg/repo/.herdr-spreader.yaml")));
        assert!(candidates.contains(&PathBuf::from("/cfg/repo/config.yaml")));
    }

    #[test]
    fn should_expand_a_tilde_in_an_include_path_against_home() {
        let env = env_with_home(Path::new("/home/demo"));
        let candidates = include_candidates(Path::new("~/code/api"), Path::new("/cfg"), &env);

        assert_eq!(candidates[0], PathBuf::from("/home/demo/code/api"));
    }
}
