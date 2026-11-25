use std::collections::HashSet;
use std::path::Path;

use crate::config::{SpreadFile, Tab};
use crate::proofs::PaneSig;

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationFinding {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SourceFile<'a> {
    pub yaml: &'a str,
    pub path: &'a Path,
}

/// Validate a config file's contents: parse the YAML, then run semantic checks.
///
/// # Errors
///
/// Returns a `Vec<ValidationFinding>` if the YAML fails to parse (a single
/// `Severity::Error` finding naming `source.path`) or if any semantic finding
/// (error *or* warning) is reported; otherwise returns the parsed [`SpreadFile`].
pub fn validate_config(source: SourceFile<'_>) -> Result<SpreadFile, Vec<ValidationFinding>> {
    let file = SpreadFile::from_str(source.yaml).map_err(|err| {
        vec![ValidationFinding {
            severity: Severity::Error,
            message: format!(
                "failed to parse config file {}: {err}",
                source.path.display()
            ),
        }]
    })?;
    let findings = validate(&file);
    if findings.is_empty() {
        Ok(file)
    } else {
        Err(findings)
    }
}

pub(crate) fn validate(file: &SpreadFile) -> Vec<ValidationFinding> {
    let mut findings = Vec::new();

    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut focus_count = 0usize;

    for ws in &file.workspaces {
        if ws.name.is_empty() {
            findings.push(ValidationFinding {
                severity: Severity::Error,
                message: "workspace name must not be empty".to_string(),
            });
        }
        if !seen_names.insert(ws.name.as_str()) {
            findings.push(ValidationFinding {
                severity: Severity::Error,
                message: format!("duplicate workspace name '{}'", ws.name),
            });
        }
        if ws.focus {
            focus_count += 1;
        }

        for tab in &ws.tabs {
            let tab_label = tab.label.as_deref().unwrap_or("(unnamed)");
            if tab.panes.is_empty() {
                findings.push(ValidationFinding {
                    severity: Severity::Warning,
                    message: format!("tab '{tab_label}' in workspace '{}' has no panes", ws.name),
                });
            }

            findings.extend(validate_pane_split_sources(&ws.name, tab_label, tab));

            for pane in &tab.panes {
                if let Some(ratio) = pane.ratio
                    && (ratio <= 0.0 || ratio >= 1.0)
                {
                    findings.push(ValidationFinding {
                        severity: Severity::Error,
                        message: format!(
                            "pane ratio {ratio} in workspace '{}' must be between 0 and 1 (exclusive)",
                            ws.name
                        ),
                    });
                }

                if pane.wait_for.is_some() && pane.command.is_none() {
                    findings.push(ValidationFinding {
                        severity: Severity::Error,
                        message: format!(
                            "wait_for was specified on a pane without a command in workspace '{}'",
                            ws.name
                        ),
                    });
                }
            }
        }
    }

    if focus_count > 1 {
        findings.push(ValidationFinding {
            severity: Severity::Warning,
            message: format!(
                "{focus_count} workspaces have `focus: true`; only the last one will receive focus"
            ),
        });
    }

    findings
}

use verus_builtin_macros::{verus, verus_verify};
// Used only by the `verus!` block below; erased builds see no vstd items.
#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

#[cfg(feature = "verus")]
use crate::proofs::{
    declares_nonempty_id, declared_ids, lemma_declared_ids_member,
    lemma_declared_ids_witness, lemma_duplicate_id_breaks_rules,
    lemma_from_invalid_breaks_rules, sig_from_chars, sig_id_chars, split_rules_ok,
    split_rules_ok_up_to,
};

/// A pane split-rule violation, identified by pane index within the tab.
///
/// The decision kernel ([`pane_split_findings`]) produces these; the message
/// layer turns them into user-facing [`ValidationFinding`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitFinding {
    /// Pane 0 used `from`, but it is the tab's root pane, not a split.
    RootPaneFrom { pane: usize },
    /// A non-first pane used `from` that no earlier pane in the tab declared.
    FromInvalid { pane: usize },
    /// A pane declared an empty id.
    EmptyPaneId { pane: usize },
    /// A pane declared an id an earlier pane in the same tab already declared.
    DuplicatePaneId { pane: usize },
}

