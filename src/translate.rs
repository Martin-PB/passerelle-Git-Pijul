//! Translation: Git commits/branches <-> Pijul changes/channels.
//!
//! The P0 import uses upstream `pijul git` rather than reimplementing Git's
//! commit replay. The helper below imports a pinned Git commit and discovers
//! its Pijul change by comparing the imported channel with its first parent.
//! It deliberately fails closed if the CLI's observed result differs from the
//! design assumption of exactly one change per non-root Git commit.

use crate::import_intent::{ImportIntent, ImportIntentStore};
use crate::ledger::{Direction, Entry, Ledger};
use crate::{Base, ExportMode, PullRequest};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitStatus};
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
    // A bare mirror has HEAD/objects/refs at its root and no .git directory.
    // Setting it as current_dir is not enough: Git will not discover it as a
    // repository. Use --git-dir explicitly for bare mirrors, while retaining
    // normal -C/current-directory semantics for checked-out worktrees.
    let is_bare = repo.join("HEAD").is_file()
        && repo.join("objects").is_dir()
        && repo.join("refs").is_dir()
        && !repo.join(".git").exists();
    if is_bare {
        let git_dir = repo.canonicalize()?;
        let mut command_args = vec!["--git-dir".to_owned(), git_dir.display().to_string()];
        command_args.extend(args.iter().map(|s| (*s).to_owned()));
        command_output("git", &command_args, None)
    } else {
        command_output(
            "git",
            &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            Some(repo),
        )
    }
}

fn pijul(repo: &Path, args: &[&str]) -> std::io::Result<String> {
    command_output(
        "pijul",
        &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        Some(repo),
    )
}

/// Require an immutable, fully resolved commit OID at the bridge boundary.
/// Branch names and other revision expressions are rejected even if Git can
/// resolve them, so later ref movement cannot silently change the import input.
fn require_pinned_commit(git_mirror: &Path, oid: &str, role: &str) -> std::io::Result<()> {
    if oid.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{role} commit OID must not be empty"),
        ));
    }
    let resolved = git(
        git_mirror,
        &["rev-parse", &format!("{oid}^{{commit}}")],
    )?;
    if resolved != oid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{role} must be pinned to a full commit OID; requested {oid:?}, resolved {resolved:?}"
            ),
        ));
    }
    Ok(())
}

