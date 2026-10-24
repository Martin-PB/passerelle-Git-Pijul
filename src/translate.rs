//! Translation: Git commits/branches <-> Pijul changes/channels.
//!
//! The P0 import uses upstream `pijul git` rather than reimplementing Git's
//! commit replay. The helper below imports a pinned Git commit and discovers
//! its Pijul change by comparing the imported channel with its first parent.
//! It deliberately fails closed if the CLI's observed result differs from the
//! design assumption of exactly one change per non-root Git commit.

use crate::ledger::{Direction, Entry, Ledger};
use crate::{Base, ExportMode, PullRequest};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn command_output(program: &str, args: &[String], cwd: Option<&Path>) -> std::io::Result<String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(path) = cwd {
        command.current_dir(path);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "`{program} {}` failed ({}): {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git(repo: &Path, args: &[&str]) -> std::io::Result<String> {
    command_output(
        "git",
        &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        Some(repo),
    )
}

fn pijul(repo: &Path, args: &[&str]) -> std::io::Result<String> {
    command_output(
        "pijul",
        &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        Some(repo),
    )
}

/// Return the set of change hashes visible on a Pijul channel.
pub fn changes_on_channel(pijul_repo: &Path, channel: &str) -> std::io::Result<BTreeSet<String>> {
    let output = pijul(
        pijul_repo,
        &["log", "--hash-only", "--channel", channel],
    )?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Find the unique change introduced by a Git commit, based on the imported
/// Pijul channels named by Git OIDs. Root commits have no parent and are
/// intentionally rejected here: their mapping must be established as the
/// import base, not treated as a PR delta.
pub fn change_for_git_commit(
    pijul_repo: &Path,
    git_sha: &str,
    parent_sha: &str,
) -> std::io::Result<String> {
    if git_sha.trim().is_empty() || parent_sha.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "git and parent SHAs must be non-empty",
        ));
    }
    let after = changes_on_channel(pijul_repo, git_sha)?;
    let before = changes_on_channel(pijul_repo, parent_sha)?;
    unique_new_change(&before, &after, git_sha)
}

/// Select the one new change between two imported commit states. Kept pure
/// so the cardinality invariant can be tested without invoking external CLIs.
pub fn unique_new_change(
    before: &BTreeSet<String>,
    after: &BTreeSet<String>,
    git_sha: &str,
) -> std::io::Result<String> {
    let introduced: Vec<_> = after.difference(before).cloned().collect();
    if introduced.len() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "expected exactly one Pijul change for Git commit {git_sha}, found {}",
                introduced.len()
            ),
        ));
    }
    Ok(introduced[0].clone())
}

/// Import the history reachable from a pinned Git commit using upstream
/// `pijul git`. The Git worktree is isolated per SHA so concurrent imports
/// never race on HEAD or dirty files. The imported Pijul repository is shared
/// intentionally so the upstream OID->state cache can make repeat imports
/// incremental.
///
/// This is a lower-level primitive; callers must fetch the base and its full
/// history first. It does not itself create the operator-facing pr-N channel.
pub fn import_git_commit(
    git_mirror: &Path,
    pijul_repo: &Path,
    git_sha: &str,
    scratch: &Path,
) -> std::io::Result<()> {
    if git_sha.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "git SHA must not be empty",
        ));
    }
    let resolved = git(git_mirror, &["rev-parse", &format!("{git_sha}^{{commit}}")])?;
    if resolved != git_sha {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("requested SHA {git_sha} resolved to {resolved}"),
        ));
    }

    let git_mirror = git_mirror.canonicalize()?;
    std::fs::create_dir_all(scratch)?;
    if let Some(parent) = pijul_repo.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let worktree = scratch.join(format!(
        "git-import-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        command_output(
            "git",
            &[
                "clone".into(),
                "--no-checkout".into(),
                "--no-hardlinks".into(),
                git_mirror.display().to_string(),
                worktree.display().to_string(),
            ],
            None,
        )?;
        // `git clone` normally copies refs/heads and tags only. PR/base refs
        // live under refs/remotes/* in our bare mirror, so fetch them explicitly
        // before checking out the pinned SHA.
        command_output(
            "git",
            &[
                "-C".into(),
                worktree.display().to_string(),
                "fetch".into(),
                "--no-tags".into(),
                git_mirror.display().to_string(),
                "+refs/remotes/*:refs/remotes/*".into(),
            ],
            None,
        )?;
        git(&worktree, &["checkout", "--detach", git_sha])?;
        let status = command_output(
            "pijul",
            &[
                "git".into(),
                worktree.display().to_string(),
                pijul_repo.display().to_string(),
            ],
            None,
        );
        status.map(|_| ())
    })();
    let cleanup = std::fs::remove_dir_all(&worktree);
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// List channels using the stable human-readable CLI output.
fn list_channels(pijul_repo: &Path) -> std::io::Result<BTreeSet<String>> {
    let output = pijul(pijul_repo, &["channel"])?;
    Ok(output
        .lines()
        .map(str::trim)
        .map(|line| line.strip_prefix("* ").unwrap_or(line))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn ensure_channel_from(
    pijul_repo: &Path,
    source: &str,
    target: &str,
) -> std::io::Result<()> {
    let channels = list_channels(pijul_repo)?;
    if channels.contains(target) {
        return Ok(());
    }
    if !channels.contains(source) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("source Pijul channel {source:?} does not exist"),
        ));
    }
    pijul(pijul_repo, &["channel", "switch", source])?;
    pijul(pijul_repo, &["fork", target])?;
    Ok(())
}