/// A lemma-side membership check, verified against its spec so the kernel's
/// `contains` calls are not opaque to the prover.
#[verus_verify(rlimit(20))]
fn contains_id(ids: &[String], target: &String) -> (found: bool)
    ensures
        found == exists|k: int| 0 <= k < ids@.len() && ids@[k]@ == target@,
{
    let mut found: bool = false;
    let mut k: usize = 0;
    while k < ids.len()
        invariant
            k <= ids@.len(),
            found == (exists|j: int| 0 <= j < k && ids@[j]@ == target@),
        decreases ids@.len() - k,
    {
        found = found || ids[k] == *target;
        proof {
            assert(found == (exists|j: int| 0 <= j < k + 1 && ids@[j]@ == target@));
        }
        k += 1;
    }
    found
}

/// Check a sequence of pane signatures against the split rules.
///
/// Reports no finding exactly when the signatures satisfy the split rules
/// ([`split_rules_ok`]): the first pane is not a split, every `from` names an
/// id declared on an earlier pane, and ids are non-empty and unique within
/// the sequence.
#[verus_verify(rlimit(60))]
fn pane_split_findings(sigs: &[PaneSig]) -> (findings: Vec<SplitFinding>)
    ensures
        findings@.len() == 0 <==> split_rules_ok(sigs@),
{
    let mut findings: Vec<SplitFinding> = Vec::new();
    let mut earlier_pane_ids: Vec<String> = Vec::new();
    let mut pane_index: usize = 0;
    while pane_index < sigs.len()
        invariant
            pane_index <= sigs@.len(),
            earlier_pane_ids@ == declared_ids(sigs@, pane_index as int),
            findings@.len() == 0 <==> split_rules_ok_up_to(sigs@, pane_index as int),
        decreases sigs@.len() - pane_index,
    {
        let sig = &sigs[pane_index];
        let ghost i = pane_index as int;
        proof {
            assert(sigs@[i] == *sig);
            assert(sigs@[i].0 == sig.0);
            assert(sigs@[i].1 == sig.1);
        }
        let from_declared: bool = match &sig.1 {
            Some(from) => contains_id(&earlier_pane_ids, from),
            None => false,
        };
        let id_duplicate: bool = match &sig.0 {
            Some(id) if !id.is_empty() => contains_id(&earlier_pane_ids, id),
            _ => false,
        };
        if let Some(from) = &sig.1 {
            let _ = from;
            if pane_index == 0 {
                findings.push(SplitFinding::RootPaneFrom { pane: pane_index });
                proof {
                    assert(sigs@[0].1 is Some);
                    assert(!split_rules_ok_up_to(sigs@, i + 1));
                };
            } else if !from_declared {
                findings.push(SplitFinding::FromInvalid { pane: pane_index });
                proof {
                    assert(from@ == sig_from_chars(sigs@[i]));
                    lemma_from_invalid_breaks_rules(sigs@, earlier_pane_ids@, i);
                };
            } else {
                proof {
                    assert(from@ == sig_from_chars(sigs@[i]));
                    assert(exists|k: int|
                        0 <= k < earlier_pane_ids@.len()
                            && earlier_pane_ids@[k]@ == sig_from_chars(sigs@[i]));
                    lemma_declared_ids_witness(sigs@, i, sig_from_chars(sigs@[i]));
                };
            }
        }
        if let Some(id) = &sig.0 {
            if id.is_empty() {
                findings.push(SplitFinding::EmptyPaneId { pane: pane_index });
                proof {
                    assert(!declares_nonempty_id(sigs@, i));
                    assert(!split_rules_ok_up_to(sigs@, i + 1));
                };
            } else if id_duplicate {
                findings.push(SplitFinding::DuplicatePaneId { pane: pane_index });
                proof {
                    assert(id@ == sig_id_chars(sigs@[i]));
                    lemma_duplicate_id_breaks_rules(sigs@, i);
                };
            } else {
                proof {
                    if findings@.len() == 0 {
                        assert forall|q: int|
                            0 <= q < i && sigs@[q].0 is Some
                            implies sig_id_chars(sigs@[q]) != sig_id_chars(sigs@[i])
                        by {
                            if sig_id_chars(sigs@[q]) == sig_id_chars(sigs@[i]) {
                                lemma_declared_ids_member(sigs@, i, q);
                                assert(id_duplicate);
                                assert(false);
                            }
                        }
                    }
                }
                earlier_pane_ids.push(id.clone());
                proof {
                    assert(declares_nonempty_id(sigs@, i));
                    assert(earlier_pane_ids@ == declared_ids(sigs@, i + 1));
                };
            }
        }
        proof {
            if findings@.len() == 0 {
                // No finding was produced for pane i, so pane i satisfies the
                // rules; earlier panes satisfy them by the invariant.
                if sig.0 is Some && sig_id_chars(sigs@[i]).len() == 0 {
                    assert(false);
                }
                assert(split_rules_ok_up_to(sigs@, i + 1));
            }
        }
        pane_index += 1;
    }
    findings
}

} // verus!

