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

    let identifies_integration = status.lines().any(|line| {
        let line = line.trim();
        line.strip_prefix("On branch ") == Some("integration")
            || line.strip_prefix("On channel ") == Some("integration")
    });
    if !identifies_integration {
        return Err(invalid(
            "pijul status output did not identify the exact integration channel; refusing to assume it is clean",
        ));
    }

    if status.contains("Unrecorded changes") || status.contains("Untracked files") {
        return Err(invalid(
            "integration has unrecorded or untracked work; refusing to classify it as clean",
        ));
    }

    let mut in_conflicts = false;
    let mut paths = Vec::new();
    for line in status.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed == "On branch integration"
            || trimmed == "On channel integration"
        {
            continue;
        }
        if trimmed == "Unresolved conflicts:" {
            if in_conflicts {
                return Err(invalid(
                    "pijul status repeated the unresolved-conflicts section",
                ));
            }
            in_conflicts = true;
            continue;
        }
        if in_conflicts {
            if trimmed.starts_with("(fix conflicts") {
                continue;
            }
            if !line.starts_with(' ') && !line.starts_with('\t') {
                return Err(invalid(format!(
                    "unrecognized pijul status line after unresolved-conflicts section: {trimmed:?}"
                )));
            }
            paths.push(trimmed.to_owned());
            continue;
        }
        return Err(invalid(format!(
            "unrecognized pijul status output; refusing to assume clean: {trimmed:?}"
        )));
    }

    if !in_conflicts {
        return Ok(vec![Triage::Clean]);
    }
    if paths.is_empty() {
        return Err(invalid(
            "pijul status reported unresolved conflicts but no conflict paths could be parsed",
        ));
    }

    let mut seen = std::collections::BTreeSet::new();
    for path in &paths {
        if !seen.insert(path.clone()) {
            return Err(invalid(format!(
                "pijul status repeated conflict path {path:?}; refusing ambiguous triage"
            )));
        }
    }

    let mut reports = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = Path::new(&path);
        let safe = relative.components().all(|component| {
            matches!(component, Component::Normal(_) | Component::CurDir)
        });
        let conflict_state = if safe {
            read_reported_file(workdir, relative)
        } else {
            "Pijul reported a conflict at a non-relative path; content was not read.".into()
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
    fn clean_status_requires_an_explicit_integration_channel() {
        let clean = "On branch integration\n";
        assert!(matches!(
            parse_status_output(Path::new("."), clean).unwrap().as_slice(),
            [Triage::Clean]
        ));
        assert_eq!(
            parse_status_output(Path::new("."), "no channel reported\n")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
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
