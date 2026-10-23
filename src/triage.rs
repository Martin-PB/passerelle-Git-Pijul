//! Triage: classify what remains after applying all PR changes onto the
//! integration channel.
//!
//! Three outcomes, and only three:
//! 1. Auto: the changes commuted, or a deterministic rule resolved the
//!    conflict (identical content on both sides, formatting-only, declared
//!    union/ours semantics). Resolution is recorded as a dedicated, labelled
//!    change.
//! 2. Operator: a genuine business arbitration. The gateway exposes the
//!    conflict and waits for a resolution change (see `operator`).
//! 3. Undecidable binaries/submodules/LFS: always operator. Pijul does not
//!    interpret binary content; the gateway does not either.

use crate::operator::ConflictReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Triage {
    /// Nothing left: the changes commuted cleanly.
    Clean,
    /// Resolved by rule; the label identifies which rule, for the audit.
    AutoResolved { rule: Rule },
    /// Needs an operator decision.
    Operator { report: ConflictReport },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Both sides produced identical content.
    SameContent,
    /// Formatting-only difference, canonical form applied.
    CanonicalFormatting,
    /// .gitattributes union/ours semantics.
    DeclaredMergeSemantics,
}

/// Inspect the integration channel and triage every remaining conflict.
///
/// v0 TODO: after `pijul apply` of all PR changes, run `pijul conflict show`
/// and classify each conflicted file:
/// - textual conflict -> read both sides, apply deterministic rules;
/// - binary/conflicted submodule -> Operator, no exceptions.
pub fn triage_integration_channel(_workdir: &std::path::Path) -> std::io::Result<Vec<Triage>> {
    // TODO(p1)
    Ok(vec![Triage::Clean])
}
