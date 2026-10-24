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
3. **Import** : `pijul git <clone-SHA> <workdir-pijul>` — signature confirmée
   dans la source exacte de beta.24. Chaque SHA est présenté via un clone Git
   temporaire détaché, tandis que le dépôt Pijul/cache reste partagé dans
   `workdir-pijul`. Les imports doivent être sérialisés tant qu'ils modifient
   ce dépôt/cache partagé ; l'idempotence doit être démontrée par le test
   d'intégration, et non déduite du seul code source.
4. **Base** : la branche cible (`main` Git) est importée d'abord (même
   mécanique) : le channel de son OID de tête sert d'ancêtre commun.
5. **Mapping** : traiter `git rev-list <base>..<head>` oldest-first.
   La source beta.24 supprime les channels OID des parents lorsqu'ils n'ont
   plus de descendants à importer. Il faut donc lire `changes(parent)` avant
   d'importer chaque commit, importer ce commit, puis lire immédiatement
   `changes(commit)` et calculer la différence d'ensembles. Le mapping
   `git_sha ↔ pijul_change` est alors inscrit dans le ledger ; un SHA déjà
   lié à un autre change reste une erreur fermée.
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
- **D2 — mapping par différence d'ensembles capturée en ligne** :
  lire l'état du parent avant l'import de l'enfant, puis l'état de l'enfant
  juste après. La source beta.24 supprime des channels OID parents pendant
  l'import ; les états ne peuvent donc pas tous être interrogés après un
  import global de la tête. À prouver par le harnas 2.1 à l'exécution.
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

## 6. État de l'implémentation (24/10/2026)

L'arbitrage Pijulation 5.1 est tranché : **corpus synthétiques uniquement pour
P0**. Les dépôts sont générés en tests dans des répertoires temporaires ; aucun
projet tiers, aucune licence externe et aucun accès réseau ne sont requis.

Première tranche du code d'ingest sur la branche `agent/p0-synthetic-roundtrip` :

- initialisation explicite d'un miroir Git bare (au lieu d'appeler `git fetch`
  dans un simple répertoire) ;
- récupération des refs génériques avec historique complet ;
- épinglage des têtes sous `refs/remotes/pr/<n>` comme convention locale ;
- vérification du SHA attendu pour refuser une ref qui a bougé entre l'événement
  d'hébergement et l'ingest ;
- tests d'intégration synthétiques pour le miroir bare, le parent de commit et
  le refus d'une tête périmée / d'un répertoire non-Git non vide.

**Statut de validation : NOT RUN.** L'environnement d'exécution courant ne peut
pas cloner le dépôt depuis GitHub ; les tests n'ont donc pas été compilés ni
exécutés ici. Le test de round-trip reste ignoré et les fonctions de traduction
et d'export contiennent encore des stubs. Cette tranche ne constitue pas une
preuve P0 et ne doit pas être annoncée comme telle.

## 7. Prochaines étapes

1. Exécuter localement les tests synthétiques et corriger tout échec réel.
2. Implémenter l'import 2.2 selon D1–D5 : clone de travail isolé par ref,
   appel à `pijul git`, mapping SHA ↔ change par différence d'ensembles, puis
   channels nommés ; déverrouiller le test end-to-end seulement lorsque ces
   opérations sont réellement exécutables.
3. Auditer `pijul-export` (licence, beta.24, auteurs/dates et marks), puis
   implémenter l'export 2.3 avant de revendiquer `export(import(x)) == x`.


## 8. Implémentation ajoutée après l'arbitrage P0 (24/10/2026)

La branche `agent/p0-synthetic-roundtrip` contient maintenant les primitives
Rust suivantes dans `src/translate.rs` :

- `import_git_commit(mirror, pijul_repo, sha, scratch)` vérifie le SHA,
  crée un clone temporaire par import et appelle `pijul git <git_path>
  <pijul_path>`. Cette signature est confirmée dans le source de beta.24.
  La correction restante est de sérialiser l'accès au dépôt/cache Pijul
  partagé et de vérifier l'import incrémental et le mapping à l'exécution.
- `changes_on_channel` lit les hashes via `pijul log --hash-only`.
- `change_for_git_commit` calcule la différence entre l'ensemble de changes
  du channel du commit et celui de son parent.
- `unique_new_change` refuse les cas à zéro ou plusieurs changes au lieu de
  fabriquer une correspondance approximative. Des tests purs couvrent les
  trois cardinalités (1, 0, plusieurs).

