//! Ingest: fetch generic Git refs into an isolated bare mirror.
//!
//! The core is hosting-agnostic: callers translate hosting concepts (PRs,
//! merge requests, etc.) into ordinary Git refs. We deliberately fetch full
//! reachable history because `pijul git` imports the DAG reachable from each
//! checked-out HEAD; shallow boundaries are not assumed to be safe.
//!
//! Each fetched head is pinned under `refs/remotes/pr/<number>`. The name is
//! only a local convention, not a GitHub-specific concept.

use crate::PullRequest;
use std::path::Path;
use std::process::Command;

fn git<I, S>(repo: &Path, args: I) -> std::io::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git").args(args).current_dir(repo).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "git command failed in {}: {}",
                repo.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

/// Create or validate a bare mirror. Never run fetch in an arbitrary
/// directory: doing so previously failed because `dest` was not a Git repo.
fn ensure_bare_mirror(dest: &Path) -> std::io::Result<()> {
    if !dest.exists() {
        std::fs::create_dir_all(dest)?;
    }
    let is_repo = Command::new("git")
        .args(["rev-parse", "--is-bare-repository"])
        .current_dir(dest)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false);

    if is_repo {
        return Ok(());
    }

    // Permit an existing empty directory, but refuse to repurpose a directory
    // containing user data or a non-bare repository.
    if std::fs::read_dir(dest)?.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{} exists but is not an empty directory or bare Git repository",
                dest.display()
            ),
        ));
    }
    git(dest, ["init", "--bare"])?;
    Ok(())
}

/// Fetch the supplied PR-head refs into a local bare mirror.
///
/// The remote is any Git remote URL or local repository path. Each ref is
/// force-updated intentionally, because hosting refs can move after a
/// synchronize/force-push event. If `head_sha` is supplied, the fetched ref
/// must resolve to that exact object; this prevents translating a moving ref
/// different from the event snapshot the caller intended to process.
pub fn fetch_prs(remote: &str, prs: &[PullRequest], dest: &Path) -> std::io::Result<()> {
    if remote.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "remote must not be empty",
        ));
    }

    let mut seen = std::collections::BTreeSet::new();
    for pr in prs {
        if !seen.insert(pr.number) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("duplicate PR/branch number {}", pr.number),
            ));
        }
        if pr.head_ref.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("head ref for {} must not be empty", pr.number),
            ));
        }
    }

    ensure_bare_mirror(dest)?;
    for pr in prs {
        let destination = format!("+{}:refs/remotes/pr/{}", pr.head_ref, pr.number);
        git(dest, ["fetch", "--no-tags", remote, &destination])?;

        if !pr.head_sha.trim().is_empty() {
            let actual = git(
                dest,
                ["rev-parse", &format!("refs/remotes/pr/{}^{{commit}}", pr.number)],
            )?;
            if actual != pr.head_sha {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "head ref {} moved: expected {}, fetched {}",
                        pr.head_ref, pr.head_sha, actual
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Fetch arbitrary named refs into the mirror. Hosting adapters should use
/// this for the target/base ref as well as incoming branch heads.
pub fn fetch_refs(remote: &str, refs: &[(String, String)], dest: &Path) -> std::io::Result<()> {
    if remote.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "remote must not be empty",
        ));
    }
    ensure_bare_mirror(dest)?;
    for (source, destination) in refs {
        if source.trim().is_empty()
            || destination.trim().is_empty()
            || !destination.starts_with("refs/")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid ref mapping {source:?} -> {destination:?}"),
            ));
        }
        let refspec = format!("+{source}:{destination}");
        git(dest, ["fetch", "--no-tags", remote, &refspec])?;
    }
    Ok(())
}
