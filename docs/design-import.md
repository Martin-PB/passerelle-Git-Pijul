# Spike 2.2 — design de la traduction aller réelle (Git → Pijul)

> Statut : **spike documenté, jamais exécuté** (backlog 2.2). Ce document fixe le
> design de l'import réel AVANT le code, comme décidé en arbitrage. Toutes les
> affirmations sur le comportement de `pijul git` proviennent de la lecture du
> code source de la commande (`pijul/src/commands/git.rs`, branche beta) —
> elles sont citées comme telles, et restent à confirmer par exécution dès que
> les crédits CI permettront de lancer le harnas 2.1.

## 1. Ce que fait réellement `pijul git` (lu dans le code source)

Interface : `pijul git [<git_path>] [<pijul_path>]`.

- Ouvre le dépôt Git à `git_path` (défaut : répertoire courant) et importe
  **toute l'histoire atteignable depuis HEAD** dans le dépôt Pijul à
  `pijul_path` (créé s'il n'existe pas).
- **Refuse** de partir si des fichiers non commités existent dans la copie
  (hors `.pijul`/`.ignore`) : l'ingest doit garantir une copie propre.
- **Un channel par commit, nommé par l'OID complet du commit** (pas par le nom
  de branche) : le channel `<OID>` contient l'état (l'ensemble des changes)
  après ce commit. L'import est un DAG walk parents→enfants : chaque commit
  produit exactement **un change Pijul**, appliqué sur le channel de son
  parent, puis renommé `<OID du commit>`.
- **Incrémental** : un état `Oid → Merkle` est persisté dans
  `.pijul/git/db` (sanakirja). Un re-run repart des commits déjà importés
  (`dag.root` ne contient que les OID avec état connu). C'est le mécanisme
  naturel d'idempotence de la passerelle.
- À la fin, le working copy est matérialisé à partir du channel du HEAD.

Conséquence directe pour la passerelle : le mapping **SHA ↔ change-hash** que
le ledger doit tenir est **calculable par l'interface CLI standard**, sans
lire la base pristine :

```
changes(<OID>)  = pijul log --hash-only --channel <OID>
change du commit = changes(<OID>) − changes(<OID>^)     (différence d'ensembles)
```

`git rev-list <base>..<head>` fournit l'ordre (oldest-first) ; chaque OID
ayant un channel, l'appariement SHA ↔ change est déterministe, un change
exactement par commit — aucune heuristique d'ordre à inventer.

## 2. Pipeline aller v0 (design proposé)

1. **Ingest** (existant) : miroir Git avec `refs/remotes/pr/<n>` par PR.
2. **Préparation du HEAD par PR** : `pijul git` importe *depuis HEAD* — le
   miroir doit donc présenter HEAD = tête de la PR (checkout du ref dans un
   clone de travail par PR, ou `git checkout --detach <head_sha>`). Un clone
   de travail distinct **par PR** (pas un seul miroir partagé) évite toute
   contention sur HEAD et permet le parallélisme.
3. **Import** : `pijul git <clone-PR> <workdir-pijul>` — le même dépôt Pijul
   de travail reçoit les imports successifs ; l'incrément Oid→Merkle rend
   les re-runs sûrs (l'import d'un historique déjà vu est quasi gratuit).
4. **Base** : la branche cible (`main` Git) est importée d'abord (même
   mécanique) : le channel de son OID de tête sert d'ancêtre commun.
5. **Mapping** : pour chaque commit de `git rev-list <base>..<head>` :
   `change = changes(<OID>) − changes(<OID>^)`, entrée de ledger
   `git_sha ↔ pijul_change` (direction GitToPijul), détection de force-push
   inchangée (un SHA déjà ledgé vers un change différent = force-push).
6. **Channel `pr-<n>`** : les channels `<OID>` sont un artefact interne de
   l'import, illisibles pour un opérateur. La passerelle matérialise un
   channel lisible `pr-<n>` et y applique les changes de la PR. L'exposition
   à l'opérateur (et à l'outillage MCP) ne passe QUE par des channels nommés.
7. **Channel `integration`** : les changes de toutes les PRs y sont
   appliqués dans l'ordre de leurs dépendances (le triage 2.5 branchera là).

## 3. Décisions proposées (à valider en revue)

- **D1 — une commande d'import par PR** (clone de travail par PR), pas un
  import multi-branches : HEAD pilote l'import ; parallélisme ; pas de
  partage d'état git entre PRs. Coût : N runs `pijul git`, atténué par
  l'incrément.
- **D2 — mapping par différence d'ensembles** entre `changes(<OID>)` et
  `changes(<OID>^)` : déterministe, une commande `pijul log` par extrémité,
  zéro accès à la base pristine. Doit être prouvé par le harnas 2.1 dès
  qu'exécutable (à crédits CI réapprovisionnés).
- **D3 — les channels `<OID>` ne sortent jamais de la passerelle** : seule
  la couche nommée `pr-<n>`/`integration` est exposée.
- **D4 — ordre d'application** : `git rev-list` oldest-first (les dépendances
  sont alors déjà en place ; elles suivraient de toute façon via `apply`).
