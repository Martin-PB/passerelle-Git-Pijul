//! Triage: classify what remains after applying all PR changes onto the
//! integration channel.
//!
//! P0 only detects unresolved conflicts. Deterministic auto-resolution rules
//! and path-to-PR attribution are intentionally deferred; this module must
//! never report Clean merely because conflict inspection was skipped.

use crate::operator::ConflictReport;
use std::path::{Component, Path};
use std::process::Command;

#[derive(Debug, Clone)]
pub enum Triage {
    /// No unresolved conflicts were reported by Pijul.
    Clean,
    /// Resolved by rule; reserved for the later deterministic rules stage.
    AutoResolved { rule: Rule },
    /// Needs an operator decision.
    Operator { report: ConflictReport },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    SameContent,
    CanonicalFormatting,
    DeclaredMergeSemantics,
}

/// Inspect `pijul status` on the current integration working copy. A failure
/// to inspect status is an error, never a clean result. The first P0 version
/// reports the file contents and leaves PR attribution empty rather than
/// guessing which incoming branch caused each conflict.
pub fn triage_integration_channel(workdir: &Path) -> std::io::Result<Vec<Triage>> {
    let switch = Command::new("pijul")
        .args(["channel", "switch", "integration"])
        .current_dir(workdir)
        .output()?;
    if !switch.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "cannot switch to integration before triage: {}",
                String::from_utf8_lossy(&switch.stderr).trim()
            ),
        ));
    }

    let output = Command::new("pijul")
        .arg("status")
        .current_dir(workdir)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "pijul status failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let status = String::from_utf8_lossy(&output.stdout);
    parse_status_output(workdir, &status)
}

/// Read a reported path only when every existing path component is a
/// non-symlink directory and the final component is a regular file. This
/// prevents conflict paths from traversing symlinks into data outside the
/// Pijul working copy. It is not a defence against a hostile concurrent
/// process replacing components between checks and the final read.
fn read_reported_file(workdir: &Path, relative: &Path) -> String {
    let Ok(root) = workdir.canonicalize() else {
        return "Pijul reported a conflict, but the repository root could not be resolved; content was not read."
            .into();
    };
    let components: Vec<_> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_owned()),
            Component::CurDir => None,
            _ => Some(std::ffi::OsString::new()),
        })
        .collect();
    if components.is_empty() || components.iter().any(|part| part.is_empty()) {
        return "Pijul reported a conflict at a non-relative or empty path; content was not read."
            .into();
    }

    let mut current = root.clone();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => {
                return "Pijul reported a conflict path that is unavailable; content was not read."
                    .into();
            }
        };
        if metadata.file_type().is_symlink() {
            return "Pijul reported a conflict path traversing a symbolic link; content was not read."
                .into();
        }
        if index + 1 < components.len() {
            if !metadata.is_dir() {
                return "Pijul reported a conflict path with a non-directory parent; content was not read."
                    .into();
            }
        } else if !metadata.is_file() {
            return "Pijul reported a conflict path that is not a regular file; content was not read."
                .into();
        }
    }

    let resolved = match current.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return "Pijul reported a conflict path that could not be resolved; content was not read."
                .into();
        }
    };
    if !resolved.starts_with(&root) {
        return "Pijul reported a conflict path outside the repository; content was not read."
            .into();
    }
    std::fs::read_to_string(resolved).unwrap_or_else(|_| {
        "Pijul reported a conflict path that is binary or unreadable; content was not read."
            .into()
    })
}

