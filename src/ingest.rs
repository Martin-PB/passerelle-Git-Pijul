//! Ingest: fetch the PRs' head branches from the Git hosting platform.
//!
//! v0 shells out to `git fetch` (the gateway only needs the branch tips and
//! their short histories, not the whole repository). The GitHub API glue is
//! a TODO: today the caller passes PR head refs explicitly.

use crate::PullRequest;

/// Fetch the given PRs from `origin` into a local bare-ish mirror.
///
/// Shallow by default: the gateway works with branches, not with full
/// history, and the translation stage only walks short commit chains.
pub fn fetch_prs(remote: &str, prs: &[PullRequest], dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    let mut args: Vec<String> = vec!["fetch".into(), remote.to_string()];
    for pr in prs {
        args.push(format!("{}:refs/remotes/pr/{}", pr.head_ref, pr.number));
    }
    let status = std::process::Command::new("git").args(&args).current_dir(dest).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "git fetch failed",
        ))
    }
}
