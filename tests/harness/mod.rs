//! Harnas p0 (backlog 2.1) : outillage du round-trip
//!
//!   export(import(x)) == x   (sans perte sur le texte)
//!
//! Ce module fournit trois briques, indépendantes des stubs translate/export
//! (backlogs 2.2/2.3) :
//!
//!   1. le CORPUS de scénarios synthétiques : chaque scénario construit un
//!      dépôt Git dont les branches entrantes entrent réellement en conflit
//!      sous Git (validé par test) ;
//!   2. l'INVENTAIRE texte d'un répertoire (`snapshot_text`) et le
//!      comparateur sans-perte (`assert_text_lossless`), coeur de la
//!      propriété d'acceptation n°1 ;
//!   3. un TEST de bout en bout (`tests/roundtrip.rs`), aujourd'hui
//!      `#[ignore]` car les fonctions de traduction sont des stubs TODO :
//!      il se débloque avec 2.2 (import réel) et 2.3 (export réel).
//!
//! Le corpus est synthétique : la question 5.1 (dépôts réels, licences,
//! taille) est un arbitrage opérateur en attente et ne bloque PAS cette
//! partie du harnas.

use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// 1. Corpus de scénarios synthétiques
// ---------------------------------------------------------------------------

/// Un scénario du corpus : un dépôt Git de base + des branches entrantes.
pub struct Scenario {
    pub name: &'static str,
    /// Fichiers du commit de base (chemin relatif, contenu).
    pub base_files: Vec<(&'static str, &'static str)>,
    /// Branches entrantes (nom, fichiers écrasés/ajoutés au commit de branche).
    pub branches: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
    /// Paires de branches qui DOIVENT entrer en conflit sous Git :
    /// c'est ce qui rend le scénario utile pour la passerelle.
    pub conflicting_pairs: Vec<(&'static str, &'static str)>,
}

/// Le corpus p0 : trois classes de situations, toutes avec des branches en
/// conflit réel sous Git.
pub fn corpus() -> Vec<Scenario> {
    vec![
        // (a) Conflit binaire classique : trois réécritures de la même
        //     ligne, aucune ne peut commuter.
        Scenario {
            name: "same-line-rewrite",
            base_files: vec![
                ("README.md", "base\n"),
                ("src/lib.rs", "fn main() {\n    println!(\"hello\");\n}\n"),
            ],
            branches: vec![
                ("pr-a", vec![("src/lib.rs", "fn main() {\n    println!(\"from a\");\n}\n"), ("docs/a.md", "a\n")]),
                ("pr-b", vec![("src/lib.rs", "fn main() {\n    println!(\"from b\");\n}\n"), ("docs/b.md", "b\n")]),
                ("pr-c", vec![("src/lib.rs", "fn main() {\n    println!(\"from c\");\n}\n"), ("docs/c.md", "c\n")]),
            ],
            conflicting_pairs: vec![("pr-a", "pr-b"), ("pr-a", "pr-c"), ("pr-b", "pr-c")],
        },
        // (b) Mix commutation + conflit : pr-a et pr-b touchent des fichiers
        //     disjoints (ces changes commutent dans Pijul et le conflit Git
        //     disparaît par construction), pr-c entre en conflit avec les
        //     deux autres sur src/shared.rs.
        Scenario {
            name: "commute-and-conflict",
            base_files: vec![
                ("src/lib.rs", "mod a;\nmod b;\nmod shared;\n"),
                ("src/a.rs", "// a\n"),
                ("src/b.rs", "// b\n"),
                ("src/shared.rs", "// shared\n"),
            ],
            branches: vec![
                ("pr-a", vec![("src/a.rs", "// a edited by pr-a\n")]),
                ("pr-b", vec![("src/b.rs", "// b edited by pr-b\n")]),
                ("pr-c", vec![
                    ("src/shared.rs", "// shared edited by pr-c\n"),
                    ("docs/c.md", "c\n"),
                ]),
                ("pr-d", vec![("src/shared.rs", "// shared edited by pr-d\n")]),
            ],
            conflicting_pairs: vec![("pr-c", "pr-d")],
        },
        // (c) Changement identique sur deux branches (classe SameContent,
        //     auto-résolvable plus tard via les règles 2.5) + un tiers en
        //     conflit réel avec les deux premiers.
        Scenario {
            name: "same-content-plus-conflict",
            base_files: vec![
                ("src/lib.rs", "pub fn f() -> u32 { 1 }\n"),
                ("LICENSE-short.txt", "MIT placeholder\n"),
            ],
            branches: vec![
                ("pr-a", vec![("src/lib.rs", "pub fn f() -> u32 { 2 }\n")]),
                ("pr-b", vec![("src/lib.rs", "pub fn f() -> u32 { 2 }\n")]),
                ("pr-c", vec![("src/lib.rs", "pub fn f() -> u32 { 3 }\n")]),
            ],
            conflicting_pairs: vec![("pr-a", "pr-c"), ("pr-b", "pr-c")],
        },
    ]
}