/// Check a tab's pane `id`/`from` declarations and report findings with
/// human-readable messages.
fn validate_pane_split_sources(
    ws_name: &str,
    tab_label: &str,
    tab: &Tab,
) -> Vec<ValidationFinding> {
    let all_pane_ids: Vec<&str> = tab.panes.iter().filter_map(|p| p.id.as_deref()).collect();
    let sigs: Vec<PaneSig> = tab
        .panes
        .iter()
        .map(|p| (p.id.clone(), p.from.clone()))
        .collect();
    pane_split_findings(&sigs)
        .into_iter()
        .map(|finding| {
            let message = match &finding {
                SplitFinding::RootPaneFrom { .. } => format!(
                    "the first pane in tab '{tab_label}' of workspace '{ws_name}' cannot use `from`: it is the tab's root pane, not a split"
                ),
                SplitFinding::FromInvalid { pane } => {
                    let from = sigs[*pane]
                        .1
                        .as_deref()
                        .expect("FromInvalid names a pane with `from` set");
                    let reason = if all_pane_ids.contains(&from) {
                        "`from` may only reference a pane declared earlier in the same tab"
                    } else {
                        "no pane in this tab declares that id"
                    };
                    format!(
                        "pane `from: {from}` in tab '{tab_label}' of workspace '{ws_name}' is invalid: {reason}"
                    )
                }
                SplitFinding::EmptyPaneId { .. } => format!(
                    "pane id in tab '{tab_label}' of workspace '{ws_name}' must not be empty"
                ),
                SplitFinding::DuplicatePaneId { pane } => {
                    let id = sigs[*pane]
                        .0
                        .as_deref()
                        .expect("DuplicatePaneId names a pane with an id");
                    format!("duplicate pane id '{id}' in tab '{tab_label}' of workspace '{ws_name}'")
                }
            };
            ValidationFinding {
                severity: Severity::Error,
                message,
            }
        })
        .collect()
}

