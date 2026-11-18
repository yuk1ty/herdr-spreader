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

/// One workspace of the live session, as `herdr workspace list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceState {
    pub workspace_id: String,
    pub label: String,
    pub number: u32,
    pub focused: bool,
    pub active_tab_id: Option<String>,
}

impl From<WorkspaceState> for WorkspaceSummary {
    fn from(state: WorkspaceState) -> Self {
        Self {
            workspace_id: state.workspace_id,
            label: state.label,
        }
    }
}

/// One tab of the live session, as `herdr tab list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabState {
    pub tab_id: String,
    pub label: String,
    pub number: u32,
    pub focused: bool,
}

/// One pane of the live session, as `herdr pane list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneEntry {
    pub pane_id: String,
    pub tab_id: String,
    pub foreground_cwd: Option<PathBuf>,
}

/// A cell rectangle within a tab, in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPane {
    pub pane_id: String,
    pub rect: Rect,
}

/// The pane geometry of one tab, as `herdr pane layout` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLayout {
    pub area: Rect,
    pub panes: Vec<LayoutPane>,
    pub focused_pane_id: Option<String>,
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
    /// List the workspaces that currently exist, with their live state.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceState>, BackendError>;

    /// List the tabs of one workspace, in their live order.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn list_tabs(&mut self, workspace_id: &str) -> Result<Vec<TabState>, BackendError>;

    /// List the panes of one workspace with their current shell directories.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn list_panes(&mut self, workspace_id: &str) -> Result<Vec<PaneEntry>, BackendError>;

    /// Read the pane geometry of the tab that contains `pane_id`.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if the underlying command fails.
    fn pane_layout(&mut self, pane_id: &str) -> Result<TabLayout, BackendError>;

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