fn git_status(repo: &Path, args: &[&str]) -> std::io::Result<ExitStatus> {
    let is_bare = repo.join("HEAD").is_file()
        && repo.join("objects").is_dir()
        && repo.join("refs").is_dir()
        && !repo.join(".git").exists();
    let mut command = Command::new("git");
    if is_bare {
        command.arg("--git-dir").arg(repo.canonicalize()?);
    } else {
        command.current_dir(repo);
    }
    command.args(args);
    command.status()
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

/// Stable operator-internal channel name for a pinned Git base commit.
/// OID-named channels are temporary implementation details of `pijul git`;
/// the base used to initialize later PR channels must outlive those OID channels.
fn stable_base_channel_name(base_sha: &str) -> String {
    format!("base-{base_sha}")
}

/// Stable snapshots preserve imported state when pijul git removes the
/// corresponding OID-named channel while importing a child commit.
fn stable_commit_channel_name(git_sha: &str) -> String {
    format!("git-{git_sha}")
}

/// The first commit in a PR may have a parent OID channel that a prior import
/// removed. Use the retained stable base channel for that one snapshot.
fn parent_state_channel<'a>(
    parent_sha: &'a str,
    base_sha: &str,
    stable_base_channel: &'a str,
) -> &'a str {
    if parent_sha == base_sha {
        stable_base_channel
    } else {
        parent_sha
    }
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
    require_pinned_commit(git_mirror, &base.sha, "base")?;
    require_pinned_commit(git_mirror, &pr.head_sha, "PR head")?;

    let base_is_ancestor = git_status(
        git_mirror,
        &["merge-base", "--is-ancestor", &base.sha, &pr.head_sha],
    )?;
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
        let ancestor = git_status(
            git_mirror,
            &["merge-base", "--is-ancestor", &previous, &pr.head_sha],
        )?;
        if !ancestor.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "PR {} head {} is not a fast-forward of {}; refusing to overwrite pr-{}",
                    pr.number, pr.head_sha, previous, pr.number
                ),
            ));
        }
    }

    let ledger_root = pijul_repo.parent().unwrap_or(pijul_repo);
    let ledger_file = crate::ledger::ledger_path(ledger_root);
    let intent_store = ImportIntentStore::new(ledger_root);

    // Quarantine the entire workspace if any earlier import is unresolved.
    // A pending intent for a different SHA can still represent a mutation to
    // the shared Pijul repository, so checking only the current loop SHA is
    // insufficient. The sole automatic recovery is an exact intent whose
    // mapping is already durable in the ledger.
    let pending_intents = intent_store.pending_intents()?;
    if !pending_intents.is_empty() {
        let durable_ledger = Ledger::load(&ledger_file)?;
        for intent in pending_intents {
            let Some(change) = intent.pijul_change.as_deref() else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    format!(
                        "workspace has unresolved import intent for {}; no durable change mapping exists, recover explicitly before importing any SHA",
                        intent.git_sha
                    ),
                ));
            };
            let Some(durable_entry) = durable_ledger.get(&intent.git_sha) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    format!(
                        "workspace has unresolved import intent for {}; ledger mapping is absent, recover explicitly before importing any SHA",
                        intent.git_sha
                    ),
                ));
            };
            if durable_entry.git_sha != intent.git_sha
                || durable_entry.direction != Direction::GitToPijul
                || durable_entry.pr != Some(intent.pr)
                || durable_entry.pijul_change != change
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "durable ledger entry does not exactly match pending import intent for {}; workspace quarantined",
                        intent.git_sha
                    ),
                ));
            }

            let parent_line = git(
                git_mirror,
                &["rev-list", "--parents", "-n", "1", &intent.git_sha],
            )?;
            let parts: Vec<_> = parent_line.split_whitespace().collect();
            if parts.len() != 2 || intent.parent_sha != parts[1] {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "pending import intent for {} does not match its current Git parent",
                        intent.git_sha
                    ),
                ));
            }
            let resolved_base = git(
                git_mirror,
                &["rev-parse", &format!("{}^{{commit}}", intent.base_sha)],
            )?;
            if resolved_base != intent.base_sha {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "pending import intent for {} references an unavailable or non-canonical base",
                        intent.git_sha
                    ),
                ));
            }

            let Some(memory_entry) = ledger.get(&intent.git_sha) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "durable mapping for pending intent {} is absent from the in-memory ledger; reload before retrying",
                        intent.git_sha
                    ),
                ));
            };
            if memory_entry.git_sha != durable_entry.git_sha
                || memory_entry.direction != durable_entry.direction
                || memory_entry.pr != durable_entry.pr
                || memory_entry.pijul_change != durable_entry.pijul_change
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "in-memory and durable ledger mappings differ for pending intent {}; workspace quarantined",
                        intent.git_sha
                    ),
                ));
            }
            intent_store.reconcile_committed(
                &intent,
                &durable_entry.git_sha,
                durable_entry.pr,
                &durable_entry.pijul_change,
            )?;
        }
    }

    // Pijul beta.24 may drop intermediate OID channels as soon as their
    // children are imported. Keep a stable channel for the pinned base and
    // snapshot each parent before importing its child. A later PR must not
    // depend on the base OID channel still existing.
    let pr_channel = format!("pr-{}", pr.number);
    let base_channel = stable_base_channel_name(&base.sha);
    let has_previous_import = ledger.by_sha.values().any(|entry| {
        entry.pr == Some(pr.number) && entry.direction == Direction::GitToPijul
    });
    let has_any_import = ledger
        .by_sha
        .values()
        .any(|entry| entry.direction == Direction::GitToPijul);

    // On a fresh workspace the Pijul repository does not exist yet, so
    // querying its channels would fail before the base import can initialize it.
    // Bootstrap from the pinned base first; never silently recreate a repository
    // when the ledger says imports already exist.
    if !pijul_repo.exists() {
        if has_any_import {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Pijul repository is missing although the ledger contains prior imports; refusing unsafe reinitialization",
            ));
        }
        import_git_commit(git_mirror, pijul_repo, &base.sha, scratch)?;
    }

    let mut channels = list_channels(pijul_repo)?;
    if channels
        .iter()
        .any(|channel| channel.starts_with("base-") && channel != &base_channel)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "a different stable base channel already exists; refusing to combine base {} with an integration history built from another base",
                base.sha
            ),
        ));
    }
    if !channels.contains(&base_channel) {
        if !channels.contains(&base.sha) {
            if has_any_import {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "stable base channel {base_channel:?} is missing and the OID channel {} is unavailable after prior imports; refusing unsafe reinitialization",
                        base.sha
                    ),
                ));
            }
            import_git_commit(git_mirror, pijul_repo, &base.sha, scratch)?;
        }
        ensure_channel_from(pijul_repo, &base.sha, &base_channel)?;
        channels = list_channels(pijul_repo)?;
    }

    if !has_previous_import {
        if channels.contains(&pr_channel) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "channel {pr_channel:?} exists without matching ledger entries; refusing to reuse possibly stale state"
                ),
            ));
        }
        // Validate an existing integration channel before creating any new PR
        // channel, so a rejected stale workdir is left unchanged.
        if channels.contains("integration") && !has_any_import {
            let base_changes = changes_on_channel(pijul_repo, &base_channel)?;
            let integration_changes = changes_on_channel(pijul_repo, "integration")?;
            if integration_changes != base_changes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "integration channel differs from the stable base but the ledger has no imports; refusing to reuse possibly stale state",
                ));
            }
        }
        ensure_channel_from(pijul_repo, &base_channel, &pr_channel)?;
        channels = list_channels(pijul_repo)?;
        if !channels.contains("integration") {
            ensure_channel_from(pijul_repo, &base_channel, "integration")?;
        }
    } else if !channels.contains(&pr_channel) || !channels.contains("integration") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "ledger has prior entries for PR {} but its named channels are missing; refusing unsafe reinitialization",
                pr.number
            ),
        ));
    }

    let commits = git(
        git_mirror,
        &["rev-list", "--reverse", &format!("{}..{}", base.sha, pr.head_sha)],
    )?;
    for sha in commits.lines().map(str::trim).filter(|sha| !sha.is_empty()) {
        // A rerun must not re-import old commits: beta.24's OID cache may
        // refer to channels it has already dropped. The retained PR tip is
        // the checkpoint; import only commits not yet in the ledger.
        if let Some(existing) = ledger.get(sha) {
            if existing.direction == Direction::GitToPijul && existing.pr == Some(pr.number) {
                if let Some(intent) = intent_store.load(sha)? {
                    if intent.base_sha != base.sha {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("pending import intent for {sha} names a different base"),
                        ));
                    }
                    let parent_line =
                        git(git_mirror, &["rev-list", "--parents", "-n", "1", sha])?;
                    let parts: Vec<_> = parent_line.split_whitespace().collect();
                    if parts.len() != 2 || intent.parent_sha != parts[1] {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("pending import intent for {sha} does not match its current Git parent"),
                        ));
                    }
                    let durable_ledger = Ledger::load(&ledger_file)?;
                    let Some(durable_entry) = durable_ledger.get(sha) else {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("pending import intent for {sha} has no durable ledger mapping"),
                        ));
                    };
                    if durable_entry.git_sha != existing.git_sha
                        || durable_entry.pijul_change != existing.pijul_change
                        || durable_entry.pr != existing.pr
                        || durable_entry.direction != existing.direction
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("durable ledger entry for {sha} differs from the in-memory mapping"),
                        ));
                    }
                    intent_store.reconcile_committed(
                        &intent,
                        &durable_entry.git_sha,
                        durable_entry.pr,
                        &durable_entry.pijul_change,
                    )?;
                }
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Git SHA {sha} already has a ledger entry outside PR {}'s import", pr.number),
            ));
        }

        intent_store.require_no_uncommitted_intent(sha)?;
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
        let parent_sha = parts[1];
        let mut intent = ImportIntent {
            git_sha: sha.to_owned(),
            parent_sha: parent_sha.to_owned(),
            base_sha: base.sha.clone(),
            pr: pr.number,
            pijul_change: None,
        };
        // Persist the intent before any mutating Pijul call, including restoring
        // a missing parent channel from its stable snapshot.
        intent_store.begin(&intent)?;
        let parent_channel = parent_state_channel(parent_sha, &base.sha, &base_channel);
        let parent_snapshot = if parent_sha == base.sha {
            base_channel.clone()
        } else {
            stable_commit_channel_name(parent_sha)
        };
        // pijul git resolves a commit's parent by its Git-OID channel name.
        // beta.24 may remove that channel after importing a child, so restore
        // it from the durable snapshot before every import.
        ensure_channel_from(pijul_repo, &parent_snapshot, parent_sha)?;
        let before = changes_on_channel(pijul_repo, parent_channel)?;
        import_git_commit(git_mirror, pijul_repo, sha, scratch)?;
        let after = changes_on_channel(pijul_repo, sha)?;
        let change = unique_new_change(&before, &after, sha)?;
        // Preserve this commit's state before a later child import can remove
        // its OID-named channel.
        ensure_channel_from(pijul_repo, sha, &stable_commit_channel_name(sha))?;
        intent.pijul_change = Some(change.clone());
        intent_store.mark_mapped(&intent)?;
        record_mapping_and_checkpoint(
            ledger,
            Entry {
                git_sha: sha.to_owned(),
                pijul_change: change,
                pr: Some(pr.number),
                direction: Direction::GitToPijul,
                recorded_at: now_timestamp(),
            },
            &ledger_file,
        )?;
        intent_store.complete(&intent)?;
    }

    pull_channel_into(pijul_repo, &pr.head_sha, &pr_channel)?;
    pull_channel_into(pijul_repo, &format!("pr-{}", pr.number), "integration")?;
    ledger.save(&ledger_file)?;
    Ok(())
}

