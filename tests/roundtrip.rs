//! Propriété d'acceptation n°1 : export(import(x)) == x, sur le corpus p0.
//!
//! Trois niveaux, du plus exécutable au plus en attente :
//!
//! 1. `corpus_scenarios_are_really_conflicting` — exécutable dès que `git`
//!    est présent : prouve que le corpus est utile (les paires annoncées
//!    conflictuelles le sont réellement sous Git).
//! 2. `snapshot_text_is_deterministic` — pur Rust : l'inventaire texte et le
//!    comparateur sans-perte font ce qu'ils annoncent (stable, sourd aux
//!    métadonnées VCS, détection de perte ET d'apparition).
//! 3. `roundtrip_property_end_to_end` — le test de bout en bout complet ;
//!    `#[ignore]` jusqu'à ce que le pipeline d'import, l'exporteur externe
//!    et le matérialiseur de sortie soient validés sur le corpus. Les appels
//!    historiques sont conservés comme squelette, pas comme preuve.

mod harness;

use harness::{assert_text_lossless, corpus, snapshot_text};
use std::path::Path;

fn skip_unless(cond: bool, what: &str) -> bool {
    if !cond {
        eprintln!("SKIP : {what} indisponible sur cette machine");
    }
    cond
}

#[test]
fn corpus_scenarios_are_really_conflicting() {
    if !skip_unless(harness::have("git"), "git") {
        return;
    }
    for scenario in corpus() {
        let dir = tempfile::tempdir().expect("tempdir");
        harness::build_git_repo(dir.path(), &scenario);
        for (mine, theirs) in &scenario.conflicting_pairs {
            let (ok, out) = harness::git(dir.path(), &["switch", mine]);
            assert!(ok, "switch vers {mine} a échoué : {out}");
            assert!(
                harness::git_merge_conflicts(dir.path(), theirs),
                "corpus cassé : {mine} et {theirs} du scénario {:?} \
                 ne conflitent PAS sous Git — le scénario ne teste rien",
                scenario.name
            );
        }
        // Le répertoire temporaire est nettoyé au drop.
    }
}

#[test]
fn snapshot_text_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::write(root.join("README.md"), "hello\n").expect("write README");
    std::fs::write(root.join("src/lib.rs"), "fn f() {}\n").expect("write lib");
    std::fs::write(root.join(".ignore"), "local-only\n").expect("write Pijul ignore config");
    // Métadonnées VCS : ignorées par l'inventaire.
    std::fs::create_dir_all(root.join(".git")).expect("mkdir .git");
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");

    let snap = snapshot_text(root).expect("snapshot");
    assert_eq!(
        snap,
        [
            ("README.md".to_string(), "hello\n".to_string()),
            ("src/lib.rs".to_string(), "fn f() {}\n".to_string()),
        ]
        .into_iter()
        .collect(),
        "l'inventaire doit ignorer .git/.pijul et capturer le texte"
    );

    // Sans-perte : même arbre -> silence. Une perte ET une apparition ->
    // rapportées toutes les deux.
    let mut after = snap.clone();
    after.remove("README.md");
    after.insert("ghost.md".to_string(), "apparu\n".to_string());
    let err = std::panic::catch_unwind(|| {
        assert_text_lossless(&snap, &after, "test");
    });
    assert!(err.is_err(), "le comparateur doit rejeter un arbre différent");
    let expected = snap.clone();
    assert_text_lossless(&expected, &snap, "test identité");
}

fn build_mirror(
    repo: &Path,
    mirror: &Path,
    base_sha: &str,
    prs: &[passerelle_git_pijul::PullRequest],
) {
    let repo_path = repo.to_str().expect("UTF-8 repo path");
    let mirror_path = mirror.to_str().expect("UTF-8 mirror path");
    let (ok, output) = harness::run(repo, "git", &["clone", "--bare", repo_path, mirror_path]);
    assert!(ok, "git clone --bare failed: {output}");

    let mirror_arg = mirror.to_str().expect("UTF-8 mirror path");
    let (ok, output) = harness::run(
        Path::new("."),
        "git",
        &["--git-dir", mirror_arg, "update-ref", "refs/remotes/base/main", base_sha],
    );
    assert!(ok, "pin base ref failed: {output}");
    for pr in prs {
        let (ok, output) = harness::run(
            Path::new("."),
            "git",
            &[
                "--git-dir",
                mirror_arg,
                "update-ref",
                &format!("refs/remotes/pr/{}", pr.number),
                &pr.head_sha,
            ],
        );
        assert!(ok, "pin PR {} ref failed: {output}", pr.number);
    }
}

