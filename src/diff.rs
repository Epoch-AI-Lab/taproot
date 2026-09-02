use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::state::TaprootState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Breaking,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub path: String,
    pub kind: DiffKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub version: String,
    pub baseline: EndpointInfo,
    pub current: EndpointInfo,
    pub drifted: bool,
    pub has_breaking: bool,
    pub diffs: Vec<FieldDiff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointInfo {
    pub path: String,
    pub hash: String,
    pub signed: bool,
}

pub fn diff_states(
    baseline: &TaprootState,
    current: &TaprootState,
    strict: bool,
) -> Vec<FieldDiff> {
    let mut diffs = Vec::new();

    // version — always breaking
    if baseline.version != current.version {
        diffs.push(FieldDiff {
            path: "version".into(),
            kind: DiffKind::Changed,
            expected: Some(baseline.version.clone()),
            actual: Some(current.version.clone()),
            severity: Severity::Breaking,
        });
    }

    // base — repo is breaking, branch/commit configurable
    if baseline.base.repo != current.base.repo {
        diffs.push(FieldDiff {
            path: "base.repo".into(),
            kind: DiffKind::Changed,
            expected: Some(baseline.base.repo.clone()),
            actual: Some(current.base.repo.clone()),
            severity: Severity::Breaking,
        });
    }
    if baseline.base.branch != current.base.branch {
        diffs.push(FieldDiff {
            path: "base.branch".into(),
            kind: DiffKind::Changed,
            expected: Some(baseline.base.branch.clone()),
            actual: Some(current.base.branch.clone()),
            severity: if strict {
                Severity::Breaking
            } else {
                Severity::Warning
            },
        });
    }
    if baseline.base.commit != current.base.commit {
        diffs.push(FieldDiff {
            path: "base.commit".into(),
            kind: DiffKind::Changed,
            expected: Some(baseline.base.commit.clone()),
            actual: Some(current.base.commit.clone()),
            severity: if strict {
                Severity::Breaking
            } else {
                Severity::Warning
            },
        });
    }

    // runtimes — keyed by name
    let base_rts: BTreeMap<&str, &crate::state::Runtime> = baseline
        .runtimes
        .iter()
        .map(|r| (r.name.as_str(), r))
        .collect();
    let cur_rts: BTreeMap<&str, &crate::state::Runtime> = current
        .runtimes
        .iter()
        .map(|r| (r.name.as_str(), r))
        .collect();
    let all_rt_names: BTreeSet<&str> = base_rts
        .keys()
        .copied()
        .chain(cur_rts.keys().copied())
        .collect();
    for name in all_rt_names {
        match (base_rts.get(name), cur_rts.get(name)) {
            (Some(b), Some(c)) => {
                if b.version != c.version {
                    diffs.push(FieldDiff {
                        path: format!("runtimes.{name}.version"),
                        kind: DiffKind::Changed,
                        expected: Some(b.version.clone()),
                        actual: Some(c.version.clone()),
                        severity: Severity::Breaking,
                    });
                }
                if b.pinned != c.pinned {
                    diffs.push(FieldDiff {
                        path: format!("runtimes.{name}.pinned"),
                        kind: DiffKind::Changed,
                        expected: Some(b.pinned.to_string()),
                        actual: Some(c.pinned.to_string()),
                        severity: if strict {
                            Severity::Breaking
                        } else {
                            Severity::Warning
                        },
                    });
                }
            }
            (Some(b), None) => diffs.push(FieldDiff {
                path: format!("runtimes.{name}"),
                kind: DiffKind::Removed,
                expected: Some(b.version.clone()),
                actual: None,
                severity: Severity::Breaking,
            }),
            (None, Some(c)) => diffs.push(FieldDiff {
                path: format!("runtimes.{name}"),
                kind: DiffKind::Added,
                expected: None,
                actual: Some(c.version.clone()),
                severity: Severity::Breaking,
            }),
            (None, None) => unreachable!(),
        }
    }

    // containers — keyed by name
    let base_ct: BTreeMap<&str, &crate::state::Container> = baseline
        .containers
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    let cur_ct: BTreeMap<&str, &crate::state::Container> = current
        .containers
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    let all_ct_names: BTreeSet<&str> = base_ct
        .keys()
        .copied()
        .chain(cur_ct.keys().copied())
        .collect();
    for name in all_ct_names {
        match (base_ct.get(name), cur_ct.get(name)) {
            (Some(b), Some(c)) => {
                if b.version != c.version {
                    diffs.push(FieldDiff {
                        path: format!("containers.{name}.version"),
                        kind: DiffKind::Changed,
                        expected: Some(b.version.clone()),
                        actual: Some(c.version.clone()),
                        severity: Severity::Breaking,
                    });
                }
                if b.image != c.image {
                    diffs.push(FieldDiff {
                        path: format!("containers.{name}.image"),
                        kind: DiffKind::Changed,
                        expected: Some(b.image.clone()),
                        actual: Some(c.image.clone()),
                        severity: Severity::Breaking,
                    });
                }
                if b.signed != c.signed {
                    // flipping signed to false is always breaking (supply chain)
                    let sev = if (!c.signed && b.signed) || strict {
                        Severity::Breaking
                    } else {
                        Severity::Warning
                    };
                    diffs.push(FieldDiff {
                        path: format!("containers.{name}.signed"),
                        kind: DiffKind::Changed,
                        expected: Some(b.signed.to_string()),
                        actual: Some(c.signed.to_string()),
                        severity: sev,
                    });
                }
            }
            (Some(b), None) => diffs.push(FieldDiff {
                path: format!("containers.{name}"),
                kind: DiffKind::Removed,
                expected: Some(b.image.clone()),
                actual: None,
                severity: Severity::Breaking,
            }),
            (None, Some(c)) => diffs.push(FieldDiff {
                path: format!("containers.{name}"),
                kind: DiffKind::Added,
                expected: None,
                actual: Some(c.image.clone()),
                severity: Severity::Breaking,
            }),
            (None, None) => unreachable!(),
        }
    }

    // env_vars — BTreeMap already sorted
    let all_keys: BTreeSet<&String> = baseline
        .env_vars
        .keys()
        .chain(current.env_vars.keys())
        .collect();
    for k in all_keys {
        match (baseline.env_vars.get(k), current.env_vars.get(k)) {
            (Some(b), Some(c)) if b != c => diffs.push(FieldDiff {
                path: format!("env_vars.{k}"),
                kind: DiffKind::Changed,
                expected: Some(b.clone()),
                actual: Some(c.clone()),
                severity: Severity::Breaking,
            }),
            (Some(b), None) => diffs.push(FieldDiff {
                path: format!("env_vars.{k}"),
                kind: DiffKind::Removed,
                expected: Some(b.clone()),
                actual: None,
                severity: Severity::Breaking,
            }),
            (None, Some(c)) => diffs.push(FieldDiff {
                path: format!("env_vars.{k}"),
                kind: DiffKind::Added,
                expected: None,
                actual: Some(c.clone()),
                severity: Severity::Breaking,
            }),
            _ => {}
        }
    }

    // notes — warning unless strict then breaking
    if baseline.notes != current.notes {
        diffs.push(FieldDiff {
            path: "notes".into(),
            kind: DiffKind::Changed,
            expected: baseline.notes.clone(),
            actual: current.notes.clone(),
            severity: if strict {
                Severity::Breaking
            } else {
                Severity::Warning
            },
        });
    }

    // created_at is intentionally ignored — always drifts

    diffs
}