/// Record a completed SHA-to-change observation and checkpoint it immediately.
/// This preserves earlier successful mappings if a later commit import fails.
fn record_mapping_and_checkpoint(
    ledger: &mut Ledger,
    entry: Entry,
    ledger_file: &Path,
) -> std::io::Result<()> {
    let git_sha = entry.git_sha.clone();
    if !ledger.record(entry) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("Git SHA {git_sha} is already mapped to a different Pijul change"),
        ));
    }
    ledger.save(ledger_file)
}

#[cfg(test)]
mod ledger_checkpoint_tests {
    use super::record_mapping_and_checkpoint;
    use crate::ledger::{ledger_path, Direction, Entry, Ledger};

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn entry(pijul_change: &str) -> Entry {
        Entry {
            git_sha: SHA.into(),
            pijul_change: pijul_change.into(),
            pr: Some(42),
            direction: Direction::GitToPijul,
            recorded_at: "test-time".into(),
        }
    }

    #[test]
    fn completed_mapping_is_persisted_immediately() {
        let dir = tempfile::tempdir().expect("temporary ledger directory");
        let path = ledger_path(dir.path());
        let mut ledger = Ledger::default();

        record_mapping_and_checkpoint(&mut ledger, entry("change-one"), &path)
            .expect("checkpoint mapping");

        let loaded = Ledger::load(&path).expect("load checkpoint");
        assert_eq!(loaded.get(SHA).unwrap().pijul_change, "change-one");
    }

