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

#[cfg(unix)]
#[test]
fn operator_submit_retriages_after_applying_a_current_conflict() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("temporary test root");
    let workdir = root.path().join("workspace");
    let pijul_repo = workdir.join("pijul");
    let mock_bin = root.path().join("mock-bin");
    std::fs::create_dir_all(&pijul_repo).expect("create Pijul work directory");
    std::fs::create_dir_all(&mock_bin).expect("create mock bin directory");
    std::fs::write(pijul_repo.join("file.txt"), "conflict contents\n")
        .expect("write conflict fixture");

    let count_file = root.path().join("status-count");
    let mock = mock_bin.join("pijul");
    std::fs::write(
        &mock,
        r#"#!/bin/sh
if [ "$1" = "channel" ] && [ "$2" = "switch" ] && [ "$3" = "integration" ]; then
    exit 0
fi
if [ "$1" = "status" ]; then
    count=0
    if [ -f "$MOCK_PIJUL_COUNT" ]; then count=$(cat "$MOCK_PIJUL_COUNT"); fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$MOCK_PIJUL_COUNT"
    if [ "$count" -eq 1 ]; then
        printf 'On channel: integration\n\nUnresolved conflicts:\n    file.txt\n'
    else
        printf 'On channel: integration\n'
    fi
    exit 0
fi
if [ "$1" = "apply" ]; then exit 0; fi
echo "unexpected pijul command: $*" >&2
exit 91
"#,
    )
    .expect("write mock Pijul command");
    let mut permissions = std::fs::metadata(&mock).expect("stat mock").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&mock, permissions).expect("make mock executable");

    let mut search_paths = vec![mock_bin];
    search_paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let path = std::env::join_paths(search_paths).expect("compose PATH");
    let output = cli()
        .env("PATH", path)
        .env("MOCK_PIJUL_COUNT", &count_file)
        .args([
            "operator",
            "submit",
            "--workdir",
            workdir.to_str().unwrap(),
            "--change",
            "ABC123",
            "--path",
            "file.txt",
            "--note",
            "test resolution",
        ])
        .output()
        .expect("run CLI");

    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&count_file).unwrap().trim(), "2");
    let audit = std::fs::read_to_string(workdir.join("gateway-audit.jsonl"))
        .expect("read audit log");
    assert!(audit.contains("resolution-verified"));
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
