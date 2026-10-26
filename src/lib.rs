//! Passerelle Git <-> Pijul.
//!
//! The gateway turns a Git merge problem (several conflicting PRs) into a
//! Pijul resolution, then back into a Git branch. Scope boundary:
//!
//! - The gateway EXPOSES conflicts (as Pijul conflict states plus PR context)
//!   and RECEIVES resolution changes. Nothing else crosses the operator
//!   boundary.
//! - How a resolution change is produced (an operator alone, or an operator
//!   assisted by an LLM through the mcp-server-pijul tooling) is strictly
//!   OUT OF SCOPE: both are the same operation for the gateway, and are
//!   indistinguishable in the pipeline, the ledger and the audit trail.

pub mod export;
pub mod ingest;
pub mod import_intent;
pub mod ledger;
pub mod operator;
pub mod translate;
pub mod triage;

/// A pull request as seen by the ingest stage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// Git SHA of the PR head.
    pub head_sha: String,
    /// Name of the remote ref, e.g. "refs/pull/42/head".
    pub head_ref: String,
}

/// The branch the PRs are meant to land on, e.g. "main".
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Base {
    pub branch: String,
    pub sha: String,
}

/// Instructions the operator gives the gateway for the final Git merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportMode {
    /// One commit per Pijul change, in dependency order (stacked history).
    Linear,
    /// One merge commit per PR, preserving PR topology on GitHub.
    MergeTopology,
}

/// A merge job, end to end.
#[derive(Debug)]
pub struct Job {
    pub base: Base,
    pub prs: Vec<PullRequest>,
    pub export_mode: ExportMode,
}
