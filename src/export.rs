//! Export: turn the resolved integration channel into Git objects.
//!
//! The Git-object plumbing lives in `translate::export_integration`; this
//! module documents and plans the two Git shapes the gateway can produce.

use crate::ExportMode;

/// Human-readable plan of what will be pushed, per PR, for a given export
/// mode. Used by the CLI so the operator can confirm the shape before the
/// gateway pushes anything.
///
/// Linear: every change of every PR becomes one commit, in dependency
/// order, authors/dates/messages preserved (stacked history).
/// MergeTopology: one merge commit per PR, parents = (previous tip,
/// linearised PR head), so GitHub renders each PR as a merged branch.
pub fn plan(export_mode: ExportMode, prs: &[u64]) -> Vec<String> {
    match export_mode {
        ExportMode::Linear => prs
            .iter()
            .map(|n| {
                format!(
                    "pr-{n}: one commit per change, dependency order, authors preserved (stacked)"
                )
            })
            .collect(),
        ExportMode::MergeTopology => prs
            .iter()
            .map(|n| format!("pr-{n}: one merge commit (parents: previous tip, pr-{n} head)"))
            .collect(),
    }
}
