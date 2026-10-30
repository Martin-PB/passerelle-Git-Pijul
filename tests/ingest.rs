//! Integration tests for the real Git ingest stage. These tests use only
//! repositories generated in temporary directories; no network or third-party
//! source repository is involved.

use passerelle_git_pijul::{ingest, PullRequest};
use std::path::Path;
use std::process::Command;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be installed for ingest integration tests");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(dir: &Path, message: &str) -> String {
    run_git(
        dir,
        &[
            "-c",
            "user.name=Corpus Author",
            "-c",
            "user.email=corpus@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            message,
        ],
    );
    run_git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn fetch_prs_initializes_bare_mirror_and_pins_ref() {
    let source = tempfile::tempdir().unwrap();
    run_git(source.path(), &["init", "-b", "main"]);
    let base_sha = commit(source.path(), "synthetic base");
    run_git(source.path(), &["switch", "-c", "feature-a"]);
    let head_sha = commit(source.path(), "synthetic feature");

    let mirror = source.path().join("mirror");
    let prs = [PullRequest {
        number: 7,
        title: "synthetic feature".into(),
        author: "corpus".into(),
        head_sha: head_sha.clone(),
        head_ref: "refs/heads/feature-a".into(),
    }];

    ingest::fetch_prs(source.path().to_str().unwrap(), &prs, &mirror).unwrap();

    assert_eq!(
        run_git(&mirror, &["rev-parse", "--is-bare-repository"]),
        "true"
    );
    assert_eq!(
        run_git(&mirror, &["rev-parse", "refs/remotes/pr/7"]),
        head_sha
    );
    assert_eq!(
        run_git(&mirror, &["rev-parse", "refs/remotes/pr/7^"]),
        base_sha
    );
}

#[test]
fn fetch_prs_rejects_a_ref_that_moved_since_the_event() {
    let source = tempfile::tempdir().unwrap();
    run_git(source.path(), &["init", "-b", "main"]);
    commit(source.path(), "base");
    run_git(source.path(), &["switch", "-c", "feature-a"]);
    let actual_sha = commit(source.path(), "current feature");

    let mirror = source.path().join("mirror");
    let prs = [PullRequest {
        number: 9,
        title: "stale event".into(),
        author: "corpus".into(),
        head_sha: "0000000000000000000000000000000000000000".into(),
        head_ref: "refs/heads/feature-a".into(),
    }];

    let err = ingest::fetch_prs(source.path().to_str().unwrap(), &prs, &mirror)
        .expect_err("a stale expected SHA must fail closed");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains(&actual_sha));
}

#[test]
fn fetch_prs_refuses_to_repurpose_non_git_data() {
    let source = tempfile::tempdir().unwrap();
    run_git(source.path(), &["init", "-b", "main"]);
    let dest = source.path().join("not-a-repository");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("keep.txt"), "must remain untouched").unwrap();

    let err = ingest::fetch_prs(source.path().to_str().unwrap(), &[], &dest)
        .expect_err("non-empty non-repository destination must be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        std::fs::read_to_string(dest.join("keep.txt")).unwrap(),
        "must remain untouched"
    );
}

#[test]
fn fetch_refs_accepts_hosting_agnostic_ref_mappings() {
    let source = tempfile::tempdir().unwrap();
    run_git(source.path(), &["init", "-b", "main"]);
    let main_sha = commit(source.path(), "synthetic main");

    let mirror = source.path().join("generic-mirror");
    let refs = [(
        "refs/heads/main".to_owned(),
        "refs/remotes/base/main".to_owned(),
    )];
    ingest::fetch_refs(source.path().to_str().unwrap(), &refs, &mirror).unwrap();

    assert_eq!(
        run_git(&mirror, &["rev-parse", "refs/remotes/base/main"]),
        main_sha
    );
}

#[test]
fn fetch_refs_rejects_non_ref_destinations() {
    let source = tempfile::tempdir().unwrap();
    run_git(source.path(), &["init", "-b", "main"]);
    commit(source.path(), "synthetic main");

    let mirror = source.path().join("generic-mirror");
    let refs = [("refs/heads/main".to_owned(), "../outside".to_owned())];
    let err = ingest::fetch_refs(source.path().to_str().unwrap(), &refs, &mirror)
        .expect_err("destination must be a Git ref");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}
