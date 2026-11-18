//! Session export: capture the live herdr session and rebuild a spread file
//! from it, mirroring herdr's workspace → tab → pane model.
//!
//! The data flow is Actions → Data → Calculations: `capture_snapshot` reads
//! the session through the backend, `build_spread_file` turns the snapshot
//! into the same [`SpreadFile`] type `apply` consumes, and `render_yaml`
//! serializes it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use thiserror::Error;

use crate::backend::{BackendError, HerdrBackend, PaneEntry, Rect, WorkspaceState};
use crate::config::{Pane, SplitDirection, SpreadFile, Tab, Workspace};

#[derive(Debug, Error)]
pub enum ExportError {
    #[error(
        "tab '{tab}' in workspace '{workspace}' cannot be represented as a spread file: {detail}"
    )]
    Unrepresentable {
        workspace: String,
        tab: String,
        detail: String,
    },
    #[error(
        "duplicate workspace labels {labels:?}: a config file cannot hold two workspaces of the same name"
    )]
    DuplicateWorkspaceLabels { labels: Vec<String> },
    #[error("the herdr session has no workspaces to export")]
    EmptySession,
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("failed to serialize the exported layout: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub workspaces: Vec<WorkspaceSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSnapshot {
    pub label: String,
    pub focused: bool,
    pub active_tab_id: Option<String>,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabSnapshot {
    pub tab_id: String,
    pub label: String,
    pub panes: Vec<PaneSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneSnapshot {
    pub pane_id: String,
    pub rect: Rect,
    pub cwd: Option<PathBuf>,
    pub focused: bool,
}

/// Read the whole session — every workspace, tab, pane rect, and shell
/// directory — into a [`SessionSnapshot`].
///
/// # Errors
///
/// Returns [`ExportError::Backend`] if any herdr query fails.
pub fn capture_snapshot(backend: &mut dyn HerdrBackend) -> Result<SessionSnapshot, ExportError> {
    let mut workspaces = backend.list_workspaces()?;
    workspaces.sort_by_key(|w| w.number);

    let snapshots = workspaces
        .iter()
        .map(|ws| capture_workspace(backend, ws))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SessionSnapshot {
        workspaces: snapshots,
    })
}

fn capture_workspace(
    backend: &mut dyn HerdrBackend,
    ws: &WorkspaceState,
) -> Result<WorkspaceSnapshot, BackendError> {
    let mut tabs = backend.list_tabs(&ws.workspace_id)?;
    tabs.sort_by_key(|t| t.number);
    let panes = backend.list_panes(&ws.workspace_id)?;

    let tab_snapshots = tabs
        .iter()
        .map(|tab| {
            let tab_panes: Vec<&PaneEntry> =
                panes.iter().filter(|p| p.tab_id == tab.tab_id).collect();
            capture_tab(backend, tab, &tab_panes)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(WorkspaceSnapshot {
        label: ws.label.clone(),
        focused: ws.focused,
        active_tab_id: ws.active_tab_id.clone(),
        tabs: tab_snapshots,
    })
}

fn capture_tab(
    backend: &mut dyn HerdrBackend,
    tab: &crate::backend::TabState,
    tab_panes: &[&PaneEntry],
) -> Result<TabSnapshot, BackendError> {
    // Any pane of the tab answers `pane layout` for the whole tab; a herdr
    // tab always has at least one pane, so an empty listing is a broken
    // response rather than a layout shape.
    let probe = tab_panes.first().ok_or_else(|| BackendError::Herdr {
        message: format!("herdr listed no panes for tab {}", tab.tab_id),
    })?;
    let layout = backend.pane_layout(&probe.pane_id)?;

    let panes = layout
        .panes
        .iter()
        .map(|layout_pane| {
            let cwd = tab_panes
                .iter()
                .find(|p| p.pane_id == layout_pane.pane_id)
                .and_then(|p| p.foreground_cwd.clone());
            PaneSnapshot {
                pane_id: layout_pane.pane_id.clone(),
                rect: layout_pane.rect,
                cwd,
                focused: layout
                    .focused_pane_id
                    .as_deref()
                    .is_some_and(|id| id == layout_pane.pane_id),
            }
        })
        .collect();

    Ok(TabSnapshot {
        tab_id: tab.tab_id.clone(),
        label: tab.label.clone(),
        panes,
    })
}

/// How one pane of a tab came to exist: as the tab's root pane (`None`), or
/// as a split of an earlier pane with the direction and the ratio of the
/// extent the source pane kept.
#[derive(Debug, Clone, PartialEq)]
struct SplitPlan {
    pane_id: String,
    created_by: Option<(String, SplitDirection, f64)>,
}

/// Rebuild the split sequence that produces a tab's pane rects.
///
/// The `splits` array of herdr's `pane layout` response is not in execution
/// order (measured on herdr 0.9.1), so the sequence is reconstructed from the
/// pane rects alone. Measured split semantics: a split keeps
/// `round(ratio * extent)` cells of its source pane on the left/top and puts
/// the new pane on the right/bottom, and splitting the same pane again
/// divides its remaining rect. So every full-length cut of an exactly tiled
/// region corresponds to one split of the region's top-left pane, with
/// `ratio = kept_extent / region_extent`.
///
/// Panes are emitted in pre-order over the cuts — each cut's new pane before
/// the cuts inside either half — so that replaying the sequence splits every
/// source pane while it still covers the whole region its ratio was measured
/// against.
fn reconstruct_tab_panes(panes: &[(String, Rect)]) -> Result<Vec<SplitPlan>, String> {
    if panes.is_empty() {
        return Err("herdr listed no panes for this tab".to_string());
    }
    let area = bounding_box(panes);
    validate_tiling(panes, area)?;

    let root = pane_id_at(panes, area.x, area.y)
        .ok_or_else(|| "no pane covers the top-left corner of the tab".to_string())?;
    let mut plans = vec![SplitPlan {
        pane_id: root,
        created_by: None,
    }];
    collect_splits(area, panes, &mut plans)?;
    Ok(plans)
}

fn bounding_box(panes: &[(String, Rect)]) -> Rect {
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0;
    let mut max_y = 0;
    for (_, rect) in panes {
        min_x = min_x.min(rect.x);
        min_y = min_y.min(rect.y);
        max_x = max_x.max(rect.x + rect.width);
        max_y = max_y.max(rect.y + rect.height);
    }
    Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

fn validate_tiling(panes: &[(String, Rect)], area: Rect) -> Result<(), String> {
    for (pane_id, rect) in panes {
        if rect.width == 0 || rect.height == 0 {
            return Err(format!("pane {pane_id} has a zero width or height"));
        }
        if rect.x < area.x
            || rect.y < area.y
            || rect.x + rect.width > area.x + area.width
            || rect.y + rect.height > area.y + area.height
        {
            return Err(format!("pane {pane_id} lies outside the tab area"));
        }
    }
    for left in 0..panes.len() {
        for right in (left + 1)..panes.len() {
            if overlaps(panes[left].1, panes[right].1) {
                return Err(format!(
                    "panes {} and {} overlap",
                    panes[left].0, panes[right].0
                ));
            }
        }
    }
    let pane_area: u64 = panes
        .iter()
        .map(|(_, rect)| u64::from(rect.width) * u64::from(rect.height))
        .sum();
    let total_area = u64::from(area.width) * u64::from(area.height);
    if pane_area != total_area {
        return Err(
            "the panes do not tile the tab exactly: gaps between panes make the layout unreproducible"
                .to_string(),
        );
    }
    Ok(())
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn pane_id_at(panes: &[(String, Rect)], x: u32, y: u32) -> Option<String> {
    panes
        .iter()
        .find(|(_, rect)| {
            rect.x <= x && x < rect.x + rect.width && rect.y <= y && y < rect.y + rect.height
        })
        .map(|(pane_id, _)| pane_id.clone())
}

fn region_contains(region: Rect, rect: Rect) -> bool {
    rect.x >= region.x
        && rect.y >= region.y
        && rect.x + rect.width <= region.x + region.width
        && rect.y + rect.height <= region.y + region.height
}

fn collect_splits(
    region: Rect,
    panes: &[(String, Rect)],
    out: &mut Vec<SplitPlan>,
) -> Result<(), String> {
    let inside: Vec<&(String, Rect)> = panes
        .iter()
        .filter(|(_, rect)| region_contains(region, *rect))
        .collect();
    if inside.len() <= 1 {
        return Ok(());
    }

    let Some((direction, cut)) = find_cut(region, &inside) else {
        return Err(
            "no full-length cut divides the panes: the layout is not expressible as sequential right/down splits (for example a pinwheel layout or manually moved panes)"
                .to_string(),
        );
    };

    let (source_region, new_region) = match direction {
        SplitDirection::Right => (
            Rect {
                width: cut - region.x,
                ..region
            },
            Rect {
                x: cut,
                width: region.x + region.width - cut,
                ..region
            },
        ),
        SplitDirection::Down => (
            Rect {
                height: cut - region.y,
                ..region
            },
            Rect {
                y: cut,
                height: region.y + region.height - cut,
                ..region
            },
        ),
    };

    let source_pane = pane_id_at(panes, region.x, region.y)
        .expect("an exactly tiled region has a pane at its top-left corner");
    let new_pane = pane_id_at(panes, new_region.x, new_region.y)
        .expect("an exactly tiled region has a pane at its top-left corner");
    let ratio = match direction {
        SplitDirection::Right => f64::from(cut - region.x) / f64::from(region.width),
        SplitDirection::Down => f64::from(cut - region.y) / f64::from(region.height),
    };

    out.push(SplitPlan {
        pane_id: new_pane,
        created_by: Some((source_pane, direction, ratio)),
    });
    collect_splits(source_region, panes, out)?;
    collect_splits(new_region, panes, out)
}

/// Find the cut line for a region: a vertical line strictly inside it that
/// every pane lies entirely left or right of, preferring the smallest such
/// line, and only failing that the same search among horizontal lines.
fn find_cut(region: Rect, panes: &[&(String, Rect)]) -> Option<(SplitDirection, u32)> {
    if let Some(cut) = best_cut(region, panes, Axis::X) {
        return Some((SplitDirection::Right, cut));
    }
    best_cut(region, panes, Axis::Y).map(|cut| (SplitDirection::Down, cut))
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

fn best_cut(region: Rect, panes: &[&(String, Rect)], axis: Axis) -> Option<u32> {
    let (origin, extent) = match axis {
        Axis::X => (region.x, region.width),
        Axis::Y => (region.y, region.height),
    };
    let edges = panes.iter().filter_map(|(_, rect)| {
        let (rect_origin, rect_extent) = match axis {
            Axis::X => (rect.x, rect.width),
            Axis::Y => (rect.y, rect.height),
        };
        let edge = rect_origin + rect_extent;
        (edge > origin && edge < origin + extent).then_some(edge)
    });
    edges
        .filter(|&cut| {
            panes.iter().all(|(_, rect)| match axis {
                Axis::X => rect.x + rect.width <= cut || rect.x >= cut,
                Axis::Y => rect.y + rect.height <= cut || rect.y >= cut,
            })
        })
        .min()
}

/// Turn a captured session into the [`SpreadFile`] that reproduces it.
///
/// # Errors
///
/// Returns [`ExportError::EmptySession`] for a session with no workspaces,
/// [`ExportError::DuplicateWorkspaceLabels`] when two workspaces share a
/// label (config names must be unique), and
/// [`ExportError::Unrepresentable`] for any layout whose split topology
/// cannot be rebuilt — export then fails rather than silently writing a
/// different layout.
pub fn build_spread_file(snapshot: &SessionSnapshot) -> Result<SpreadFile, ExportError> {
    if snapshot.workspaces.is_empty() {
        return Err(ExportError::EmptySession);
    }

    let mut label_counts: HashMap<&str, usize> = HashMap::new();
    for ws in &snapshot.workspaces {
        *label_counts.entry(ws.label.as_str()).or_insert(0) += 1;
    }
    let mut duplicates: Vec<String> = label_counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(label, _)| label.to_string())
        .collect();
    duplicates.sort();
    if !duplicates.is_empty() {
        return Err(ExportError::DuplicateWorkspaceLabels { labels: duplicates });
    }

    let workspaces = snapshot
        .workspaces
        .iter()
        .map(build_workspace)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SpreadFile { workspaces })
}

fn build_workspace(ws: &WorkspaceSnapshot) -> Result<Workspace, ExportError> {
    if ws.label.is_empty() {
        return Err(ExportError::Unrepresentable {
            workspace: String::new(),
            tab: String::new(),
            detail: "the workspace label is empty, but workspace names must be non-empty"
                .to_string(),
        });
    }
    let tabs = ws
        .tabs
        .iter()
        .map(|tab| build_tab(ws, tab))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Workspace {
        name: ws.label.clone(),
        root: None,
        env: BTreeMap::new(),
        tabs,
        focus: ws.focused,
    })
}

fn build_tab(ws: &WorkspaceSnapshot, tab: &TabSnapshot) -> Result<Tab, ExportError> {
    let unrepresentable = |detail: String| ExportError::Unrepresentable {
        workspace: ws.label.clone(),
        tab: tab.label.clone(),
        detail,
    };

    let rects: Vec<(String, Rect)> = tab
        .panes
        .iter()
        .map(|pane| (pane.pane_id.clone(), pane.rect))
        .collect();
    let plans = reconstruct_tab_panes(&rects).map_err(unrepresentable)?;

    let position: HashMap<&str, usize> = plans
        .iter()
        .enumerate()
        .map(|(index, plan)| (plan.pane_id.as_str(), index))
        .collect();
    let referenced: HashSet<&str> = plans
        .iter()
        .enumerate()
        .filter_map(|(index, plan)| {
            let (source, _, _) = plan.created_by.as_ref()?;
            let source_position = position
                .get(source.as_str())
                .copied()
                .expect("a split source is always an earlier pane of the same tab");
            // A pane that chains from its predecessor emits no `from`, so
            // the predecessor needs no id.
            (source_position + 1 != index).then_some(source.as_str())
        })
        .collect();
    let is_active_tab = ws.active_tab_id.as_deref() == Some(tab.tab_id.as_str());

    let panes = plans
        .iter()
        .enumerate()
        .map(|(index, plan)| {
            let snapshot_pane = tab
                .panes
                .iter()
                .find(|pane| pane.pane_id == plan.pane_id)
                .expect("every reconstructed pane comes from the tab's own panes");
            let (split, ratio, from) = match &plan.created_by {
                None => (SplitDirection::Right, None, None),
                Some((source, direction, ratio)) => {
                    let source_position = position
                        .get(source.as_str())
                        .copied()
                        .expect("a split source is always an earlier pane of the same tab");
                    let chains_from_previous = source_position + 1 == index;
                    let from = (!chains_from_previous).then(|| format!("p{}", source_position + 1));
                    (*direction, Some(*ratio), from)
                }
            };
            Pane {
                id: referenced
                    .contains(plan.pane_id.as_str())
                    .then(|| format!("p{}", index + 1)),
                command: None,
                cwd: snapshot_pane.cwd.clone(),
                env: BTreeMap::new(),
                split,
                from,
                ratio,
                wait_for: None,
                focus: is_active_tab && snapshot_pane.focused,
            }
        })
        .collect();

    Ok(Tab {
        label: Some(tab.label.clone()),
        cwd: None,
        panes,
    })
}

/// Serialize an exported spread file to YAML.
///
/// # Errors
///
/// Returns [`ExportError::Yaml`] if serialization fails.
pub fn render_yaml(file: &SpreadFile) -> Result<String, ExportError> {
    serde_yaml_ng::to_string(file).map_err(ExportError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(pane_id: &str, x: u32, y: u32, width: u32, height: u32) -> PaneSnapshot {
        PaneSnapshot {
            pane_id: pane_id.to_string(),
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            cwd: None,
            focused: false,
        }
    }

    fn with_cwd(mut pane: PaneSnapshot, cwd: &str) -> PaneSnapshot {
        pane.cwd = Some(PathBuf::from(cwd));
        pane
    }

    fn tab(tab_id: &str, label: &str, panes: Vec<PaneSnapshot>) -> TabSnapshot {
        TabSnapshot {
            tab_id: tab_id.to_string(),
            label: label.to_string(),
            panes,
        }
    }

    fn workspace(
        label: &str,
        focused: bool,
        active_tab_id: Option<&str>,
        tabs: Vec<TabSnapshot>,
    ) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            label: label.to_string(),
            focused,
            active_tab_id: active_tab_id.map(ToString::to_string),
            tabs,
        }
    }

    fn session(workspaces: Vec<WorkspaceSnapshot>) -> SessionSnapshot {
        SessionSnapshot { workspaces }
    }

    #[test]
    fn should_reconstruct_a_two_pane_chain_as_root_plus_one_split_with_exact_ratio() {
        let snapshot = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![
                    pane("wA:p1", 0, 0, 119, 73),
                    with_cwd(pane("wA:p2", 119, 0, 277, 73), "/home/demo/project/logs"),
                ],
            )],
        )]);

        let file = build_spread_file(&snapshot).unwrap();

        assert_eq!(file.workspaces[0].root, None);
        assert_eq!(file.workspaces[0].tabs[0].label, Some("main".to_string()));
        let panes = &file.workspaces[0].tabs[0].panes;
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].id, None);
        assert_eq!(panes[0].from, None);
        assert_eq!(panes[0].ratio, None);
        assert_eq!(panes[0].cwd, None);
        assert_eq!(panes[1].id, None);
        assert_eq!(panes[1].from, None);
        assert_eq!(panes[1].split, SplitDirection::Right);
        assert_eq!(panes[1].ratio, Some(119.0 / 396.0));
        assert_eq!(panes[1].cwd, Some(PathBuf::from("/home/demo/project/logs")));
    }

    #[test]
    fn should_reconstruct_a_2x2_grid_with_ids_and_from_references() {
        let snapshot = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![
                    pane("a", 0, 0, 50, 50),
                    pane("b", 50, 0, 50, 50),
                    pane("c", 0, 50, 50, 50),
                    pane("d", 50, 50, 50, 50),
                ],
            )],
        )]);

        let file = build_spread_file(&snapshot).unwrap();
        let panes = &file.workspaces[0].tabs[0].panes;

        assert_eq!(panes.len(), 4);
        // Emission order: a (root), b (right split off a, the previous
        // pane), c (down split off a, needs an id + from), d (down split
        // off b, needs an id + from).
        assert_eq!(panes[0].id, Some("p1".to_string()));
        assert_eq!(panes[1].id, Some("p2".to_string()));
        assert_eq!(panes[1].from, None);
        assert_eq!(panes[1].split, SplitDirection::Right);
        assert_eq!(panes[1].ratio, Some(0.5));
        assert_eq!(panes[2].id, None);
        assert_eq!(panes[2].from, Some("p1".to_string()));
        assert_eq!(panes[2].split, SplitDirection::Down);
        assert_eq!(panes[2].ratio, Some(0.5));
        assert_eq!(panes[3].id, None);
        assert_eq!(panes[3].from, Some("p2".to_string()));
        assert_eq!(panes[3].split, SplitDirection::Down);
        assert_eq!(panes[3].ratio, Some(0.5));
    }

    #[test]
    fn should_reconstruct_the_measured_asymmetric_probe_layout_with_its_ratios() {
        // Live herdr 0.9.1 probe: a 396x73 tab with an editor column split
        // asymmetrically and the right column split once more.
        let snapshot = session(vec![workspace(
            "probe",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![
                    pane("p1", 0, 0, 119, 37),
                    pane("p4", 0, 37, 119, 36),
                    pane("p2", 119, 0, 277, 51),
                    pane("p3", 119, 51, 277, 22),
                ],
            )],
        )]);

        let file = build_spread_file(&snapshot).unwrap();
        let panes = &file.workspaces[0].tabs[0].panes;

        assert_eq!(panes.len(), 4);
        // Creation order p1, p2, p4, p3: the right column first (root split
        // right), then each column's internal down split.
        assert_eq!(panes[0].id, Some("p1".to_string()));
        assert_eq!(panes[1].id, Some("p2".to_string()));
        assert_eq!(panes[1].from, None);
        assert_eq!(panes[1].ratio, Some(119.0 / 396.0));
        assert_eq!(panes[2].from, Some("p1".to_string()));
        assert_eq!(panes[2].split, SplitDirection::Down);
        assert_eq!(panes[2].ratio, Some(37.0 / 73.0));
        assert_eq!(panes[3].from, Some("p2".to_string()));
        assert_eq!(panes[3].split, SplitDirection::Down);
        assert_eq!(panes[3].ratio, Some(51.0 / 73.0));
    }

    #[test]
    fn should_reconstruct_three_equal_columns_as_chained_splits_without_ids() {
        let snapshot = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![
                    pane("a", 0, 0, 30, 40),
                    pane("b", 30, 0, 30, 40),
                    pane("c", 60, 0, 30, 40),
                ],
            )],
        )]);

        let file = build_spread_file(&snapshot).unwrap();
        let panes = &file.workspaces[0].tabs[0].panes;

        assert_eq!(panes.len(), 3);
        assert!(panes.iter().all(|p| p.id.is_none()));
        assert!(panes.iter().all(|p| p.from.is_none()));
        assert_eq!(panes[1].ratio, Some(1.0 / 3.0));
        assert_eq!(panes[2].ratio, Some(0.5));
    }

    #[test]
    fn should_fail_with_unrepresentable_given_a_pinwheel_layout() {
        // A pinwheel tiling has no full-length cut, so no sequence of
        // right/down splits can reproduce it.
        let snapshot = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![
                    pane("a", 0, 0, 70, 40),
                    pane("b", 70, 0, 30, 70),
                    pane("c", 30, 70, 70, 30),
                    pane("d", 0, 40, 30, 60),
                    pane("e", 30, 40, 40, 30),
                ],
            )],
        )]);

        let err = build_spread_file(&snapshot).unwrap_err();

        match err {
            ExportError::Unrepresentable {
                workspace,
                tab,
                detail,
            } => {
                assert_eq!(workspace, "demo");
                assert_eq!(tab, "main");
                assert!(detail.contains("no full-length cut"), "{detail}");
            }
            other => panic!("expected Unrepresentable, got {other:?}"),
        }
    }

    #[test]
    fn should_fail_with_unrepresentable_given_panes_with_gaps_or_overlaps() {
        let gaps = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![pane("a", 0, 0, 50, 50), pane("b", 60, 0, 40, 50)],
            )],
        )]);
        let err = build_spread_file(&gaps).unwrap_err();
        match err {
            ExportError::Unrepresentable { detail, .. } => {
                assert!(detail.contains("gaps"), "{detail}");
            }
            other => panic!("expected Unrepresentable, got {other:?}"),
        }

        let overlaps = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab(
                "t1",
                "main",
                vec![pane("a", 0, 0, 60, 50), pane("b", 50, 0, 50, 50)],
            )],
        )]);
        let err = build_spread_file(&overlaps).unwrap_err();
        match err {
            ExportError::Unrepresentable { detail, .. } => {
                assert!(detail.contains("overlap"), "{detail}");
            }
            other => panic!("expected Unrepresentable, got {other:?}"),
        }
    }

    #[test]
    fn should_fail_with_duplicate_workspace_labels_given_two_workspaces_of_one_label() {
        let snapshot = session(vec![
            workspace("demo", false, None, vec![]),
            workspace("demo", false, None, vec![]),
        ]);

        let err = build_spread_file(&snapshot).unwrap_err();

        match err {
            ExportError::DuplicateWorkspaceLabels { labels } => {
                assert_eq!(labels, vec!["demo".to_string()]);
            }
            other => panic!("expected DuplicateWorkspaceLabels, got {other:?}"),
        }
    }

    #[test]
    fn should_fail_with_empty_session_given_no_workspaces() {
        let err = build_spread_file(&session(vec![])).unwrap_err();
        assert!(matches!(err, ExportError::EmptySession));
    }

    #[test]
    fn should_focus_only_the_focused_pane_of_the_active_tab_of_the_focused_workspace() {
        let mut focused_editor = pane("t1p2", 119, 0, 277, 73);
        focused_editor.focused = true;
        let mut focused_other_tab_pane = pane("t2p1", 0, 0, 396, 73);
        focused_other_tab_pane.focused = true;
        let snapshot = session(vec![
            workspace(
                "active",
                true,
                Some("t1"),
                vec![
                    tab("t1", "1", vec![pane("t1p1", 0, 0, 119, 73), focused_editor]),
                    tab("t2", "editor", vec![focused_other_tab_pane]),
                ],
            ),
            workspace(
                "background",
                false,
                None,
                vec![tab("t3", "1", vec![pane("t3p1", 0, 0, 396, 73)])],
            ),
        ]);

        let file = build_spread_file(&snapshot).unwrap();

        assert!(file.workspaces[0].focus);
        assert!(!file.workspaces[1].focus);
        let active_tab_panes = &file.workspaces[0].tabs[0].panes;
        assert!(!active_tab_panes[0].focus);
        assert!(active_tab_panes[1].focus);
        assert!(
            file.workspaces[0].tabs[1].panes.iter().all(|p| !p.focus),
            "a focused pane of a non-active tab must not keep focus"
        );
        assert!(
            file.workspaces[1].tabs[0].panes.iter().all(|p| !p.focus),
            "an unfocused workspace must not focus any pane"
        );
    }

    #[test]
    fn should_emit_tab_labels_even_when_they_equal_their_number_string() {
        let snapshot = session(vec![workspace(
            "demo",
            false,
            None,
            vec![tab("t1", "1", vec![pane("a", 0, 0, 100, 50)])],
        )]);

        let file = build_spread_file(&snapshot).unwrap();

        assert_eq!(file.workspaces[0].tabs[0].label, Some("1".to_string()));
        assert_eq!(file.workspaces[0].tabs[0].panes.len(), 1);
        assert_eq!(file.workspaces[0].tabs[0].panes[0].id, None);
        assert_eq!(file.workspaces[0].tabs[0].panes[0].ratio, None);
    }
}