// ---------------------------------------------------------------------------
// 2. Inventaire texte et comparateur sans-perte
// ---------------------------------------------------------------------------

/// Inventaire le texte d'un répertoire : tous les fichiers réguliers,
/// récursivement, hors `.git` et `.pijul`. C'est la valeur « x » de la
/// propriété d'acceptation (sans perte sur le texte).
pub fn snapshot_text(dir: &Path) -> std::io::Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    fn walk(dir: &Path, prefix: &str, map: &mut BTreeMap<String, String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == ".pijul" {
                continue; // métadonnées de VCS, pas du texte transporté
            }
            let path = entry.path();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if path.is_dir() {
                walk(&path, &rel, map)?;
            } else if path.is_file() {
                let bytes = std::fs::read(&path)?;
                map.insert(rel, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        Ok(())
    }
    walk(dir, "", &mut map)?;
    Ok(map)
}

/// Le comparateur de la propriété n°1 : échoue avec un diff lisible si
/// `after` (l'arbre réexporté) n'est pas exactement `before` (l'arbre
/// importé), texte pour texte.
pub fn assert_text_lossless(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    context: &str,
) {
    let mut problems = Vec::new();
    for (path, expected) in before {
        match after.get(path) {
            Some(actual) if actual == expected => {}
            Some(actual) => problems.push(format!("  {path}: contenu différent\n    attendu: {:?}\n    obtenu:  {:?}", expected, actual)),
            None => problems.push(format!("  {path}: fichier perdu à l'export")),
        }
    }
    for path in after.keys() {
        if !before.contains_key(path) {
            problems.push(format!("  {path}: fichier apparu à l'export (pas dans l'import)"));
        }
    }
    assert!(
        problems.is_empty(),
        "export(import(x)) != x [{context}] :\n{}",
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 3. Exécution des binaires externes
// ---------------------------------------------------------------------------

/// Exécute une commande, renvoie (succès, stdout+stderr concaténés).
pub fn run(dir: &Path, program: &str, args: &[&str]) -> (bool, String) {
    match std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
    {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !text.trim().is_empty() {
                    text.push('\n');
                }
                text.push_str(err.trim_end());
            }
            (out.status.success(), text.trim_end().to_string())
        }
        Err(e) => (false, format!("impossible de lancer {program} : {e}")),
    }
}

pub fn have(program: &str) -> bool {
    run(Path::new("."), program, &["--version"]).0
}

pub fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    run(dir, "git", args)
}

/// Construit le dépôt Git du scénario dans `dir` : commit de base sur main,
/// puis un commit par branche. Renvoie une erreur lisible si git échoue.
pub fn build_git_repo(dir: &Path, s: &Scenario) {
    let (ok, out) = git(dir, &["init"]);
    assert!(ok, "git init a échoué : {out}");
    // Portable : pas de `git init -b` (version-dépendant), on pointe HEAD.
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(dir, &["config", "user.name", "corpus"]);
    git(dir, &["config", "user.email", "corpus@example.invalid"]);
    for (path, content) in &s.base_files {
        let p = dir.join(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(p, content).expect("write base file");
    }
    let (ok, out) = git(dir, &["add", "."]);
    assert!(ok, "git add (base) a échoué : {out}");
    let (ok, out) = git(dir, &["commit", "-m", "base"]);
    assert!(ok, "git commit (base) a échoué : {out}");
    for (branch, files) in &s.branches {
        let (ok, out) = git(dir, &["switch", "-c", branch]);
        assert!(ok, "git switch -c {branch} a échoué : {out}");
        for (path, content) in files {
            let p = dir.join(path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("create parent dir");
            }
            std::fs::write(p, content).expect("write branch file");
        }
        let (ok, out) = git(dir, &["add", "."]);
        assert!(ok, "git add ({branch}) a échoué : {out}");
        let (ok, out) = git(dir, &["commit", "-m", &format!("commit {branch}")]);
        assert!(ok, "git commit ({branch}) a échoué : {out}");
        let (ok, out) = git(dir, &["switch", "main"]);
        assert!(ok, "git switch main (après {branch}) a échoué : {out}");
    }
}

/// Vérifie qu'un merge Git de `theirs` dans la branche courante échoue
/// (conflit réel), puis annule proprement le merge. Renvoie true si le
/// merge a bien échoué ou produit des marqueurs de conflit.
pub fn git_merge_conflicts(dir: &Path, theirs: &str) -> bool {
    let (ok, out) = git(dir, &["merge", "--no-ff", "--no-commit", theirs]);
    let conflicted = !ok || out.contains("CONFLICT") || out.contains("Conflict");
    // Nettoyage systématique, quel que soit le résultat.
    if !ok {
        git(dir, &["merge", "--abort"]);
    } else {
        git(dir, &["reset", "--hard", "HEAD"]);
    }
    conflicted
}
