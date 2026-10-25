//! Acceptance property #1 of the project: the round-trip must be lossless
//! on text.
//!
//!   export(import(x)) == x
//!
//! The full pipeline requires `git` and `pijul` on PATH and is exercised by
//! the harness in `tests/harness/` (p0). What this file pins down today are
//! the properties the ledger and the operator boundary must hold in pure
//! Rust, so they stay green in CI regardless of the external binaries.

use passerelle_git_pijul::ledger::{Direction, Entry, Ledger, ledger_path};
use passerelle_git_pijul::operator::{ConflictReport, OperatorQueue, PrContext, ResolutionChange};

#[test]
fn ledger_is_idempotent_on_re_record() {
    let mut l = Ledger::default();
    let e = |sha: &str| Entry {
        git_sha: sha.into(),
        pijul_change: format!("change-{sha}"),
        pr: Some(42),
        direction: Direction::GitToPijul,
        recorded_at: "t".into(),
    };
    assert!(l.record(e("abc")));
    assert!(l.record(e("abc"))); // re-record same: fine
    assert_eq!(l.get("abc").unwrap().pijul_change, "change-abc");
}

#[test]
fn ledger_detects_force_push() {
    let mut l = Ledger::default();
    assert!(l.record(Entry {
        git_sha: "abc".into(),
        pijul_change: "change-1".into(),
        pr: None,
        direction: Direction::GitToPijul,
        recorded_at: "t".into(),
    }));
    // Same SHA mapped to a different change: the Git side moved.
    assert!(!l.record(Entry {
        git_sha: "abc".into(),
        pijul_change: "change-2".into(),
        pr: None,
        direction: Direction::GitToPijul,
        recorded_at: "t".into(),
    }));
}

#[test]
fn ledger_survives_save_load_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut l = Ledger::default();
    l.record(Entry {
        git_sha: "sha1".into(),
        pijul_change: "chg1".into(),
        pr: Some(7),
        direction: Direction::PijulToGit,
        recorded_at: "t".into(),
    });
    let p = ledger_path(dir.path());
    l.save(&p).unwrap();
    let back = Ledger::load(&p).unwrap();
    assert_eq!(back.get("sha1").unwrap().pijul_change, "chg1");
}

#[test]
fn ledger_save_atomically_replaces_existing_file_and_cleans_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = ledger_path(dir.path());
    let mut first = Ledger::default();
    first.record(Entry {
        git_sha: "sha-one".into(),
        pijul_change: "change-one".into(),
        pr: Some(1),
        direction: Direction::GitToPijul,
        recorded_at: "t1".into(),
    });
    first.save(&path).unwrap();

    let mut second = Ledger::default();
    second.record(Entry {
        git_sha: "sha-one".into(),
        pijul_change: "change-one".into(),
        pr: Some(1),
        direction: Direction::GitToPijul,
        recorded_at: "t1".into(),
    });
    second.record(Entry {
        git_sha: "sha-two".into(),
        pijul_change: "change-two".into(),
        pr: Some(2),
        direction: Direction::GitToPijul,
        recorded_at: "t2".into(),
    });
    second.save(&path).unwrap();

    let loaded = Ledger::load(&path).unwrap();
    assert_eq!(loaded.get("sha-one").unwrap().pijul_change, "change-one");
    assert_eq!(loaded.get("sha-two").unwrap().pijul_change, "change-two");

    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, vec![std::ffi::OsString::from("gateway-ledger.json")]);
}

#[test]
fn operator_boundary_is_producer_agnostic() {
    // The queue accepts a ResolutionChange and nothing else. Whether the
    // change was produced by a lone operator or an operator assisted by an
    // LLM (via mcp-server-pijul on the gateway's workdir), the gateway sees
    // the same payload. This test pins that: there is no field, tag or
    // capability in the contract that could distinguish a producer.
    let mut q = OperatorQueue::default();
    q.pending.push(ConflictReport {
        path: "src/lib.rs".into(),
        conflict_state: "<< Pijul conflict state >>".into(),
        prs: vec![
            PrContext { number: 1, title: "A".into(), author: "alice".into() },
            PrContext { number: 2, title: "B".into(), author: "bob".into() },
        ],
    });
    q.submit(ResolutionChange {
        change_hash: "XYZ123".into(),
        note: Some("chose PR 1's approach".into()),
    });
    assert!(q.pending.is_empty());
    assert_eq!(q.received.len(), 1);
    assert_eq!(q.received[0].change_hash, "XYZ123");
}