#[test]
fn synthetic_import_pipeline_reports_pijul_conflicts_when_available() {
    if !skip_unless(harness::have("git"), "git")
        || !skip_unless(harness::have("pijul"), "pijul")
    {
        return;
    }

    for scenario in corpus() {
        let repo = tempfile::tempdir().expect("synthetic Git repo");
        harness::build_git_repo(repo.path(), &scenario);
        let (ok, base_sha) = harness::git(repo.path(), &["rev-parse", "main"]);
        assert!(ok, "cannot resolve base main: {base_sha}");
        let prs: Vec<_> = scenario
            .branches
            .iter()
            .enumerate()
            .map(|(index, (branch, _))| {
                let (ok, head_sha) = harness::git(repo.path(), &["rev-parse", branch]);
                assert!(ok, "cannot resolve branch {branch}: {head_sha}");
                passerelle_git_pijul::PullRequest {
                    number: index as u64 + 1,
                    title: format!("PR {}", index + 1),
                    author: "synthetic-corpus".into(),
                    head_sha,
                    head_ref: format!("refs/remotes/pr/{}", index + 1),
                }
            })
            .collect();
        let work = tempfile::tempdir().expect("gateway workspace");
        let mirror = work.path().join("mirror.git");
        build_mirror(repo.path(), &mirror, &base_sha, &prs);
        let base = passerelle_git_pijul::Base {
            branch: "main".into(),
            sha: base_sha,
        };
        let pijul_repo = work.path().join("pijul");
        let scratch = work.path().join("scratch");
        let mut ledger = passerelle_git_pijul::ledger::Ledger::default();
        for pr in &prs {
            passerelle_git_pijul::translate::translate_pr_from_mirror(
                &mirror,
                &pijul_repo,
                &scratch,
                &base,
                pr,
                &mut ledger,
            )
            .unwrap_or_else(|error| panic!("import {} failed in {}: {error}", pr.number, scenario.name));
        }
        let triage =
            passerelle_git_pijul::triage::triage_integration_channel(&pijul_repo)
                .unwrap_or_else(|error| panic!("triage failed in {}: {error}", scenario.name));
        assert!(
            triage
                .iter()
                .any(|item| matches!(item, passerelle_git_pijul::triage::Triage::Operator { .. })),
            "scenario {} must expose at least one unresolved Pijul conflict",
            scenario.name
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Propriété de bout en bout
// ---------------------------------------------------------------------------

#[test]
#[ignore = "attend un mécanisme déterministe de résolution des conflits du corpus"]
fn roundtrip_property_end_to_end() {
    if !skip_unless(harness::have("git"), "git")
        || !skip_unless(harness::have("pijul"), "pijul")
        || !skip_unless(harness::have("pijul-export"), "pijul-export")
    {
        return;
    }

    for scenario in corpus() {
        let repo = tempfile::tempdir().expect("synthetic Git repo");
        harness::build_git_repo(repo.path(), &scenario);
        let (ok, base_sha) = harness::git(repo.path(), &["rev-parse", "main"]);
        assert!(ok, "cannot resolve base main: {base_sha}");
        let prs: Vec<_> = scenario
            .branches
            .iter()
            .enumerate()
            .map(|(index, (branch, _))| {
                let (ok, head_sha) = harness::git(repo.path(), &["rev-parse", branch]);
                assert!(ok, "cannot resolve branch {branch}: {head_sha}");
                passerelle_git_pijul::PullRequest {
                    number: index as u64 + 1,
                    title: format!("PR {}", index + 1),
                    author: "synthetic-corpus".into(),
                    head_sha,
                    head_ref: format!("refs/remotes/pr/{}", index + 1),
                }
            })
            .collect();

        let work = tempfile::tempdir().expect("gateway workspace");
        let mirror = work.path().join("mirror.git");
        build_mirror(repo.path(), &mirror, &base_sha, &prs);
        let base = passerelle_git_pijul::Base {
            branch: "main".into(),
            sha: base_sha,
        };
        let pijul_repo = work.path().join("pijul");
        let scratch = work.path().join("scratch");
        let mut ledger = passerelle_git_pijul::ledger::Ledger::default();
        for pr in &prs {
            passerelle_git_pijul::translate::translate_pr_from_mirror(
                &mirror,
                &pijul_repo,
                &scratch,
                &base,
                pr,
                &mut ledger,
            )
            .unwrap_or_else(|error| panic!("import {} failed in {}: {error}", pr.number, scenario.name));
        }

        let triage = passerelle_git_pijul::triage::triage_integration_channel(&pijul_repo)
            .unwrap_or_else(|error| panic!("triage failed in {}: {error}", scenario.name));
        assert!(
            triage
                .iter()
                .all(|item| matches!(item, passerelle_git_pijul::triage::Triage::Clean)),
            "scenario {} needs explicit resolution changes before round-trip export",
            scenario.name
        );

        let exported = passerelle_git_pijul::translate::export_integration(
            work.path(),
            &mirror,
            &mut ledger,
            passerelle_git_pijul::ExportMode::Linear,
            &base,
            &prs,
        )
        .expect("validated linear export must succeed");
        assert_eq!(exported, "refs/heads/gateway/integration");

        let exported_worktree = tempfile::tempdir().expect("export worktree");
        let checkout = exported_worktree.path().join("checkout");
        let (ok, output) = harness::run(
            Path::new("."),
            "git",
            &[
                "--git-dir",
                mirror.to_str().expect("UTF-8 mirror path"),
                "worktree",
                "add",
                "--detach",
                checkout.to_str().expect("UTF-8 worktree path"),
                &exported,
            ],
        );
        assert!(ok, "cannot materialize exported branch: {output}");
        let imported_text = snapshot_text(&pijul_repo).expect("snapshot Pijul integration");
        let exported_text = snapshot_text(&checkout).expect("snapshot exported Git branch");
        assert_text_lossless(&imported_text, &exported_text, scenario.name);
    }
}
