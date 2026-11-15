use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use thiserror::Error;

use crate::config::{SplitDirection, WaitFor};

pub mod cli;

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceOpts {
    pub label: String,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceCreated {
    pub workspace_id: String,
    pub tab_id: String,
    pub root_pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSummary {
    pub workspace_id: String,
    pub label: String,
}

/// Index of the workspaces that already exist, by label.
///
/// Identity is label-only: herdr cannot observe the commands or environment
/// of a running pane, so any deeper comparison would be guesswork. The id
/// kept per label is what `workspace focus` needs when a skipped workspace
/// still requests focus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExistingWorkspaces {
    ids_by_label: HashMap<String, String>,
}

impl ExistingWorkspaces {
    #[must_use]
    pub fn from_list(list: impl IntoIterator<Item = WorkspaceSummary>) -> Self {
        Self {
            ids_by_label: list
                .into_iter()
                .map(|w| (w.label, w.workspace_id))
                .collect(),
        }
    }

    #[must_use]
    pub fn id_of(&self, label: &str) -> Option<&str> {
        self.ids_by_label.get(label).map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabOpts {
    pub label: Option<String>,
    pub cwd: Option<PathBuf>,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabCreated {
    pub tab_id: String,
    pub root_pane_id: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SplitOpts {
    pub direction: SplitDirection,
    pub ratio: Option<f64>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub focus: bool,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("backend error: {message}")]
    Herdr { message: String },
    #[error("herdr command failed (exit code {code:?}): {stderr}")]
    CommandFailed { code: Option<i32>, stderr: String },
}

pub trait HerdrBackend {
    /// List the workspaces that currently exist.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceSummary>, BackendError>;

    /// Focus an existing workspace.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn focus_workspace(&mut self, workspace_id: &str) -> Result<(), BackendError>;

    /// Create a new workspace.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn create_workspace(&mut self, opts: &WorkspaceOpts) -> Result<WorkspaceCreated, BackendError>;

    /// Rename an existing tab.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn rename_tab(&mut self, tab_id: &str, label: &str) -> Result<(), BackendError>;

    /// Create a new tab inside the given workspace.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn create_tab(
        &mut self,
        workspace_id: &str,
        opts: &TabOpts,
    ) -> Result<TabCreated, BackendError>;

    /// Split an existing pane.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn split_pane(&mut self, from_pane: &str, opts: &SplitOpts) -> Result<String, BackendError>;

    /// Run a command inside the given pane.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn run(&mut self, pane_id: &str, command: &str) -> Result<(), BackendError>;

    /// Wait for output matching a pattern inside the given pane.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn wait_output(&mut self, pane_id: &str, wait: &WaitFor) -> Result<(), BackendError>;

    /// Focus the given pane.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn focus_pane(&mut self, pane_id: &str) -> Result<(), BackendError>;
}
