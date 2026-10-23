//! Translation: Git commits/branches <-> Pijul changes/channels.
//!
//! Dictionary of concepts (see README):
//!   Git commit      -> Pijul change (author, date and message travel in the
//!                      change metadata; the ledger keeps SHA <-> hash)
//!   Git branch      -> Pijul channel (fork = O(1), no working copy dup)
//!   Git merge commit-> set of applied changes (no equivalent, by design)
//!   Git rebase      -> nothing (commutation replaces it)
//!   Git conflict    -> Pijul conflict state (first-class, resolvable by a
//!                      dedicated change)
//!
//! v0 uses `pijul git` (history replay import) on short PR branches. The
//! export direction is the hard part and is stubbed here: it must linearise
//! the integration channel's changes in dependency order (or re-create a
//! merge commit per PR in MergeTopology mode) and preserve authors/dates.

use crate::ledger::{Direction, Entry, Ledger};
use crate::{Base, ExportMode, PullRequest};
use std::path::Path;

/// Translate one PR into a Pijul channel `pr-<n>` forked from `base`.
pub fn translate_pr(
    workdir: &Path,
    base: &Base,
    pr: &PullRequest,
    ledger: &mut Ledger,
) -> std::io::Result<()> {
    // TODO(p0): run `pijul channel new pr-<n>` from the base channel, then
    // `pijul git` import of the PR head into it. Requires the imported Git
    // objects from the ingest stage to be reachable from the workdir.
    let _ = (workdir, base);
    let ok = ledger.record(Entry {
        git_sha: pr.head_sha.clone(),
        pijul_change: format!("TODO-import-{}", pr.head_sha), // replaced by real change hashes at p0
        pr: Some(pr.number),
        direction: Direction::GitToPijul,
        recorded_at: now_iso(),
    });
    if !ok {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("SHA {} moved since last ingest (force-push?)", pr.head_sha),
        ));
    }
    Ok(())
}

/// Export the integration channel back to a Git branch.
pub fn export_integration(
    workdir: &Path,
    ledger: &mut Ledger,
    mode: ExportMode,
    base: &Base,
    prs: &[PullRequest],
) -> std::io::Result<String> {
    // TODO(p0):
    // - Linear: topologically sort the channel's changes, `git commit-tree`
    //   one commit per change (authors, dates, messages preserved), chained.
    // - MergeTopology: one merge commit per PR group, parents = base tip and
    //   the linearised PR head, so GitHub keeps showing the PR merges.
    // Every produced SHA is recorded in the ledger with direction
    // PijulToGit, closing the round-trip.
    let _ = (workdir, ledger, mode, base, prs);
    Ok(String::from("TODO-exported-branch"))
}

fn now_iso() -> String {
    // No chrono dependency in v0; std fallback for the ledger timestamp.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("unix:{}", d.as_secs()))
        .unwrap_or_else(|_| "unix:0".into())
}