fn parse_status_output(workdir: &Path, status: &str) -> std::io::Result<Vec<Triage>> {
    fn invalid(message: impl Into<String>) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
    }

    // The caller has successfully switched to integration. beta.24 may omit
    // its header, so empty output is a valid clean state; unknown non-empty
    // output is never interpreted as clean.
    let mut section: Option<&str> = None;
    let mut conflict_section_seen = false;
    let mut changes_section_seen = false;
    let mut untracked_section_seen = false;
    let mut paths: Vec<(String, bool)> = Vec::new();

    for line in status.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(
            trimmed,
            "On branch integration" | "On channel integration" | "On channel: integration"
        ) {
            if section.is_some() {
                return Err(invalid("unexpected integration header inside a status section"));
            }
            continue;
        }

        match trimmed {
            "Unresolved conflicts:" => {
                if conflict_section_seen || changes_section_seen || untracked_section_seen {
                    return Err(invalid(
                        "pijul status repeated or mixed incompatible status sections",
                    ));
                }
                conflict_section_seen = true;
                section = Some("conflicts");
                continue;
            }
            "Changes not recorded:" => {
                if conflict_section_seen || changes_section_seen || untracked_section_seen {
                    return Err(invalid(
                        "pijul status repeated or mixed incompatible status sections",
                    ));
                }
                changes_section_seen = true;
                section = Some("changes");
                continue;
            }
            "Untracked files:" => {
                if conflict_section_seen || untracked_section_seen {
                    return Err(invalid(
                        "pijul status repeated or mixed incompatible status sections",
                    ));
                }
                untracked_section_seen = true;
                section = Some("untracked");
                continue;
            }
            _ => {}
        }

        if trimmed.starts_with('(') {
            if section.is_some() {
                continue;
            }
            return Err(invalid(format!(
                "unrecognized pijul status output; refusing to assume clean: {trimmed:?}"
            )));
        }

        let indented = line.starts_with(' ') || line.starts_with('\t');
        match section {
            Some("conflicts") => {
                if !indented {
                    return Err(invalid(format!(
                        "unrecognized pijul status line in conflict section: {trimmed:?}"
                    )));
                }
                paths.push((trimmed.to_owned(), true));
            }
            Some("changes") => {
                if !indented {
                    return Err(invalid(format!(
                        "unrecognized pijul status line in changes section: {trimmed:?}"
                    )));
                }
                let mut parts = trimmed.splitn(2, char::is_whitespace);
                let _operation = parts.next();
                let path = parts.next().unwrap_or("").trim();
                if path.is_empty() {
                    return Err(invalid(format!(
                        "cannot parse path from Pijul status entry: {trimmed:?}"
                    )));
                }
                paths.push((path.to_owned(), false));
            }
            Some("untracked") => {
                if !indented {
                    return Err(invalid(format!(
                        "unrecognized pijul status line in untracked section: {trimmed:?}"
                    )));
                }
                paths.push((trimmed.to_owned(), false));
            }
            _ => {
                return Err(invalid(format!(
                    "unrecognized pijul status output; refusing to assume clean: {trimmed:?}"
                )));
            }
        }
    }

    if paths.is_empty() {
        if conflict_section_seen {
            return Err(invalid(
                "pijul status reported unresolved conflicts but no conflict paths could be parsed",
            ));
        }
        if changes_section_seen || untracked_section_seen {
            return Err(invalid(
                "pijul status reported unrecorded or untracked work but no paths could be parsed",
            ));
        }
        return Ok(vec![Triage::Clean]);
    }

    let mut seen = std::collections::BTreeSet::new();
    for (path, _) in &paths {
        if !seen.insert(path.clone()) {
            return Err(invalid(format!(
                "pijul status repeated path {path:?}; refusing ambiguous triage"
            )));
        }
    }

    let dirty_without_conflict_section = !conflict_section_seen;
    let mut reports = Vec::with_capacity(paths.len());
    for (path, is_conflict) in paths {
        let relative = Path::new(&path);
        let safe = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
        let file_contents = if safe {
            read_reported_file(workdir, relative)
        } else {
            "Pijul reported a non-relative path; content was not read.".into()
        };
        let conflict_state = if dirty_without_conflict_section && !is_conflict {
            format!(
                "Pijul status did not expose an explicit unresolved-conflict section. This path is reported as dirty/untracked and requires operator review.\n\n{}",
                file_contents
            )
        } else {
            file_contents
        };
        reports.push(Triage::Operator {
            report: ConflictReport {
                path,
                conflict_state,
                prs: Vec::new(),
            },
        });
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::{parse_status_output, Triage};
    use std::path::Path;

    #[test]
    fn clean_status_accepts_empty_or_known_integration_header() {
        for clean in [
            "",
            "On branch integration\n",
            "On channel integration\n",
            "On channel: integration\n",
        ] {
            assert!(matches!(
                parse_status_output(Path::new("."), clean).unwrap().as_slice(),
                [Triage::Clean]
            ));
        }
    }

    #[test]
    fn channel_name_must_match_exactly() {
        let status = "On branch not-integration\n";
        assert_eq!(
            parse_status_output(Path::new("."), status)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn unknown_status_output_is_not_misclassified_as_clean() {
        let status = "On channel integration\n\nStatus format changed unexpectedly\n";
        assert_eq!(
            parse_status_output(Path::new("."), status)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn duplicate_conflict_paths_are_rejected() {
        let status = "On channel integration\n\nUnresolved conflicts:\n  (fix conflicts and record the resolution)\n    src/lib.rs\n    src/lib.rs\n";
        assert_eq!(
            parse_status_output(Path::new("."), status)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn channel_header_variant_is_supported() {
        let status = "On channel integration\n";
        assert!(matches!(
            parse_status_output(Path::new("."), status).unwrap().as_slice(),
            [Triage::Clean]
        ));
    }

    #[test]
    fn dirty_beta24_status_without_conflict_section_is_routed_to_operator() {
        let root = tempfile::tempdir().expect("repository root");
        std::fs::write(root.path().join("docs.md"), "dirty content\n").expect("write file");
        let status = "Changes not recorded:\n  (use record)\n        moved   docs.md\n\nUntracked files:\n  (use add)\n        new.md\n";
        let result = parse_status_output(root.path(), status).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|item| matches!(
            item,
            Triage::Operator { report } if report.conflict_state.contains("did not expose an explicit unresolved-conflict section")
        )));
    }

    #[cfg(unix)]
    #[test]
    fn conflict_path_cannot_read_through_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("repository root");
        let outside = tempfile::tempdir().expect("outside directory");
        std::fs::write(outside.path().join("secret.txt"), "outside-secret-content")
            .expect("write outside file");
        symlink(outside.path(), root.path().join("linked-dir")).expect("create symlink");

        let status = "On branch integration\n\nUnresolved conflicts:\n    linked-dir/secret.txt\n";
        let result = parse_status_output(root.path(), status).unwrap();
        match &result[0] {
            Triage::Operator { report } => {
                assert_eq!(report.path, "linked-dir/secret.txt");
                assert!(!report.conflict_state.contains("outside-secret-content"));
                assert!(report.conflict_state.contains("symbolic link"));
            }
            other => panic!("expected operator triage, got {other:?}"),
        }
    }

    #[test]
    fn unresolved_conflict_is_reported_for_operator() {
        let root = tempfile::tempdir().expect("temp root");
        std::fs::write(root.path().join("conflict.txt"), "<<<<<<< conflict\n")
            .expect("write file");
        let status = "On branch integration\n\nUnresolved conflicts:\n  (fix conflicts and record the resolution)\n    conflict.txt\n";
        let result = parse_status_output(root.path(), status).unwrap();
        match &result[0] {
            Triage::Operator { report } => {
                assert_eq!(report.path, "conflict.txt");
                assert!(report.conflict_state.contains("<<<<<<< conflict"));
            }
            other => panic!("expected operator triage, got {other:?}"),
        }
    }
}
