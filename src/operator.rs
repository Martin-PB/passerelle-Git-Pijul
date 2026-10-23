//! Operator boundary: the gateway's ONLY interface towards humans (and
//! whatever assists them).
//!
//! The contract is minimal and closed:
//!   the gateway EXPOSES   -> ConflictReport (a Pijul conflict state + the
//!                            context of the conflicting PRs)
//!   the gateway RECEIVES  -> ResolutionChange (a change hash to apply on
//!                            the integration channel)
//!
//! Whether the operator works alone or with an LLM (e.g. through
//! mcp-server-pijul pointed at the gateway's working repository) changes
//! NOTHING here: both produce the same ResolutionChange through the same
//! queue. The gateway cannot tell them apart, and must not be able to.

use serde::{Deserialize, Serialize};

/// What the gateway shows for one unresolved conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReport {
    /// File path in conflict.
    pub path: String,
    /// The Pijul conflict state of the file (the gateway's own rendering).
    pub conflict_state: String,
    /// The PRs whose changes collide on this path.
    pub prs: Vec<PrContext>,
}

/// Minimal PR context shown alongside a conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrContext {
    pub number: u64,
    pub title: String,
    pub author: String,
}

/// What the operator sends back: a recorded change that resolves the
/// conflict. The gateway applies it and moves on. It never inspects how
/// the change was produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionChange {
    /// Hash (or unambiguous prefix) of the resolving change.
    pub change_hash: String,
    /// Free-form justification, archived in the ledger for audit.
    pub note: Option<String>,
}

/// The operator queue: exposed reports waiting for resolutions.
/// v0 is an in-process Vec; p2 turns it into the persistent queue behind the
/// CLI (and later web) console.
#[derive(Debug, Default)]
pub struct OperatorQueue {
    pub pending: Vec<ConflictReport>,
    pub received: Vec<ResolutionChange>,
}

impl OperatorQueue {
    pub fn submit(&mut self, resolution: ResolutionChange) {
        self.received.push(resolution);
        if !self.pending.is_empty() {
            self.pending.remove(0);
        }
    }
}
