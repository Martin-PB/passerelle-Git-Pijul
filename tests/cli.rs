use passerelle_git_pijul::workspace_lock::WorkspaceLock;
use std::process::Command;
use tempfile::tempdir;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_passerelle-git-pijul"))
}

#[test]
fn operator_submit_without_path_is_rejected_before_pijul_mutation() {
    let workdir = tempdir().expect("temporary gateway workspace");
    let output = cli()
        .args([
            "operator",
            "submit",
            "--workdir",
            workdir.path().to_str().unwrap(),
            "--change",
            "ABC123",
            "--note",
            "test",
        ])
        .output()
        .expect("run CLI");

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --path"));
    assert!(!workdir.path().join("pijul").exists());
}

#[test]
fn operator_queue_refuses_to_run_when_workspace_lock_is_held() {
    let workdir = tempdir().expect("temporary gateway workspace");
    let lock = WorkspaceLock::for_workspace(workdir.path()).expect("acquire workspace lock");
    let output = cli()
        .args([
            "operator",
            "queue",
            "--workdir",
            workdir.path().to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("cannot acquire gateway workspace lock"));
    assert!(!workdir.path().join("pijul").exists());
    drop(lock);
}