pub fn print_findings(findings: &[ValidationFinding]) {
    for finding in findings {
        let tag = match finding.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
        };
        eprintln!("[{tag}] {}", finding.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Pane, SpreadFile, Tab, WaitFor, Workspace};

    #[test]
    fn should_return_empty_findings_list_given_valid_minimal_config() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert!(findings.is_empty());
    }

    #[test]
    fn should_report_error_given_workspace_with_empty_name() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: String::new(),
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("empty"));
    }

    #[test]
    fn should_report_error_given_duplicate_workspace_names() {
        let file = SpreadFile {
            workspaces: vec![
                Workspace {
                    name: "frontend".to_string(),
                    ..Default::default()
                },
                Workspace {
                    name: "frontend".to_string(),
                    ..Default::default()
                },
            ],
        };
        let findings = validate(&file);
        let dup_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("duplicate"))
            .collect();
        assert_eq!(dup_findings.len(), 1);
        assert_eq!(dup_findings[0].severity, Severity::Error);
    }

    #[test]
    fn should_report_error_given_ratio_equal_to_zero() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        ratio: Some(0.0),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn should_report_error_given_ratio_equal_to_one() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        ratio: Some(1.0),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn should_report_error_given_negative_ratio() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        ratio: Some(-0.5),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn should_skip_ratio_check_when_ratio_is_none() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        ratio: None,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert!(findings.is_empty());
    }

    #[test]
    fn should_accept_valid_ratio_between_zero_and_one() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        ratio: Some(0.5),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert!(findings.is_empty());
    }

    #[test]
    fn should_report_warning_given_multiple_focused_workspaces() {
        let file = SpreadFile {
            workspaces: vec![
                Workspace {
                    name: "alpha".to_string(),
                    focus: true,
                    ..Default::default()
                },
                Workspace {
                    name: "beta".to_string(),
                    focus: false,
                    ..Default::default()
                },
                Workspace {
                    name: "gamma".to_string(),
                    focus: true,
                    ..Default::default()
                },
            ],
        };
        let findings = validate(&file);
        let focus_warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Warning && f.message.contains("focus"))
            .collect();
        assert_eq!(focus_warnings.len(), 1);
    }

    #[test]
    fn should_not_report_warning_given_exactly_one_focused_workspace() {
        let file = SpreadFile {
            workspaces: vec![
                Workspace {
                    name: "alpha".to_string(),
                    focus: true,
                    ..Default::default()
                },
                Workspace {
                    name: "beta".to_string(),
                    ..Default::default()
                },
            ],
        };
        let findings = validate(&file);
        let focus_warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("focus"))
            .collect();
        assert!(focus_warnings.is_empty());
    }

    #[test]
    fn should_not_report_warning_given_zero_focused_workspaces() {
        let file = SpreadFile {
            workspaces: vec![
                Workspace {
                    name: "alpha".to_string(),
                    ..Default::default()
                },
                Workspace {
                    name: "beta".to_string(),
                    ..Default::default()
                },
            ],
        };
        let findings = validate(&file);
        let focus_warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("focus"))
            .collect();
        assert!(focus_warnings.is_empty());
    }

    #[test]
    fn should_report_warning_given_tab_with_no_panes() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        let tab_warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Warning && f.message.contains("no panes"))
            .collect();
        assert_eq!(tab_warnings.len(), 1);
    }

    #[test]
    fn should_not_report_warning_given_tab_with_panes() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: None,
                    panes: vec![Pane {
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        let tab_warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("no panes"))
            .collect();
        assert!(tab_warnings.is_empty());
    }

    #[test]
    fn should_report_error_given_pane_with_wait_for_but_no_command() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: None,
                        wait_for: Some(WaitFor {
                            pattern: "ready".to_string(),
                            timeout_ms: None,
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        let wait_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("wait_for"))
            .collect();
        assert_eq!(wait_findings.len(), 1);
        assert_eq!(wait_findings[0].severity, Severity::Error);
    }

    #[test]
    fn should_not_report_wait_for_error_given_pane_with_both_command_and_wait_for() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    panes: vec![Pane {
                        command: Some("cargo run".to_string()),
                        wait_for: Some(WaitFor {
                            pattern: "ready".to_string(),
                            timeout_ms: None,
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        let wait_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("wait_for"))
            .collect();
        assert!(wait_findings.is_empty());
    }

    fn pane_with_id(id: &str) -> Pane {
        Pane {
            id: Some(id.to_string()),
            ..Default::default()
        }
    }

    fn pane_from(from: &str) -> Pane {
        Pane {
            from: Some(from.to_string()),
            ..Default::default()
        }
    }

    fn single_tab_file(panes: Vec<Pane>) -> SpreadFile {
        SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![Tab {
                    label: Some("main".to_string()),
                    panes,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    #[test]
    fn should_return_empty_findings_given_branching_layout_with_valid_from_references() {
        let file = single_tab_file(vec![
            pane_with_id("editor"),
            Pane {
                id: Some("agent".to_string()),
                from: Some("editor".to_string()),
                ..Default::default()
            },
            pane_from("editor"),
        ]);
        let findings = validate(&file);
        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    fn sig(id: Option<&str>, from: Option<&str>) -> (Option<String>, Option<String>) {
        (id.map(str::to_string), from.map(str::to_string))
    }

    #[test]
    fn should_report_no_kernel_findings_given_valid_branching_layout() {
        let sigs = vec![
            sig(Some("editor"), None),
            sig(Some("agent"), Some("editor")),
            sig(None, Some("editor")),
        ];
        assert_eq!(pane_split_findings(&sigs), vec![]);
    }

    #[test]
    fn should_report_root_from_and_invalid_from_in_pane_order() {
        let sigs = vec![sig(None, Some("editor")), sig(None, Some("git"))];
        assert_eq!(
            pane_split_findings(&sigs),
            vec![
                SplitFinding::RootPaneFrom { pane: 0 },
                SplitFinding::FromInvalid { pane: 1 },
            ]
        );
    }

    #[test]
    fn should_report_empty_and_duplicate_ids_in_pane_order() {
        let sigs = vec![
            sig(Some(""), None),
            sig(Some("editor"), None),
            sig(Some("editor"), None),
        ];
        assert_eq!(
            pane_split_findings(&sigs),
            vec![
                SplitFinding::EmptyPaneId { pane: 0 },
                SplitFinding::DuplicatePaneId { pane: 2 },
            ]
        );
    }

    #[test]
    fn should_not_register_empty_ids_as_earlier_ids() {
        let sigs = vec![sig(Some(""), None), sig(Some(""), None)];
        assert_eq!(
            pane_split_findings(&sigs),
            vec![
                SplitFinding::EmptyPaneId { pane: 0 },
                SplitFinding::EmptyPaneId { pane: 1 },
            ]
        );
    }

    #[test]
    fn should_report_error_given_duplicate_pane_ids_in_same_tab() {
        let file = single_tab_file(vec![pane_with_id("editor"), pane_with_id("editor")]);
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("duplicate pane id 'editor'"));
    }

    #[test]
    fn should_not_report_error_given_same_pane_id_reused_in_different_tabs() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![
                    Tab {
                        panes: vec![pane_with_id("editor")],
                        ..Default::default()
                    },
                    Tab {
                        panes: vec![pane_with_id("editor")],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    #[test]
    fn should_report_error_given_empty_pane_id() {
        let file = single_tab_file(vec![pane_with_id("")]);
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("must not be empty"));
    }

    #[test]
    fn should_report_error_given_from_on_the_first_pane_of_a_tab() {
        let file = single_tab_file(vec![pane_from("editor"), pane_with_id("editor")]);
        let findings = validate(&file);
        let from_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("root pane"))
            .collect();
        assert_eq!(from_findings.len(), 1);
        assert_eq!(from_findings[0].severity, Severity::Error);
    }

    #[test]
    fn should_report_error_given_from_referencing_a_pane_declared_later() {
        let file = single_tab_file(vec![
            pane_with_id("editor"),
            pane_from("git"),
            pane_with_id("git"),
        ]);
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("earlier"));
    }

    #[test]
    fn should_report_error_given_from_referencing_the_panes_own_id() {
        let file = single_tab_file(vec![
            pane_with_id("editor"),
            Pane {
                id: Some("agent".to_string()),
                from: Some("agent".to_string()),
                ..Default::default()
            },
        ]);
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("earlier"));
    }

    #[test]
    fn should_report_error_given_from_referencing_an_unknown_id() {
        let file = single_tab_file(vec![pane_with_id("editor"), pane_from("editr")]);
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(
            findings[0]
                .message
                .contains("no pane in this tab declares that id")
        );
    }

    #[test]
    fn should_report_error_given_from_referencing_an_id_declared_in_another_tab() {
        let file = SpreadFile {
            workspaces: vec![Workspace {
                name: "demo".to_string(),
                tabs: vec![
                    Tab {
                        panes: vec![pane_with_id("editor")],
                        ..Default::default()
                    },
                    Tab {
                        panes: vec![Pane::default(), pane_from("editor")],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        };
        let findings = validate(&file);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(
            findings[0]
                .message
                .contains("no pane in this tab declares that id")
        );
    }

    #[test]
    fn should_return_ok_with_spread_file_given_clean_yaml() {
        let source = SourceFile {
            yaml: "workspaces:\n  - name: demo\n",
            path: Path::new("config.yaml"),
        };
        let result = validate_config(source);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().workspaces[0].name, "demo");
    }

    #[test]
    fn should_return_err_with_parse_finding_given_malformed_yaml() {
        let source = SourceFile {
            yaml: "name: demo\ntabs: []",
            path: Path::new("/cfg/spread.yaml"),
        };
        let result = validate_config(source);
        let err = result.unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].severity, Severity::Error);
        assert!(err[0].message.contains("/cfg/spread.yaml"));
        assert!(err[0].message.to_lowercase().contains("parse"));
    }

    #[test]
    fn should_return_err_with_findings_given_duplicate_workspace_names() {
        let source = SourceFile {
            yaml: "workspaces:\n  - name: frontend\n  - name: frontend\n",
            path: Path::new("c.yaml"),
        };
        let result = validate_config(source);
        let err = result.unwrap_err();
        assert!(
            err.iter()
                .any(|f| f.message.contains("duplicate") && f.severity == Severity::Error)
        );
    }

    #[test]
    fn should_return_err_given_warning_only_config_with_empty_tab() {
        let source = SourceFile {
            yaml: "workspaces:\n  - name: demo\n    tabs:\n      - panes: []\n",
            path: Path::new("c.yaml"),
        };
        let result = validate_config(source);
        let err = result.unwrap_err();
        assert!(
            err.iter()
                .any(|f| f.severity == Severity::Warning && f.message.contains("no panes"))
        );
    }

    #[test]
    fn should_pass_validation_given_bundled_example_config() {
        let source = SourceFile {
            yaml: include_str!("../examples/config.yaml"),
            path: Path::new("examples/config.yaml"),
        };
        assert!(validate_config(source).is_ok());
    }
}