    #[test]
    fn conflicting_mapping_does_not_replace_persisted_checkpoint() {
        let dir = tempfile::tempdir().expect("temporary ledger directory");
        let path = ledger_path(dir.path());
        let mut ledger = Ledger::default();
        record_mapping_and_checkpoint(&mut ledger, entry("change-one"), &path)
            .expect("checkpoint original mapping");

        let error = record_mapping_and_checkpoint(&mut ledger, entry("different-change"), &path)
            .expect_err("a conflicting mapping must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);

        let loaded = Ledger::load(&path).expect("load original checkpoint");
        assert_eq!(loaded.get(SHA).unwrap().pijul_change, "change-one");
    }
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
    git_mirror: &Path,
    ledger: &mut Ledger,
    mode: ExportMode,
    base: &Base,
    prs: &[PullRequest],
) -> std::io::Result<String> {
    let _ = (ledger, prs);
    match mode {
        ExportMode::Linear => crate::export::export_linear_with_pijul_export(
            &workdir.join("pijul"),
            git_mirror,
            &base.sha,
            &workdir.join("scratch"),
        ),
        ExportMode::MergeTopology => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "merge-topology export is not implemented; only validated linear export is available",
        )),
    }
}

fn now_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| format!("unix:{}", duration.as_secs()))
        .unwrap_or_else(|_| "unix:0".into())
}