**Avancement suivant :** le chemin est maintenant câblé dans `merge-prs`.
La commande résout `refs/remotes/base/<nom>` et `refs/remotes/pr/<n>` dans le
miroir, vérifie l’ascendance base→tête, importe les commits, refuse les commits
de merge non pris en charge en P0, crée `pr-<n>` depuis la base et applique
les changes vers `integration`. Une tête forcée non fast-forward est refusée
pour éviter d’écraser le channel existant. Le ledger est écrit à côté du dépôt
Pijul.

**Limites :** tests Rust **NOT RUN** (Cargo indisponible) et commandes Pijul
non validées par exécution. Le triage/opérateur et l’export Pijul → Git restent
incomplets ; le round-trip P0 n’est pas démontré.


## 9. Audit initial de `pijul-export` (24/10/2026)

Le dépôt officiel du projet est visible sur le Nest :
`https://nest.pijul.com/andybalholm/pijul-export`. Son README décrit un
exporteur `git fast-export` pour un historique **linéaire d'un channel**,
consommable via `git fast-import`, avec options `--channel`, `--branch` et
fichiers de marks pour suivre la correspondance Pijul/Git. C'est un candidat
technique plausible pour le mode `linear`, pas pour la topologie de merge
par PR sans traitement supplémentaire.

**Blocage licence :** le fichier `COPYING` existe, mais son contenu visible
est vide (aucune licence explicite vérifiable). La passerelle ne doit donc
pas incorporer, copier ou redistribuer ce code tant que son statut de licence
n'est pas clarifié. Le README ne prouve pas non plus la compatibilité avec
la version exacte de Pijul visée ici. Décision provisoire : garder
`pijul-export` comme outil externe facultatif à évaluer, ne pas en faire une
dépendance de compilation et ne pas prétendre que l'export 2.3 est résolu.

Références consultées :
- README et description de l'interface : https://nest.pijul.com/andybalholm/pijul-export
- Fichier COPYING : https://nest.pijul.com/andybalholm/pijul-export:main/tree/IYGPU3IZMM2BI.LMAAA


## 10. Export linéaire : adaptateur expérimental (24/10/2026)

Un adaptateur a été ajouté dans `src/export.rs`. Il invoque un exécutable
`pijul-export` installé séparément, transmet son flux à `git fast-import`
dans le miroir bare, puis ne publie la ref locale
`refs/heads/gateway/integration` qu'après deux contrôles :

1. le commit exporté descend de la base Git épinglée ;
2. l'arbre Git exporté est identique à l'arbre du working copy Pijul sur le
   channel `integration` (hors métadonnées internes `.pijul` et `.ignore`).

Une ref de sortie existante n'est réutilisée que si sa base et son arbre sont
identiques ; une ref différente n'est jamais écrasée. L'adaptateur n'effectue
aucun push distant. Le mode `merge-topology` reste explicitement non pris en
charge.

**Non validé à l'exécution.** Cet adaptateur dépend d'un binaire externe dont
la compatibilité exacte avec la version beta de Pijul n'a pas été vérifiée.
Le binaire n'est ni incorporé ni redistribué ; le statut de licence upstream
reste à clarifier. L'égalité d'arbre est vérifiée à l'exécution, mais cela ne
remplace pas le test P0 synthétique de bout en bout.


Le triage P0 lit aussi `pijul status` après avoir basculé explicitement sur
`integration`. Une erreur de commande ou un format de sortie qui n'identifie
pas le channel ne vaut jamais `Clean`. Les chemins conflictuels sont rapportés
à l'opérateur ; leur attribution à une PR et la file persistante restent à
implémenter. L'export refuse les conflits et changements non enregistrés.


### Limite beta.24 découverte : channels OID temporaires

La source beta.24 ne conserve pas tous les channels nommés par OID pendant
l'import. `Dag::collect_dead_parents` appelle `drop_channel` quand un parent
n'a plus de descendants à traiter (source `src/commands/git.rs`, autour des
lignes 1245–1264 et 1406–1411). Le cache OID→Merkle, lui, est persistant.

Conséquences :

- Le mapping en ligne (snapshot du parent, import d'un enfant, lecture immédiate
  du channel enfant) évite de demander après coup des channels déjà supprimés.
