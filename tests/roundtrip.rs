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

// ---------------------------------------------------------------------------
// 3. Propriété de bout en bout — débloquée par les backlogs 2.2 et 2.3
// ---------------------------------------------------------------------------

#[test]
#[ignore = "squelette historique : nécessite le pipeline miroir→channels et la matérialisation de la branche exportée"]
fn roundtrip_property_end_to_end() {
    if !skip_unless(harness::have("git"), "git") || !skip_unless(harness::have("pijul"), "pijul") {
        return;
    }
    for scenario in corpus() {
        // x = le dépôt Git du scénario (branche de base + branches de PRs).
        let repo = tempfile::tempdir().expect("tempdir");
        harness::build_git_repo(repo.path(), &scenario);

        // L'inventaire texte de l'état fusionné attendu : la branche main
        // enrichie de chaque branche de PR (pour les changes qui commutent).
        let work = tempfile::tempdir().expect("tempdir");

        let base = passerelle_git_pijul::Base {
            branch: "main".into(),
            sha: String::new(), // résolu par l'ingest, pas par le harnas
        };
        let prs: Vec<_> = scenario
            .branches
            .iter()
            .enumerate()
            .map(|(i, (branch, _))| passerelle_git_pijul::PullRequest {
                number: i as u64 + 1,
                title: format!("PR {}", i + 1),
                author: "corpus".into(),
                head_sha: String::new(), // résolu par l'ingest
                head_ref: branch.to_string(),
            })
            .collect();

        let mut ledger = passerelle_git_pijul::ledger::Ledger::default();
        for pr in &prs {
            passerelle_git_pijul::translate::translate_pr(work.path(), &base, pr, &mut ledger)
                .expect("la traduction aller doit réussir");
        }
        // (l'étape fusion/tri/opérateur du pipeline est couverte par 2.5/2.6 ;
        // la propriété n°1 ne porte que sur la perte de texte.)
        let exported = passerelle_git_pijul::translate::export_integration(
            work.path(),
            repo.path(),
            &mut ledger,
            passerelle_git_pijul::ExportMode::Linear,
            &base,
            &prs,
        )
        .expect("l'export doit réussir");
        assert_ne!(exported, "TODO-exported-branch", "l'export est encore un stub");

        // TODO(2.3) : `exported` est le nom de la branche Git produite ; le
        // harnas devra la matérialiser (clone ou worktree) pour comparer son
        // inventaire texte à celui de l'état importé. Esquisse de la
        // comparaison, ajustée quand l'export réel existera :
        let imported_text = snapshot_text(work.path()).expect("snapshot côté import");
        let exported_text = snapshot_text(work.path())
            .expect("snapshot côté export (matérialisation : TODO 2.3)");
        assert_text_lossless(&imported_text, &exported_text, scenario.name);
    }
}
