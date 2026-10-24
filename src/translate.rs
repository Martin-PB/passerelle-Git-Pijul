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
use std::path::{Path, PathBuf};
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
        &["log", "--hash-only", "--channel", channel, "--repository", &pijul_repo.display().to_string()],
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
    let introduced: Vec<_> = after.difference(&before).cloned().collect();
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

/// Translate one PR into a Pijul channel `pr-<n>`.
///
/// The high-level orchestration still needs the mirror path and must create
/// the named channel from the imported base before exposing it to operators.
pub fn translate_pr(
    workdir: &Path,
    base: &Base,
    pr: &PullRequest,
    ledger: &mut Ledger,
) -> std::io::Result<()> {
    let _ = (workdir, base);
    if pr.head_sha.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("PR {} has no pinned head SHA", pr.number),
        ));
    }
    if let Some(existing) = ledger.get(&pr.head_sha) {
        if existing.pijul_change.starts_with("TODO-import-") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("ledger contains a placeholder mapping for {}", pr.head_sha),
            ));
        }
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "translate_pr orchestration is not wired yet; call import_git_commit and change_for_git_commit with the fetched mirror",
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
