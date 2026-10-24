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

    if !in_conflicts {
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