#[cfg(test)]
mod stable_base_channel_tests {
    use super::{parent_state_channel, stable_base_channel_name};

    #[test]
    fn base_channel_name_is_stable_for_a_pinned_sha() {
        assert_eq!(
            stable_base_channel_name("0123456789abcdef"),
            "base-0123456789abcdef"
        );
    }

    #[test]
    fn first_commit_uses_stable_base_for_parent_snapshot() {
        let stable = "base-0123456789abcdef";
        assert_eq!(
            parent_state_channel("0123456789abcdef", "0123456789abcdef", stable),
            stable
        );
        assert_eq!(
            parent_state_channel("fedcba9876543210", "0123456789abcdef", stable),
            "fedcba9876543210"
        );
    }
}

#[cfg(test)]
mod pinned_commit_tests {
    use super::require_pinned_commit;
    use std::path::Path;
    use std::process::Command;

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git must be installed for pinned-commit tests");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn accepts_full_commit_oid_and_rejects_movable_branch_name() {
        let root = tempfile::tempdir().expect("temporary Git repository");
        run_git(root.path(), &["init", "-b", "main"]);
        run_git(root.path(), &["config", "user.name", "Synthetic Test"]);
        run_git(root.path(), &["config", "user.email", "synthetic@example.invalid"]);
        std::fs::write(root.path().join("file.txt"), "base\n").expect("write file");
        run_git(root.path(), &["add", "file.txt"]);
        run_git(root.path(), &["commit", "-m", "base"]);
        let sha = run_git(root.path(), &["rev-parse", "HEAD"]);

        require_pinned_commit(root.path(), &sha, "test base")
            .expect("full commit OID should be accepted");
        let error = require_pinned_commit(root.path(), "main", "test base")
            .expect_err("a movable branch name must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("must be pinned"));
    }
}

#[cfg(test)]
mod pending_intent_preflight_tests {
    use super::translate_pr_from_mirror;
    use crate::import_intent::{ImportIntent, ImportIntentStore};
    use crate::ledger::Ledger;
    use crate::{Base, PullRequest};
    use std::path::Path;
    use std::process::Command;

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git must be installed for pending-intent preflight tests");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn unrelated_unresolved_intent_blocks_before_pijul_repository_initialization() {
        let root = tempfile::tempdir().expect("temporary root");
        let work = root.path().join("git-work");
        let mirror = root.path().join("mirror.git");
        let workspace = root.path().join("workspace");
        let pijul_repo = workspace.join("pijul");
        let scratch = workspace.join("scratch");
        std::fs::create_dir_all(&work).expect("create Git worktree");
        std::fs::create_dir_all(&workspace).expect("create gateway workspace");
        run_git(root.path(), &["init", "--bare", mirror.to_str().unwrap()]);
        run_git(&work, &["init", "-b", "main"]);
        run_git(&work, &["config", "user.name", "Synthetic Test"]);
        run_git(&work, &["config", "user.email", "synthetic@example.invalid"]);
        std::fs::write(work.join("base.txt"), "base\n").expect("write base file");
        run_git(&work, &["add", "base.txt"]);
        run_git(&work, &["commit", "-m", "base"]);
        let base_sha = run_git(&work, &["rev-parse", "HEAD"]);

