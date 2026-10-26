use passerelle_git_pijul::import_intent::{ImportIntent, ImportIntentStore};
use std::io::ErrorKind;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const PARENT: &str = "abcdef0123456789abcdef0123456789abcdef01";
const BASE: &str = "fedcba9876543210fedcba9876543210fedcba98";
const CHANGE: &str = "Hh9YQ6r3ExamplePijulChangeHash";

fn intent() -> ImportIntent {
    ImportIntent {
        git_sha: SHA.into(),
        parent_sha: PARENT.into(),
        base_sha: BASE.into(),
        pr: 42,
        pijul_change: None,
    }
}

fn mapped_intent() -> ImportIntent {
    ImportIntent {
        pijul_change: Some(CHANGE.into()),
        ..intent()
    }
}

#[test]
fn intent_survives_reload_and_can_be_completed_after_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());
    let prepared = intent();
    let mapped = mapped_intent();

    store.begin(&prepared).unwrap();
    assert_eq!(store.load(SHA).unwrap(), Some(prepared));

    store.mark_mapped(&mapped).unwrap();
    assert_eq!(store.load(SHA).unwrap(), Some(mapped.clone()));

    store.complete(&mapped).unwrap();
    assert_eq!(store.load(SHA).unwrap(), None);
}

#[test]
fn pending_intents_lists_all_shas_in_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());
    let first = intent();
    let second = ImportIntent {
        git_sha: "1111111111111111111111111111111111111111".into(),
        ..intent()
    };
    store.begin(&first).unwrap();
    store.begin(&second).unwrap();

    let pending = store.pending_intents().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].git_sha, SHA);
    assert_eq!(pending[1].git_sha, second.git_sha);
}

#[test]
fn pending_intent_blocks_blind_retry_after_interrupted_import() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());
    let prepared = intent();
    let mapped = mapped_intent();

    store.begin(&prepared).unwrap();
    let error = store.require_no_uncommitted_intent(SHA).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Interrupted);
    assert!(error.to_string().contains("recover explicitly"));

    store.mark_mapped(&mapped).unwrap();
    store.complete(&mapped).unwrap();
    store.require_no_uncommitted_intent(SHA).unwrap();
}

#[test]
fn intent_prevents_two_runs_from_claiming_the_same_sha() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());
    let expected = intent();

    store.begin(&expected).unwrap();
    let error = store.begin(&expected).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert_eq!(store.load(SHA).unwrap(), Some(expected));
}

#[test]
fn committed_ledger_mapping_reconciles_only_the_exact_intent() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());
    let prepared = intent();
    let mapped = mapped_intent();
    store.begin(&prepared).unwrap();
    store.mark_mapped(&mapped).unwrap();

    let wrong_pr = store
        .reconcile_committed(&mapped, SHA, Some(99), CHANGE)
        .unwrap_err();
    assert_eq!(wrong_pr.kind(), ErrorKind::InvalidData);
    assert!(store.load(SHA).unwrap().is_some());

    let wrong_change = store
        .reconcile_committed(&mapped, SHA, Some(42), "different-change")
        .unwrap_err();
    assert_eq!(wrong_change.kind(), ErrorKind::InvalidData);
    assert!(store.load(SHA).unwrap().is_some());

    store
        .reconcile_committed(&mapped, SHA, Some(42), CHANGE)
        .unwrap();
    assert!(store.load(SHA).unwrap().is_none());
}

#[test]
fn rejects_non_oid_path_components_and_corrupt_intents() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());

    let error = store.load("../outside").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);

    let root = dir.path().join(".bridge").join("import-intents");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(format!("{SHA}.json")), "{not json").unwrap();
    let error = store.load(SHA).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);

    let invalid_parent = format!(
        r#"{{"git_sha":"{SHA}","parent_sha":"not-an-oid","base_sha":"{BASE}","pr":42,"pijul_change":null}}"#
    );
    std::fs::write(root.join(format!("{SHA}.json")), invalid_parent).unwrap();
    let error = store.load(SHA).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
}

#[test]
fn cannot_complete_an_intent_before_observing_the_change_hash() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImportIntentStore::new(dir.path());
    let prepared = intent();

    store.begin(&prepared).unwrap();
    let error = store.complete(&prepared).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert_eq!(store.load(SHA).unwrap(), Some(prepared));
}
