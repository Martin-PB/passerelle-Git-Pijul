//! Triage: classify what remains after applying all PR changes onto the
//! integration channel.
//!
//! P0 surfaces explicit unresolved conflicts and conservatively routes dirty
//! or untracked integration paths to operator review when Pijul does not expose
//! a conflict section. Dirty status is not proof of a semantic conflict.
//! Deterministic auto-resolution and path-to-PR attribution are deferred; this
//! module must never report Clean merely because conflict inspection was skipped.

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
/// prevents a conflict path from traversing a symlink into data outside the
/// Pijul working copy. It is not a defence against a hostile concurrent
/// process replacing components between checks and the final read.
fn read_reported_file(workdir: &Path, relative: &Path) -> String {
    let Ok(root) = workdir.canonicalize() else {
        return "Pijul reported a path, but the repository root could not be resolved; content was not read."
            .into();
    };
    let components: Vec<_> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_owned()),
            Component::CurDir => None,
            _ => {
                // Reject the whole path rather than trying to normalize it.
                Some(std::ffi::OsString::new())
            }
        })
        .collect();
    if components.is_empty() || components.iter().any(|part| part.is_empty()) {
        return "Pijul reported a non-relative or empty path; content was not read.".into();
    }

    let mut current = root.clone();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => {
                return "Pijul reported a path that is unavailable; content was not read.".into();
            }
        };
        if metadata.file_type().is_symlink() {
            return "Pijul reported a path traversing a symbolic link; content was not read.".into();
        }
        if index + 1 < components.len() {
            if !metadata.is_dir() {
                return "Pijul reported a path with a non-directory parent; content was not read."
                    .into();
            }
        } else if !metadata.is_file() {
            return "Pijul reported this path, but it is not a regular file; content was not read."
                .into();
        }
    }

    let resolved = match current.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return "Pijul reported a path that could not be resolved; content was not read.".into();
        }
    };
    if !resolved.starts_with(&root) {
        return "Pijul reported a path outside the repository; content was not read.".into();
    }
    std::fs::read_to_string(resolved)
        .unwrap_or_else(|_| "Pijul reported a path that is binary or unreadable; content was not read.".into())
}

fn parse_status_output(workdir: &Path, status: &str) -> std::io::Result<Vec<Triage>> {
    // The caller has already successfully switched to `integration`.
    // Pijul beta.24 does not always include an explicit conflict section.
    // In that case, dirty/untracked paths must still be surfaced for operator
    // review; they must never be mistaken for a clean integration.
    let mut in_conflicts = false;
    let mut paths = Vec::new();
    for line in status.lines() {
        let trimmed = line.trim();
        if trimmed == "Unresolved conflicts:" {
            in_conflicts = true;
            continue;
        }
        if !in_conflicts {
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("(fix conflicts") {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        paths.push(trimmed.to_owned());
    }

    let dirty_without_conflict_section = !in_conflicts
        && (status.contains("Changes not recorded:") || status.contains("Untracked files:"));
    if dirty_without_conflict_section {
        let mut section = "";
        for line in status.lines() {
            let trimmed = line.trim();
            match trimmed {
                "Changes not recorded:" => {
                    section = "changes";
                    continue;
                }
                "Untracked files:" => {
                    section = "untracked";
                    continue;
                }
                _ => {}
            }
            if trimmed.is_empty() {
                continue;
            }
            if section.is_empty() || trimmed.starts_with('(') {
                continue;
            }
            let path = if section == "changes" {
                // Status entries such as "moved   docs" put the path last.
                trimmed.split_whitespace().last().unwrap_or(trimmed)
            } else {
                trimmed
            };
            paths.push(path.to_owned());
        }
        paths.sort();
        paths.dedup();
    }

    if !in_conflicts && !dirty_without_conflict_section {
        // An empty status is a valid clean result on beta.24. If the CLI emits
        // a header, accept only a known integration header; arbitrary non-empty
        // output must not silently become Clean.
        let unexpected: Vec<_> = status
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && *line != "On channel: integration"
                    && *line != "On branch integration"
            })
            .collect();
        if !unexpected.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unrecognized pijul status output; refusing to classify integration as clean: {}",
                    unexpected.join(" | ")
                ),
            ));
        }
        return Ok(vec![Triage::Clean]);
    }
    if paths.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            if in_conflicts {
                "pijul status reported unresolved conflicts but no conflict paths could be parsed"
            } else {
                "pijul status reports unrecorded or untracked work but no paths could be parsed"
            },
        ));
    }

    let mut reports = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = Path::new(&path);
        let safe = relative.components().all(|component| {
            matches!(component, Component::Normal(_) | Component::CurDir)
        });
        let file_contents = if safe {
            read_reported_file(workdir, relative)
        } else {
            "Pijul reported a non-relative path; content was not read.".into()
        };
        let conflict_state = if dirty_without_conflict_section {
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
    fn clean_status_is_parsed_after_caller_switches_to_integration() {
        // Pijul beta.24 may omit the active channel header from status output.
        let clean = "";
        assert!(matches!(
            parse_status_output(Path::new("."), clean).unwrap().as_slice(),
            [Triage::Clean]
        ));
    }

    #[test]
    fn unknown_nonempty_status_is_not_classified_as_clean() {
        let error = parse_status_output(Path::new("."), "unexpected status output\n")
            .expect_err("unknown status must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn wrong_channel_header_is_not_classified_as_clean() {
        let error = parse_status_output(Path::new("."), "On channel: other\n")
            .expect_err("a different channel header must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn dirty_status_without_explicit_conflicts_is_reported_for_operator() {
        let root = tempfile::tempdir().expect("temp root");
        std::fs::write(root.path().join("docs-b.md"), "dirty content\n")
            .expect("write fixture");
        let status = "On channel: integration\n\nChanges not recorded:\n  (use record)\n\n        moved   docs\n\nUntracked files:\n  (use add)\n\n        docs-b.md\n";
        let result = parse_status_output(root.path(), status).unwrap();
        assert!(result.iter().any(|item| matches!(
            item,
            Triage::Operator { report } if report.path == "docs-b.md"
                && report.conflict_state.contains("did not expose an explicit unresolved-conflict section")
        )));
        assert!(!result.iter().any(|item| matches!(item, Triage::Clean)));
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

        let status = "Unresolved conflicts:\n    linked-dir/secret.txt\n";
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