        run_git(&work, &["checkout", "-b", "other"]);
        std::fs::write(work.join("other.txt"), "other\n").expect("write unrelated file");
        run_git(&work, &["add", "other.txt"]);
        run_git(&work, &["commit", "-m", "unrelated commit"]);
        let unrelated_sha = run_git(&work, &["rev-parse", "HEAD"]);

        run_git(&work, &["checkout", "main"]);
        std::fs::write(work.join("head.txt"), "head\n").expect("write PR file");
        run_git(&work, &["add", "head.txt"]);
        run_git(&work, &["commit", "-m", "PR head"]);
        let head_sha = run_git(&work, &["rev-parse", "HEAD"]);
        run_git(
            &work,
            &["push", mirror.to_str().unwrap(), "main", "other"],
        );

        ImportIntentStore::new(&workspace)
            .begin(&ImportIntent {
                git_sha: unrelated_sha,
                parent_sha: base_sha.clone(),
                base_sha: base_sha.clone(),
                pr: 999,
                pijul_change: None,
            })
            .expect("persist unrelated pending intent");

        let error = translate_pr_from_mirror(
            &mirror,
            &pijul_repo,
            &scratch,
            &Base {
                branch: "main".into(),
                sha: base_sha,
            },
            &PullRequest {
                number: 42,
                title: "synthetic PR".into(),
                author: "test".into(),
                head_sha,
                head_ref: "refs/heads/main".into(),
            },
            &mut Ledger::default(),
        )
        .expect_err("an unrelated unresolved intent must quarantine the workspace");

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(error.to_string().contains("unresolved import intent"));
        assert!(
            !pijul_repo.exists(),
            "the pending-intent check must run before initializing Pijul"
        );
    }
}

#[cfg(test)]
mod bare_mirror_tests {
    use super::{git, git_status};
    use std::path::Path;
    use std::process::Command;

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git must be installed for bare-mirror tests");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn bare_mirror_supports_rev_list_and_ancestry_checks() {
        let root = tempfile::tempdir().expect("temporary root");
        let work = root.path().join("work");
        let mirror = root.path().join("mirror.git");
        std::fs::create_dir_all(&work).expect("create worktree");
        run_git(root.path(), &["init", "--bare", mirror.to_str().expect("UTF-8 mirror")]);
        run_git(&work, &["init"]);
        run_git(&work, &["config", "user.name", "Synthetic Test"]);
        run_git(&work, &["config", "user.email", "synthetic@example.invalid"]);
        std::fs::write(work.join("file.txt"), "base\n").expect("write fixture");
        run_git(&work, &["add", "file.txt"]);
        run_git(&work, &["commit", "-m", "base"]);
        let sha = run_git(&work, &["rev-parse", "HEAD"]);
        run_git(
            &work,
            &[
                "push",
                mirror.to_str().expect("UTF-8 mirror"),
                "HEAD:refs/heads/main",
            ],
        );

        assert_eq!(
            git(&mirror, &["rev-parse", "refs/heads/main^{commit}"])
                .expect("rev-parse in bare mirror"),
            sha
        );
        assert_eq!(
            git(&mirror, &["rev-list", "--reverse", "--all"])
                .expect("rev-list in bare mirror"),
            sha
        );
        assert!(
            git_status(&mirror, &["merge-base", "--is-ancestor", &sha, &sha])
                .expect("ancestry check in bare mirror")
                .success()
        );
    }
}
