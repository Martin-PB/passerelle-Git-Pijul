# Installation de Lean et avancement de la spécification (Ubuntu)

Ce guide installe l'outillage Lean exactement épinglé par le dépôt, lance le
premier build de la spécification, puis propose un plan d'avancement en tranches
vérifiables. Il complète [`local-verification.md`](local-verification.md), qui
décrit la vérification locale complète (Rust + Lean + Pijul).

## 1. Prérequis système

```sh
sudo apt-get update && sudo apt-get install -y curl git build-essential
```

`build-essential` fournit le compilateur C exigé par Lean. Rien d'autre : la
spécification n'importe que `Std` (pas Mathlib), donc aucun téléchargement de
Mathlib n'est nécessaire et le build reste rapide.

## 2. Installer elan et le toolchain épinglé

Le plus simple :

```sh
bash scripts/setup-lean.sh
```

Le script installe elan sous `~/.elan` (jamais de sudo), lit le toolchain
épinglé dans `lean-toolchain`, le télécharge via elan et lance
`lake build Bridge`. Il imprime la ligne `export PATH` à ajouter à `~/.bashrc`
si besoin.

À la main, si vous préférez :

```sh
curl -sSfL https://elan.lean-lang.org/elan-init.sh | sh -s -- -y --default-toolchain none
export PATH="$HOME/.elan/bin:$PATH"   # et l'ajouter dans ~/.bashrc
cd passerelle-Git-Pijul
lean --version   # elan télécharge automatiquement le toolchain de lean-toolchain
lake build Bridge
```

Convention de sortie de `setup-lean.sh` (identique à `verify-local.sh`) :
0 = tout passe, 1 = échec, 2 = prérequis système manquant.

## 3. Premier build : le verdict

```sh
lake build Bridge
```

**Tant que cette commande n'a pas réussi une fois, la spécification est du texte
non vérifié.** Toute affirmation de preuve dans les PRs, issues ou le backlog
doit rester conditionnelle à ce build. Si le build échoue : corriger les erreurs
de compilation dans des commits dédiés (« fix(lean): compile … ») avant
d'ajouter le moindre théorème. C'est la tranche T0 ci-dessous.

## 4. Éditeur

VS Code avec l'extension « lean4 » (vscode-lean4) donne le *lagda* infoview dans
`formal/lean/Bridge/Spec.lean` : les buts et erreurs s'affichent en direct, ce
qui est le moyen le plus rapide de travailler sur les preuves. emacs/vim ont
des équivalents (lean4-mode, nvim lean).

## 5. Plan d'avancement de la spécification

Chaque tranche est une PR/un commit unique, avec son critère d'acceptation.
**Ordre strict : une tranche n'est commencée que si la précédente compile.**

### T0 — Faire compiler le fichier actuel (bloquant, aucun ajout)

Critère d'acceptation : `lake build Bridge` passe sur un environnement propre
(`rm -rf .lake/lakebuild` inutile normalement, mais tester un build depuis
zéro). Consigner le résultat réel (hash du commit + sortie de version) dans le
corps de la PR et dans `docs/design-import.md`.

### T1 — Hygiène du modèle (petits pas, tous compilés)

- Supprimer le code mort (`InRange` défini et jamais utilisé).
- Renforcer `IsSingletonDelta` : la canonique des listes doit être *typée*, pas
  documentée — remplacer les `List ChangeId` par `Finset ChangeId` (ou ajouter
  l'hypothèse `List.Distinct` dans le prédicat), et adapter les deux lemmes
  `singleton_delta_*`.
- Ajouter des `example` d'instanciation : une histoire à 3 commits, un ledger
  avec une entrée, un delta singleton — pour prouver que les définitions ne
  sont pas vacues. Critère : chaque section du fichier a au moins un `example`
  qui l'exerce.

### T2 — Unifier les modèles dupliqués

- Arbres : `GitTree` (§1) et `ObservableTree` (§7) coexistent sans pont.
  Faire de la version observable la seule référence (`abbrev GitTree :=
  ObservableTree` ou un lemme de normalisation), adapter `P0Equivalent`.
- Ledgers : `Ledger` (§4) et `BridgeLedger` (§7) — soit fusionner, soit prouver
  un lemme de cohérence entre `recordChecked` et `record`.
- Critère d'acceptation : un seul modèle d'arbre et un seul modèle de ledger
  dans le fichier ; les anciens noms sont des alias ou des lemmes.

### T3 — Rendre P0Equivalent testable

- Lier `imported` à un prédicat concret (l'équivalent de
  `rev-list base..head` : un ensemble d'OIDs atteignables et couvrant
  l'intervalle), sinon le critère est satisfait par l'ensemble vide.
- Lier `P0Equivalent` aux `ConflictScenario` : définir `gitConflict` et
  `pijulConflict` comme les résultats des opérations appariées du scénario.
- Critère : un `example` où `P0Equivalent` est vrai et un où il est faux
  (conflit non préservé) sur des modèles jouets.

### T4 — Théorèmes de modèle avec un PatchModel jouet

- Instancier `PatchModel` avec une sémantique minimaliste (patches = paires
  (chemin, contenu), deux patches sur chemins disjoints commutent,
  conflit = même chemin, contenus distincts).
- Prouver dans ce jouet : l'ingest sans conflit préserve `TreeEq`, l'idempotence
  observationnelle, la fermeture des dépendances.
- Critère : ces théorèmes ne prouvent rien sur Pijul réel ; ils valident la
  *forme* des définitions. L'écrire dans les commentaires.

### T5 — Réduire la frontière de confiance (hors spécification seule)

Deux pistes déjà documentées, à arbitrer **après** T0–T4 :
- un patch ciblé de `pijul git` émettant un manifeste `OID → change → parents`,
  qui transforme la boîte noire en interface auditée ;
- l'audit Aeneas/hax pour relier le Rust de la passerelle aux définitions.

### Ce qu'il ne faut PAS faire

- Ne pas ajouter d'`axiom` ou de champ supposé égal à la propriété voulue
  (règle déjà suivie par le fichier : la conserver).
- Ne pas déclarer une tranche « faite » sur la base d'un fichier non compilé :
  chaque tranche citée en PR/backlog doit comporter le résultat réel du build.
- Ne pas réimplémenter le format des changes Pijul depuis des diffs Git.

## 6. Intégration continue (quand les crédits reviendront)

Ajouter au workflow CI un job `lean` : `lake build Bridge` sur
`ubuntu-latest`, échec bloquant. Tant que les crédits sont épuisés,
`bash scripts/verify-local.sh` (qui appelle `lake build` si présent) fait
foi, avec les logs locaux hors Git (voir `.gitignore`).