fn pull_channel_into(
    pijul_repo: &Path,
    source: &str,
    target: &str,
) -> std::io::Result<()> {
    pijul(
        pijul_repo,
        &["pull", "--all", ".", "--from-channel", source, "--to-channel", target],
    )?;
    Ok(())
}

/// Import one PR, ledger every non-merge commit since base, and materialize
/// operator-facing `pr-N` and `integration` channels. The target channels
/// are named, stable entry points; commit-OID channels remain implementation
/// details created by upstream `pijul git`.
///
/// A force-push that is not a fast-forward is rejected when prior ledger
/// entries for this PR exist. Automatically replacing its channel could
/// discard operator resolution work, so recovery must be explicit.
pub fn translate_pr_from_mirror(
    git_mirror: &Path,
    pijul_repo: &Path,
    scratch: &Path,
    base: &Base,
    pr: &PullRequest,
    ledger: &mut Ledger,
) -> std::io::Result<()> {
    if base.sha.trim().is_empty() || pr.head_sha.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "base and PR head must be pinned to full Git SHAs",
        ));
    }

    let base_is_ancestor = Command::new("git")
        .args(["merge-base", "--is-ancestor", &base.sha, &pr.head_sha])
        .current_dir(git_mirror)
        .status()?;
    if !base_is_ancestor.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "base {} is not an ancestor of PR {} head {}",
                base.sha, pr.number, pr.head_sha
            ),
        ));
    }

    let previous_heads: Vec<String> = ledger
        .by_sha
        .values()
        .filter(|entry| entry.pr == Some(pr.number) && entry.direction == Direction::GitToPijul)
        .map(|entry| entry.git_sha.clone())
        .collect();
    for previous in previous_heads {
        if previous == pr.head_sha {
            continue;
        }
        let ancestor = Command::new("git")
            .args(["merge-base", "--is-ancestor", &previous, &pr.head_sha])
            .current_dir(git_mirror)
            .status()?;
        if !ancestor.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "PR {} head {} is not a fast-forward of previously imported {}; refusing to overwrite channel pr-{}",
                    pr.number, pr.head_sha, previous, pr.number
                ),
            ));
        }
    }

    import_git_commit(git_mirror, pijul_repo, &base.sha, scratch)?;
    import_git_commit(git_mirror, pijul_repo, &pr.head_sha, scratch)?;

    let commits = git(
        git_mirror,
        &["rev-list", "--reverse", &format!("{}..{}", base.sha, pr.head_sha)],
    )?;
    for sha in commits.lines().map(str::trim).filter(|sha| !sha.is_empty()) {
        let parents = git(git_mirror, &["rev-list", "--parents", "-n", "1", sha])?;
        let parts: Vec<_> = parents.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "P0 importer currently requires non-merge commits; {sha} has {} parent(s)",
                    parts.len().saturating_sub(1)
                ),
            ));
        }
        let change = change_for_git_commit(pijul_repo, sha, parts[1])?;
        if !ledger.record(Entry {
            git_sha: sha.to_owned(),
            pijul_change: change,
            pr: Some(pr.number),
            direction: Direction::GitToPijul,
            recorded_at: now_iso(),
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Git SHA {sha} is already mapped to a different Pijul change"),
            ));
        }
    }

    ensure_channel_from(pijul_repo, &base.sha, &format!("pr-{}", pr.number))?;
    pull_channel_into(pijul_repo, &pr.head_sha, &format!("pr-{}", pr.number))?;
    ensure_channel_from(pijul_repo, &base.sha, "integration")?;
    pull_channel_into(pijul_repo, &format!("pr-{}", pr.number), "integration")?;
    let ledger_root = pijul_repo.parent().unwrap_or(pijul_repo);
    ledger.save(&crate::ledger::ledger_path(ledger_root))?;
    Ok(())
}

/// Translate one PR into a Pijul channel `pr-<n>`.
///
/// The high-level orchestration still needs the mirror path and must create
/// the named channel from the imported base before exposing it to operators.
pub fn translate_pr(
    _workdir: &Path,
    _base: &Base,
    pr: &PullRequest,
    _ledger: &mut Ledger,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "translate_pr requires the pinned Git mirror; use translate_pr_from_mirror for PR {}",
            pr.number
        ),
    ))
}

/// Export the integration channel back to a Git branch.
pub fn export_integration(
    workdir: &Path,
    ledger: &mut Ledger,
    mode: ExportMode,
    base: &Base,
    prs: &[PullRequest],
) -> std::io::Result<String> {
    let _ = (workdir, ledger, mode, base, prs);
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Pijul-to-Git export is not implemented; refusing to return a placeholder branch name",
    ))
}

#[allow(dead_code)]
fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("unix:{}", d.as_secs()))
        .unwrap_or_else(|_| "unix:0".into())
}