- Cela ne règle **pas** le cas de plusieurs PR partageant la même base : après
  l'import d'une première branche, le channel OID de la base peut avoir été
  supprimé. Un nouvel appel `pijul git <autre-clone> <même-dépôt-pijul>` peut
  alors retrouver l'OID dans le cache sans retrouver le channel attendu.
- La réexécution d'un import à partir de la base originale pose le même risque.
  Le ledger et les channels nommés `pr-N`/ `integration` ne suffisent pas à
  recréer les channels OID supprimés.

**Décision de sûreté :** ne pas annoncer le pipeline multi-PR comme fonctionnel.
Le prochain spike doit prouver une stratégie de partage de base qui marche avec
la suppression des channels upstream : soit imports isolés puis transfert
des changes vers un dépôt d'intégration, soit un adaptateur sur l'API Rust de
Pijul qui conserve les états nécessaires, soit une modification upstream
explicitement assumée. Le choix doit être validé par un test synthétique
beta.24 avec deux branches sœurs et un rerun, pas seulement par les tests purs.

### Vérification exacte de la signature beta.24 de `pijul git`

La source du paquet exact `pijul 1.0.0-beta.24`, publiée sur docs.rs,
définit bien deux arguments positionnels optionnels :
`git_path` et `pijul_path` (`src/commands/git.rs`, lignes 1068–1075).
Le code ouvre le dépôt Git depuis `git_path`, initialise/ouvre le dépôt
Pijul depuis `pijul_path`, et stocke le cache incrémental dans
`<pijul_path>/.pijul/git/db` (lignes 1104–1140). L'appel à deux chemins de
la passerelle correspond donc bien à l'interface source de beta.24.

La documentation de la branche `main` consultée séparément ne reflète pas
cette signature de la version beta.24 ciblée ; elle ne doit pas servir de
contrat à sa place. **La signature de commande n'est plus le blocage.**
Restent à démontrer à l'exécution : le comportement incrémental lors d'appels
successifs avec des clones Git différents, le mapping SHA→change, les
commandes de channels/pull, puis le round-trip complet. Aucun round-trip ne
doit être revendiqué avant l'exécution du test d'intégration avec beta.24.

Source versionnée : https://docs.rs/crate/pijul/1.0.0-beta.24/source/src/commands/git.rs

### Hypothèse à tester : reconstruire les changes depuis le DAG Git local

La remarque suivante change l'orientation du spike : le miroir Git complet contient
bien les données sources nécessaires pour rejouer l'historique commit par commit
(parent(s), tree, blobs, identités, horodatages et messages). Il n'est donc pas
nécessaire de dépendre de la persistance des channels OID pour **connaître le
graphe Git** ou reconstruire les deltas de fichiers. En revanche, un `git diff`
ne constitue pas à lui seul un `change` Pijul : le hash et les dépendances Pijul
doivent être produits par Pijul, pas inventés depuis le SHA Git.

Hypothèse prioritaire, à prouver sur beta.24 avant de coder davantage :

1. Créer un dépôt Pijul temporaire isolé par ref/branche entrante.
2. Rejouer dans ce dépôt l'histoire Git nécessaire, avec `pijul git`, en
   capturant pour chaque commit l'ensemble de changes avant et après l'import
   de son enfant ; cela évite que les imports d'une autre branche aient déjà
   supprimé les channels OID nécessaires.
3. Vérifier expérimentalement que l'import du même commit et de ses ancêtres
   dans deux dépôts Pijul propres produit les mêmes hashes de changes pour
   chaque commit commun. C'est une hypothèse de déterminisme, pas une garantie
   acquise.
4. Transférer les changes de la branche isolée vers le dépôt partagé par les
   commandes natives Pijul, puis vérifier que les changes communs ne sont pas
   dupliqués, que les branches sœurs restent intégrables et qu'un rerun est
   idempotent.
5. Tester un corpus synthétique comprenant une base, deux commits sœurs, une
   branche plus longue et un rerun complet. Comparer à la fois les hashes de
   changes, les channels et les arbres matérialisés.

Si les hashes communs ne sont pas stables entre imports isolés, ne pas tenter
de les apparier à partir du seul contenu Git : il faudra un adaptateur fondé sur
l'API/crates Pijul, ou une évolution upstream conservant les états nécessaires.
La génération de diffs Git reste utile pour auditer les changements attendus et
comparer les arbres, mais ne remplace pas le format natif Pijul.

**État : piste prioritaire, non exécutée.** Aucun résultat de test ne confirme
encore le déterminisme des hashes entre dépôts isolés ni le transfert des changes.
