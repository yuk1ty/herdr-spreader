use std::collections::BTreeMap;
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

/// A workspace that already exists on the server.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSummary {
    pub workspace_id: String,
    /// Herdr allows an unlabelled workspace, which can never match a layout.
    pub label: Option<String>,
}

/// A tab that already exists inside a workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct TabSummary {
    pub tab_id: String,
    pub label: Option<String>,
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

    /// List the workspaces that already exist on the server.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceSummary>, BackendError>;

    /// List the tabs of an existing workspace.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn list_tabs(&mut self, workspace_id: &str) -> Result<Vec<TabSummary>, BackendError>;

    /// Focus an existing workspace.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn focus_workspace(&mut self, workspace_id: &str) -> Result<(), BackendError>;

    /// Focus the given pane.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn focus_pane(&mut self, pane_id: &str) -> Result<(), BackendError>;
}