pub fn has_breaking(diffs: &[FieldDiff]) -> bool {
    diffs.iter().any(|d| d.severity == Severity::Breaking)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TaprootState;

    fn base() -> TaprootState {
        TaprootState::new("myapp", "main", "abc123")
            .with_runtime("python", "3.11.4")
            .with_env("FOO", "bar")
    }

    #[test]
    fn no_diff_on_identical() {
        let s = base();
        let mut b = s.clone();
        let mut c = s.clone();
        // created_at differs but ignored
        b.created_at = chrono::Utc::now();
        c.created_at = chrono::Utc::now();
        assert!(diff_states(&b, &c, true).is_empty());
        assert!(diff_states(&b, &c, false).is_empty());
    }

    #[test]
    fn detects_env_added() {
        let b = base();
        let mut c = b.clone();
        c.env_vars.insert("NEW".into(), "1".into());
        let diffs = diff_states(&b, &c, false);
        assert!(diffs
            .iter()
            .any(|d| d.path == "env_vars.NEW" && d.kind == DiffKind::Added));
        assert!(has_breaking(&diffs));
    }

    #[test]
    fn detects_runtime_version_change() {
        let b = base();
        let mut c = b.clone();
        c.runtimes[0].version = "3.12.0".into();
        let diffs = diff_states(&b, &c, false);
        assert!(diffs.iter().any(|d| d.path == "runtimes.python.version"));
    }

    #[test]
    fn ignores_created_at() {
        let mut b = base();
        let mut c = base();
        b.created_at = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        c.created_at = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(diff_states(&b, &c, true).is_empty());
    }

    #[test]
    fn notes_is_warning_unless_strict() {
        let b = base();
        let mut c = b.clone();
        c.notes = Some("hello".into());
        assert_eq!(diff_states(&b, &c, false)[0].severity, Severity::Warning);
        assert_eq!(diff_states(&b, &c, true)[0].severity, Severity::Breaking);
    }

    #[test]
    fn branch_is_warning_unless_strict() {
        let b = base();
        let mut c = b.clone();
        c.base.branch = "feat/foo".into();
        assert_eq!(
            diff_states(&b, &c, false)
                .iter()
                .find(|d| d.path == "base.branch")
                .unwrap()
                .severity,
            Severity::Warning
        );
        assert_eq!(
            diff_states(&b, &c, true)
                .iter()
                .find(|d| d.path == "base.branch")
                .unwrap()
                .severity,
            Severity::Breaking
        );
    }
}
