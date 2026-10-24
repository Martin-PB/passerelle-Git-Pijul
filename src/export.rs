//! Export planning and an optional, fail-closed adapter for the external
//! `pijul-export` fast-export executable.
//!
//! The executable is not bundled or linked: its upstream COPYING file did not
//! expose a verifiable license during the initial audit. Linear export is
//! attempted only when an operator has separately installed that tool.
//! Merge-topology export remains unsupported.

use crate::ExportMode;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Human-readable plan of what will be pushed, per PR, for a given export
/// mode. Used by the CLI so the operator can confirm the shape before the
/// gateway pushes anything.
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

fn git(
    mirror: &Path,
    args: &[&str],
    worktree: Option<&Path>,
    index: Option<&Path>,
) -> std::io::Result<String> {
    let mirror = mirror.canonicalize()?;
    let mut command = Command::new("git");
    command.arg("--git-dir").arg(&mirror);
    if let Some(path) = worktree {
        command.arg("--work-tree").arg(path);
    }
    command.args(args);
    if let Some(path) = worktree {
        command.current_dir(path);
    }
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn ref_exists(mirror: &Path, reference: &str) -> std::io::Result<bool> {
    let output = Command::new("git")
        .args(["--git-dir"])
        .arg(mirror.canonicalize()?)
        .args(["show-ref", "--verify", "--quiet", reference])
        .output()?;
    Ok(output.status.success())
}

fn delete_ref(mirror: &Path, reference: &str) {
    if let Ok(mirror) = mirror.canonicalize() {
        let _ = Command::new("git")
            .args(["--git-dir"])
            .arg(mirror)
            .args(["update-ref", "-d", reference])
            .status();
    }
}

fn working_tree_id(mirror: &Path, pijul_repo: &Path, scratch: &Path) -> std::io::Result<String> {
    let mirror = mirror.canonicalize()?;
    let pijul_repo = pijul_repo.canonicalize()?;
    std::fs::create_dir_all(scratch)?;
    let index = scratch.join(format!("export-index-{}", std::process::id()));
    let result = (|| {
        git(&mirror, &["read-tree", "--empty"], Some(&pijul_repo), Some(&index))?;
        git(
            &mirror,
            &[
                "add",
                "-A",
                "--",
                ".",
                ":(exclude).pijul",
                ":(exclude).ignore",
            ],
            Some(&pijul_repo),
            Some(&index),
        )?;
        git(&mirror, &["write-tree"], Some(&pijul_repo), Some(&index))
    })();
    let _ = std::fs::remove_file(index);
    result
}

/// Export the integration channel to a local Git branch using an independently
/// installed `pijul-export` executable. It validates both base ancestry and
/// the final tree before publishing the ref. It never pushes to a remote.
///
/// This adapter deliberately supports only Linear mode. A pre-existing output
/// branch is reused only if its tree matches exactly; otherwise it is left
/// untouched and the operation fails closed.
pub fn export_linear_with_pijul_export(
    pijul_repo: &Path,
    git_mirror: &Path,
    base_sha: &str,
    scratch: &Path,
) -> std::io::Result<String> {
    if base_sha.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "base SHA must not be empty",
        ));
    }
    let pijul_repo = pijul_repo.canonicalize()?;
    let git_mirror = git_mirror.canonicalize()?;
    std::fs::create_dir_all(scratch)?;
    let scratch = scratch.canonicalize()?;

    // Switching channels refuses to discard unrecorded work by default.
    let switch = Command::new("pijul")
        .args(["channel", "switch", "integration"])
        .current_dir(&pijul_repo)
        .output()?;
    if !switch.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "cannot switch Pijul working copy to integration: {}",
                String::from_utf8_lossy(&switch.stderr).trim()
            ),
        ));
    }

    let output_ref = "refs/heads/gateway/integration";
    let expected_tree = working_tree_id(&git_mirror, &pijul_repo, &scratch)?;
    let staging_branch = format!("gateway/staging-{}", std::process::id());
    let staging_ref = format!("refs/heads/{staging_branch}");
    if ref_exists(&git_mirror, &staging_ref)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("temporary export ref {staging_ref} already exists"),
        ));
    }

    let exporter = Command::new("pijul-export")
        .args([
            "--repo",
            pijul_repo.to_str().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "non-UTF-8 Pijul path")
            })?,
            "--channel",
            "integration",
            "--branch",
            &staging_branch,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut exporter = exporter;
    let stream = exporter.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "pijul-export stdout was not piped")
    })?;
    let import = Command::new("git")
        .args(["--git-dir"])
        .arg(&git_mirror)
        .arg("fast-import")
        .stdin(Stdio::from(stream))
        .output()?;
    let export_status = exporter.wait()?;
    if !export_status.success() || !import.status.success() {
        delete_ref(&git_mirror, &staging_ref);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "fast-export/import failed (pijul-export: {export_status}; git fast-import: {}): {}",
                import.status,
                String::from_utf8_lossy(&import.stderr).trim()
            ),
        ));
    }

    let exported_sha = git(&git_mirror, &["rev-parse", &format!("{staging_ref}^{{commit}}")], None, None)?;
    let exported_tree = git(&git_mirror, &["rev-parse", &format!("{exported_sha}^{{tree}}")], None, None)?;
    if exported_tree != expected_tree {
        delete_ref(&git_mirror, &staging_ref);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "export tree mismatch: Pijul integration {expected_tree}, Git export {exported_tree}"
            ),
        ));
    }

    let ancestor = Command::new("git")
        .args(["--git-dir"])
        .arg(&git_mirror)
        .args(["merge-base", "--is-ancestor", base_sha, &exported_sha])
        .status()?;
    if !ancestor.success() {
        delete_ref(&git_mirror, &staging_ref);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("exported history is not a descendant of base {base_sha}"),
        ));
    }

    if ref_exists(&git_mirror, output_ref)? {
        let existing_sha = git(&git_mirror, &["rev-parse", &format!("{output_ref}^{{commit}}")], None, None)?;
        let existing_tree = git(&git_mirror, &["rev-parse", &format!("{existing_sha}^{{tree}}")], None, None)?;
        delete_ref(&git_mirror, &staging_ref);
        if existing_tree == expected_tree {
            return Ok(output_ref.to_owned());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("output ref {output_ref} exists with a different tree; it was not overwritten"),
        ));
    }

    let object_format = git(&git_mirror, &["rev-parse", "--show-object-format"], None, None)?;
    let zero = if object_format == "sha256" {
        "0000000000000000000000000000000000000000000000000000000000000000"
    } else {
        "0000000000000000000000000000000000000000"
    };
    let update = Command::new("git")
        .args(["--git-dir"])
        .arg(&git_mirror)
        .args(["update-ref", output_ref, &exported_sha, zero])
        .status()?;
    if !update.success() {
        delete_ref(&git_mirror, &staging_ref);
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("could not create output ref {output_ref}; it may have changed concurrently"),
        ));
    }
    delete_ref(&git_mirror, &staging_ref);
    Ok(output_ref.to_owned())
}