- **D5 — idempotence** : l'état incrémental de `pijul git` + le ledger
  suffisent ; aucun nettoyage des channels `<OID>` en v0 (le coût de
  renommage/suppression est différé à l'observation réelle).

## 4. Inconnus restants (tranchés à l'exécution, pas avant)

- Comportement de `pijul git` sur un clone **shallow** (le DAG walk des
  parents s'arrête-t-il proprement aux frontières shallow ?).
- Coût des channels par commit sur des historiques longs (atténué par des
  branches de PR courtes, design initial du projet).
- Comportement si le miroir a des fichiers non commités (refus documenté —
  l'ingest doit garantir la propreté).
- L'export retour (2.3) reste hors périmètre de ce spike : le README prévoit
  `git commit-tree` en mode linear et un merge commit par PR en
  merge-topology.


## 5. Briques existantes (recherche du 24/10/2026 — ne pas réimplémenter avant d'avoir vérifié)

Réponse à la vigilance légitime « ne recréons-nous pas pijul git ? » : non —
l'import est appelé comme boîte noire (D1). La même vigilance s'applique au
sens retour (2.3), voici l'état de l'art trouvé :

| Outil | Sens | État | Verdict pour la passerelle |
|---|---|---|---|
| `pijul git` (upstream) | Git → Pijul | Intégré au CLI beta | Boîte noire de l'import (D1) |
| `pijul-export` (andybalholm, Nest) | **Pijul → Git** | Rust ; flux `git fast-export`, historique linéaire d'un channel, **fichiers de marks** pour la correspondance incrémentale | **Candidat de réutilisation pour 2.3 mode linear** — à auditer : licence (COPYING présent, contenu non vérifié), maintenance face à beta.24, préservation auteurs/dates |
| `git-pijul` (ganwell, PyPI) | Git → Pijul (+ `set-diff`/`set-union`/`set-intersection` sur channels) | Python ; import par ancestry-path | Import remplacé par `pijul git` ; **les opérations ensemblistes sur les changes valent le détour pour le triage 2.5** |
| `PijulGit` (purplesyringa) | Bidirectionnel (miroir webhooks) | Python, 2019, pijul 0.12, GitLab+Nest seulement | Référence historique ; API pijul pré-beta, pas de GitHub, pas de sémantique PR |
| Nest (hébergement officiel) | — | Code vérifié : **aucune interface Git native** ; le manuel ne documente que l'échange de patches | Hypothèse « le Nest sert déjà git » : réfutée ; aucun pont à attendre de ce côté |
| Discussion upstream #803 | export/import pijul | En discussion sur le Nest | À suivre (veille 3.5) : si `pijul export` atterrit en upstream, réévaluer 2.3 |

### Le sens Pijul → GitHub n'existe nulle part

GitHub = git + refs de PR (`refs/pull/<n>/head`) + sémantique merge/queue.
Le mécanique (`git fast-import`/plumbing) est couvrable par les briques
ci-dessus ; la **sémantique PR de GitHub reste à nous** : détection de
force-push sur événement `synchronize` (le ledger la fait déjà), topologie
merge-commit par PR (mode `merge-topology`, absent de `pijul-export` qui
est purement linéaire), et la frontière opérateur. Conclusion : la passerelle
est la couche orchestration + le chaînon manquant, pas une réécriture.

### Décision de périmètre (opérateur, 24/10/2026)

- **D9** : la passerelle est **bidirectionnelle Pijul ↔ Git et agnostique de
  l'hébergement** (ni Nest, ni GitHub ciblés par le cœur). Le cœur ne
  connaît que refs git et changes pijul ; la sémantique d'hébergement
  (PRs, merge queues, événements `synchronize`) vivra dans des greffons
  frontaux qui la traduisent en refs. Conséquences concrètes : le mode
  `merge-topology` de 2.3 doit rester exprimable **sans** GitHub (un merge
  commit par branche entrante, génériquement) ; l'ingest du 2.2 prend des
  refs en entrée, pas des numéros de PR ; `pr-<n>` est une convention de
  nommage des channels, pas un type du cœur. Le README est aligné par la
  PR dédiée (branche mp-bidir-agnostic).

### Décisions mises à jour

- **D6** : avant d'implémenter 2.3 mode linear, auditer `pijul-export`
  (licence, beta.24, auteurs/dates) ; réutiliser ou dériver, ne pas
  réécrire. Le mode `merge-topology` reste de fabrication maison.
- **D7** : les fichiers de marks de `pijul-export` et notre ledger servent
  le même but (correspondance incrémentale) — si réutilisation, le ledger
  s'aligne sur le format marks plutôt que l'inverse.
- **D8** : étudier les opérations ensemblistes de `git-pijul` pour le
  triage (2.5), où la sémantique union/intersection des changes est centrale.

## 6. Prochaines étapes

1. Faire tourner le harnas 2.1 (crédits CI) pour valider le corpus.
2. Implémenter 2.2 selon D1–D5, avec le mapping D2 couvert par le test
   `roundtrip_property_end_to_end` (dé-ignoré à ce moment-là).
3. 2.3 (export réel) selon le même canevas : spike d'abord si nécessaire.
