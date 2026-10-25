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

fn parse_status_output(workdir: &Path, status: &str) -> std::io::Result<Vec<Triage>> {
    // Accept only the exact channel labels emitted by the known status format.
    // Substring matching could accidentally accept e.g. "integration-old".
    let channel_lines: Vec<_> = status
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("On branch ") || line.starts_with("On channel "))
        .collect();
    if channel_lines.len() != 1
        || !matches!(channel_lines[0], "On branch integration" | "On channel integration")
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pijul status did not identify exactly the integration channel; refusing to assume it is clean",
        ));
    }

    if status.contains("Unrecorded changes") || status.contains("Untracked files") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "integration has unrecorded or untracked work; refusing to classify it as clean",
        ));
    }

    let mut in_conflicts = false;
    let mut saw_conflict_section = false;
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
            if saw_conflict_section {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "pijul status repeated the unresolved-conflicts section",
                ));
            }
            saw_conflict_section = true;
            in_conflicts = true;
            continue;
        }

        if !in_conflicts {
            // Once the channel header is accounted for, any other non-empty
            // status line must be understood. Unknown sections must not become Clean.
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unrecognized pijul status line: {trimmed:?}"),
            ));
        }

        if trimmed.starts_with("(fix conflicts") {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unexpected line in pijul unresolved-conflicts section: {trimmed:?}"),
            ));
        }
        paths.push(trimmed.to_owned());
    }

    if !saw_conflict_section {
        return Ok(vec![Triage::Clean]);
    }
    if paths.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pijul status reported unresolved conflicts but no conflict paths could be parsed",
        ));
    }

    let mut reports = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = Path::new(&path);
        let safe = relative.components().all(|component| {
            matches!(component, Component::Normal(_) | Component::CurDir)
        });
        let conflict_state = if safe {
            let file = workdir.join(relative);
            match std::fs::symlink_metadata(&file) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    std::fs::read_to_string(&file).unwrap_or_else(|_| {
                        "Pijul reported a conflict; file is binary, unreadable, or unavailable."
                            .into()
                    })
                }
                _ => "Pijul reported a conflict; path is not a regular file, so content was not read."
                    .into(),
            }
        } else {
            "Pijul reported a conflict at a non-relative path; content not read.".into()
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
    fn unknown_status_sections_fail_closed_instead_of_reporting_clean() {
        for status in [
            "On branch integration\\nFuture status section:\\n  item.txt\\n",
            "On branch integration\\nChanges not understood by this parser\\n",
            "On branch integration-old\\n",
            "On branch integration\\nUnresolved conflicts:\\n  (fix conflicts and record the resolution)\\n",
        ] {
            assert_eq!(
                parse_status_output(Path::new("."), status)
                    .expect_err("unknown or malformed status must not be treated as clean")
                    .kind(),
                std::io::ErrorKind::InvalidData,
                "unexpectedly accepted status: {status:?}"
            );
        }
    }

    #[test]
    fn unknown_status_lines_after_conflict_heading_fail_closed() {
        let status = "On branch integration\\nUnresolved conflicts:\\n  conflict.txt\\nUnexpected footer\\n";
        assert_eq!(
            parse_status_output(Path::new("."), status)
                .expect_err("unexpected trailing status text must be rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
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
