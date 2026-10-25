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

### Approche alternative à tester en premier : un seul DAG Git synthétique

L'analyse du défaut de beta.24 suggère une approche plus simple que N imports
isolés : **ne plus appeler `pijul git` séparément pour chaque branche sœur**.
Construire dans le miroir temporaire un commit Git synthétique dont les parents
sont les têtes épinglées des branches à importer, puis appeler `pijul git`
une seule fois sur ce DAG complet.

Pourquoi cela pourrait marcher : l'importateur upstream parcourt le DAG depuis
HEAD et compte les descendants encore à traiter avant de supprimer les channels
OID parents. Dans un DAG unique, la base commune a plusieurs chemins descendants
et ne devrait être libérée qu'après le traitement de toutes les branches qui en
dépendent. Cela évite le scénario fautif « importer branche A, supprimer le
channel de base, puis importer branche B depuis le cache ». Cette explication
est une déduction à partir du code beta.24, pas encore une preuve expérimentale.

Protocole de validation :
1. Construire un dépôt Git synthétique avec une base B et deux branches sœurs
   A et C, chacune ajoutant au moins un commit indépendant.
2. Créer un commit temporaire M dont les parents sont A et C, en fixant
   explicitement son arbre à celui de A. Le commit est un artifice de parcours,
   il ne doit pas être exporté vers le dépôt cible.
3. Lancer une seule fois `pijul git` sur M ; vérifier que les changes propres
   à A et C sont tous présents, et que le channel OID de B n'est pas supprimé
   avant que ses deux branches aient été traitées.
4. Vérifier le comportement du commit synthétique sans différence d'arbre :
   si l'importateur le refuse ou crée un change artificiel, tester une racine
   synthétique équivalente ou un importeur modifié, et surtout ne jamais exposer
   M dans les refs exportées.
5. Comparer les arbres matérialisés des channels A et C, vérifier le mapping
   Git-SHA→change en capturant les deltas pendant le parcours si possible, puis
   relancer l'import sur le même DAG et vérifier l'idempotence.
6. Étendre à trois branches, une branche dépendante (C basée sur A), un merge
   Git réel et un rerun partiel.

Limites : le CLI ne fournit pas de hook documenté pour capturer chaque change
pendant le parcours interne. Il peut donc résoudre le partage de base sans
résoudre à lui seul le mapping exact par commit. Si ce mapping est indispensable
pour l'export, l'étape suivante est un petit fork/patch de `pijul git` qui
émet un manifeste JSON `Git OID → change hash → parent OIDs` pendant l'import,
ou une API interne basée sur les crates Pijul. C'est plus ciblé qu'une
réimplémentation du format des changes.

### Recherche théorique utile

La documentation théorique officielle de Pijul décrit les changes comme des
modifications d'un graphe de lignes et de relations, avec identité liée au
change qui introduit chaque ligne ; elle explique pourquoi un diff de texte ne
suffit pas à fabriquer un change Pijul valide
(https://pijul.org/manual/theory). La présentation générale de la théorie des
patches et de la commutation confirme que l'objet pertinent est le change avec
ses dépendances, pas seulement un instantané de fichiers
(https://pijul.org/manual/why_pijul.html). Pour le contexte de recherche plus
large, *Current Concepts in Version Control Systems* (Petr Baudiš, arXiv:1405.3496)
passe en revue les architectures des VCS et leurs compromis
(https://arxiv.org/abs/1405.3496). Ces sources motivent de conserver Pijul comme
autorité pour produire les changes ; elles ne démontrent pas le comportement
précis de beta.24, qui reste à tester.


## 11. Nouvelle exigence d'architecture : correction formelle, pas seulement tests (24/10/2026)

Les tests synthétiques peuvent réfuter une conception et détecter des régressions ; ils ne démontrent pas la correction générale de la traduction. La passerelle doit donc définir une sémantique précise et établir une preuve de raffinement entre l'implémentation et cette sémantique. Aucun test réussi ne doit être présenté comme preuve mathématique.

### Outil recommandé : Lean 4 pour le modèle et les preuves

Lean est retenu comme premier candidat parce qu'il combine un noyau de vérification des preuves et un langage fonctionnel utilisable pour écrire un cœur exécutable. Cette décision ne signifie pas que l'usage de Lean suffit à rendre le système correct : la spécification, les hypothèses, la relation au code exécuté et la frontière de confiance doivent elles-mêmes être explicites.

Références :
- Manuel officiel Lean : https://lean-lang.org/doc/reference/latest/
- FAQ Lean sur le noyau, les preuves et la base de confiance : https://lean-lang.org/faq/
- Aeneas, vérification du Rust par traduction fonctionnelle (ICFP 2022) : https://doi.org/10.1145/3547647
- Documentation hax : https://hax.cryspen.com/manual/

Aeneas/hax sont des candidats pour relier le Rust réel à des obligations de preuve, mais leur sous-ensemble pris en charge et la fidélité de l'extraction doivent être audités sur ce crate. Une extraction réussie n'est pas, à elle seule, la preuve de nos propriétés métier.

### Modèle formel à construire

Le modèle doit être indépendant des détails du CLI et définir au minimum :

1. **DAG Git** : commits, arbres, blobs, parents multiples, ancêtres, branches et commits inaccessibles.
2. **Sémantique Pijul abstraite** : changements, identité, dépendances, contextes et conflits ; ne pas assimiler un change à un diff textuel.
3. **Relation de traduction** : quelles représentations Pijul sont admissibles pour un historique Git donné, et quelle information doit être préservée.
4. **État du protocole** : refs épinglées, ledger, channels, imports partiels, erreurs, reruns et publication de refs.
5. **Frontière d'observation** : ce que le programme peut réellement lire du CLI/API Pijul et les garanties exigées de cette interface.

### Théorèmes cibles

- **Sûreté de graphe** : toute dépendance produite est justifiée par la relation source spécifiée ; aucun parent ou ancêtre nécessaire n'est perdu.
- **Fidélité de traduction** : le résultat satisfait la relation de raffinement définie entre l'historique Git et les changes Pijul ; la simple égalité des arbres n'est pas suffisante.
- **Préservation des conflits** : les conflits du modèle source ne sont ni supprimés ni résolus implicitement par la passerelle.
- **Correction du ledger** : un OID ne peut être associé à deux changes incompatibles ; toute correspondance publiée est justifiée.
- **Idempotence** : réexécuter une opération valide ne modifie pas le résultat abstrait, hormis les métadonnées explicitement exclues du contrat.
- **Publication sûre** : une ref de sortie n'est publiée que si tous les prérequis formels sont satisfaits ; une erreur externe ne peut être assimilée à un résultat propre ou réussi.

Ces théorèmes devront être formulés précisément avant de choisir l'algorithme. Ils ne peuvent pas reposer sur des axiomes qui réaffirment simplement les propriétés à démontrer.

### Point critique : l'importateur Pijul ne peut pas rester une boîte noire sans contrat

Si la passerelle appelle `pijul git` et suppose que celui-ci produit les bons changes, Lean ne prouvera que la correction de l'orchestration **conditionnellement** à cette hypothèse. Pour une preuve de bout en bout, il faudra soit :

- formaliser la sémantique pertinente de l'importateur et prouver sa conformité ;
- remplacer le chemin critique par un cœur dont l'implémentation et la spécification peuvent être reliées par une preuve ;
- ou expliciter et réduire la frontière de confiance à une API Pijul étroite, dont le contrat est lui-même vérifié et dont les hypothèses ne sont pas présentées comme des théorèmes.

Réimplémenter à la main le format interne des changes à partir de diffs Git serait une mauvaise réponse : cela déplace le problème sans fournir de preuve.

### Plan de travail

1. Écrire une spécification Lean minimale de DAG Git, de changes abstraits et de relation de traduction.
2. Prouver les invariants de graphe, de ledger, d'idempotence et de publication avant de choisir entre DAG synthétique, imports isolés ou adaptation upstream.
3. Formaliser explicitement la propriété de fidélité aux changements et conflits Pijul, et identifier quelles définitions peuvent être dérivées du modèle upstream plutôt que réinventées.
4. Étudier si Aeneas ou hax peut extraire les fonctions Rust pures de la passerelle et établir la correspondance entre code exécuté et fonctions spécifiées.
5. Auditer séparément les frontières Git/Pijul/OS : résultats de commandes, erreurs, données externes et version exacte de Pijul. Les appels externes non vérifiés restent des hypothèses.
6. Garder les tests comme contre-exemples et vérifications d'intégration, jamais comme substituts aux preuves.

**Statut : proposition d'architecture formelle ; aucune preuve n'a encore été écrite ou vérifiée.** Le blocage multi-branches ne doit pas être déclaré résolu tant que le modèle, les obligations de preuve et la relation à l'implémentation ne sont pas établis.


### Première spécification Lean ajoutée (24/10/2026)

Fichier : `formal/lean/Bridge/Spec.lean`.

Cette première tranche formalise :
- un modèle de commit Git et d'arbre, avec parents explicites ;
- une histoire dont les parents existent et ont un rang strictement inférieur, ce qui exclut les cycles ;
- la relation d'ascendance et sa transitivité ;
- des identifiants de changes distincts des OID Git ;
- un invariant de fonctionnalité du ledger (un OID ne peut pas pointer vers deux changes distincts) ;
- le contrat fail-closed d'un enregistrement importé (un état « imported » doit avoir un mapping).

La sémantique des changes Pijul n'est volontairement pas déclarée prouvée : elle est identifiée comme une frontière à formaliser. Les théorèmes de fidélité, préservation des conflits, dépendances et idempotence sont listés comme obligations, et non introduits comme axiomes. La prochaine tranche doit définir une relation de raffinement explicite entre historique Git et modèle de patchs Pijul.

**Validation :** Lean/Lake ne sont pas installés dans l'environnement courant. Le fichier n'a donc pas été compilé ni vérifié par Lean ; aucun statut de preuve n'est revendiqué. Il faudra ajouter une configuration Lake reproductible, compiler le modèle, puis remplacer les commentaires d'obligations par des définitions et théorèmes vérifiés.

### Deuxième tranche Lean : contrat d'acceptation P0 (24/10/2026)

`formal/lean/Bridge/Spec.lean` définit maintenant explicitement :

- `TreeEq` : égalité extensionnelle des arbres matérialisés ;
- `P0Observation` et `P0Equivalent` : le critère P0 porte sur l'équivalence du contenu de chaque commit importé **et** sur l'équivalence des observations de conflits entre paires de commits importés ;
- `GitTopologyEquivalent` : la préservation exacte des arêtes parentales Git est un objectif séparé, pas une exigence implicite de P0 ;
- `parent_edge_rank_lt` et `ancestor_rank_le` : le rang certifie formellement la monotonie sur les arêtes et les chemins d'ascendance.

Ces définitions rendent le critère d'acceptation explicite, mais **ne prouvent pas** que le programme Rust ou `pijul git` le satisfait. Les relations d'observation de conflits restent abstraites jusqu'à ce qu'elles soient reliées à une sémantique Pijul concrète et à des observations définies de l'interface. Aucun axiome ne doit être ajouté pour postuler cette équivalence.

**Validation :** cette mise à jour n'a pas été compilée par Lean : aucun exécutable Lean/Lake n'est disponible dans l'environnement de travail. Les preuves écrites restent donc non vérifiées jusqu'à une compilation réelle. Aucun workflow GitHub Actions n'a été lancé.

### Configuration Lean/Lake reproductible (24/10/2026)

Ajout des fichiers racine `lean-toolchain` et `lakefile.lean` :

- version Lean épinglée : `leanprover/lean4:v4.19.0` ;
- bibliothèque Lake `Bridge`, dont la racine source est `formal/lean` ;
- module principal : `Bridge.Spec` (`formal/lean/Bridge/Spec.lean`).

Depuis la racine du dépôt, après installation de Lean via elan :

```sh
lake --version
lean --version
lake build Bridge
```

Le pin est volontairement explicite pour rendre les résultats reproductibles ; il pourra être actualisé dans une modification dédiée après validation de la compilation. La présence de cette configuration ne signifie pas que le build est déjà passé : il n'a pas été exécuté dans cet environnement.

**Limite de méthode à conserver :** `P0Equivalent` est actuellement un contrat d'observation, pas encore une relation de raffinement démontrée. En particulier, `gitConflict` et `pijulConflict` sont des prédicats fournis par l'observateur : tant qu'on ne définit pas comment ils sont dérivés des états et de l'interface réelle, l'équivalence ne prouve rien sur le comportement concret du pont. La prochaine étape doit spécifier ces observations et les hypothèses de confiance, puis seulement formuler un théorème reliant l'exécution au contrat.

Les lois de `TreeEq` sont aussi énoncées comme théorèmes (`treeEq_refl`, `treeEq_symm`, `treeEq_trans`) plutôt que supposées. Elles sont simples mais établissent les bases algébriques de la relation d'égalité des arbres. Elles restent à confirmer par `lake build Bridge`.
Un lemme supplémentaire, `ancestor_rank_lt_of_ne`, établit que deux commits distincts reliés par l'ascendance ont un rang strictement croissant. Il renforce la propriété de fondation du modèle de DAG ; il ne prouve pas à lui seul que le graphe observé depuis Git correspond au graphe réellement importé par Pijul.
### Précision sémantique sur les conflits P0

Le modèle Lean précise désormais que Git ne stocke pas un prédicat de conflit intrinsèque sur chaque commit. `gitConflict` doit représenter le résultat d'une tentative de merge explicitement définie par le harnais ; `pijulConflict` doit représenter le résultat de l'opération Pijul correspondante. L'équivalence de ces observations ne devient un contrat testable qu'une fois cette correspondance d'opérations définie. Il serait incorrect d'interpréter le prédicat abstrait comme une preuve générale d'équivalence des notions de conflit des deux VCS.

## 12. Progrès de la preuve Lean — composition locale vers globale

La spécification `formal/lean/Bridge/Spec.lean` contient maintenant trois obligations prouvées dans le modèle :

- `GitHistory.ancestry_antisymmetric` : le rang strict interdit qu'un commit distinct soit ancêtre l'un de l'autre dans les deux sens ;
- `singleton_delta_unique` et `singleton_delta_same_difference` : une observation de différence d'ensembles satisfaisant le contrat singleton identifie un seul change, et deux observations avec mêmes ensembles avant/après ne peuvent produire deux mappings différents ;
- `tree_refinement_along_ancestry` : si l'état de base respecte la relation d'arbres et si chaque étape parent→enfant préserve localement cette relation, alors elle est préservée le long du chemin d'ascendance.

Ces théorèmes ne sont pas des axiomes de correction du binaire. En particulier, `TreeRefinement.edgePreserves` est une obligation que le traducteur concret devra démontrer ou justifier par un contrat de raffinement vérifié. Le théorème de composition montre comment une preuve locale peut se propager, mais ne prouve pas encore que `pijul git` satisfait cette obligation. Les listes avant/après du mapping doivent aussi être canoniques et représenter des ensembles ; cette précondition reste explicitement à garantir par l'implémentation.

Le protocole de test et de preuve local est consigné dans [`docs/local-verification.md`](local-verification.md), exécuté par [`scripts/verify-local.sh`](../scripts/verify-local.sh). Tant que le build n'a pas été lancé sur Linux, ces nouveaux théorèmes sont **écrits mais non validés par Lean**.

### Formalisation Lean — idempotence locale du ledger

Commit : [2151578](https://github.com/Martin-PB/passerelle-Git-Pijul/commit/215157846d541c6a16aff89f212271c0a683607a).

La spécification ajoute `Ledger.recordIfAbsent` et le théorème
`Ledger.recordIfAbsent_idempotent` : enregistrer deux fois la même paire
`(Git OID, change Pijul)` ne duplique pas cette entrée dans le ledger abstrait.

**Portée limitée :** cela prouve uniquement la suppression des doublons exacts
dans le modèle de données. Cela ne prouve ni l'idempotence de la commande
`pijul git`, ni le comportement d'une nouvelle importation, ni le rejet
d'une association contradictoire où un même OID serait lié à un autre change.
La fonctionnalité reste à compiler avec `lake build Bridge`; aucune preuve
Lean n'est considérée vérifiée avant cette compilation.

### Test d'intégration ajouté — branches sœurs puis rerun

Le test synthétique `sibling_branches_then_rerun_preserves_base_and_mapping` a été ajouté dans `tests/roundtrip.rs`. Il importe deux branches sœurs dans le même dépôt Pijul, relance ensuite l'import de la première et vérifie que les mappings du ledger existent encore avec la bonne attribution.

Le test est `#[ignore]` par défaut car il requiert le vrai binaire Pijul. Le protocole local le déclenche explicitement avec `RUN_SIBLING_RERUN_TEST=1 bash scripts/verify-local.sh`. Le script a été adapté pour exécuter les tests ignorés correspondants, et `docs/local-verification.md` décrit maintenant ce scénario comme implémenté mais non exécuté.

Ce test n'affirme pas que l'approche actuelle réussit : il peut révéler le blocage déjà anticipé sur le channel de base commun. Aucun résultat ne sera revendiqué avant son exécution avec Pijul beta.24.

### Formalisation Lean — écriture du ledger en échec fermé

Commit : [20dfdbc](https://github.com/Martin-PB/passerelle-Git-Pijul/commit/20dfdbc615da48757e3d9fc92408b53ae4585de8).

La spécification ajoute `Ledger.recordChecked`, qui renvoie `none` lorsqu'un OID existe déjà avec un change différent, ainsi que les théorèmes `Ledger.recordChecked_rejects_conflict` et `Ledger.recordChecked_accepts_consistent`. Le contrat évite qu'une association contradictoire soit ajoutée silencieusement au ledger abstrait.

Cette preuve reste une propriété du modèle Lean. Le ledger Rust actuel possède sa propre logique `record` ; le lien entre cette implémentation et le modèle n'est pas encore prouvé, et l'exactitude du change fourni par l'importateur reste une obligation distincte. Build Lean non exécuté.

## 12. Contrat de plateforme Git ↔ Pijul : définition métier et périmètre (25/10/2026)

Cette section affine l'objectif au niveau de la plateforme, au-delà du critère
P0 de laboratoire. Elle constitue le contrat de conception à formaliser ; elle
ne prétend pas que l'implémentation actuelle le satisfait.

### 12.1 Cas d'utilisation

Deux groupes de développeurs peuvent travailler en parallèle sur le même
logiciel : l'un dans Git, l'autre dans Pijul. La plateforme doit permettre
d'utiliser les outils propres à chacun des écosystèmes (notamment PR et
GitHub Actions côté Git) sans obliger les deux dépôts à partager le même
historique natif.

Chaque dépôt conserve un historique autonome. La duplication structurelle
des modifications entre dépôts est intentionnelle : il doit rester possible
de retirer la passerelle et de continuer le développement avec Git seul ou
Pijul seul. La garantie recherchée est donc la fidélité et la traçabilité des
correspondances, pas l'absence de copies de changements.

### 12.2 Amorçage sur un dépôt Git existant

Pijul doit pouvoir être ajouté à un dépôt Git ancien sans demander une
migration préalable de son historique, une instrumentation historique, ni des
métadonnées qui n'existent pas déjà. L'amorçage de la passerelle ne doit
s'appuyer que sur les informations que les interfaces/commandes Git permettent
d'exposer : commits et parents, arbres et contenus, refs et autres métadonnées
effectivement disponibles par ces interfaces.

Le contrat ne doit pas présupposer que les développeurs ont utilisé la
passerelle dès le premier commit. Les informations que Git ne permet pas
d'exposer ne peuvent pas être requises comme précondition silencieuse. Toute
limite de reconstruction doit être documentée et échouer explicitement, plutôt
que d'inventer une provenance ou une correspondance.

### 12.3 Équivalence de plateforme, pas identité des dépôts

On cherche une relation de cohérence entre l'état Git, l'état Pijul et le
registre de correspondance. Elle doit préserver au moins :

- **Fidélité du contenu** aux points de synchronisation définis : les arbres
  matérialisés représentent les mêmes chemins, types d'entrées, contenus et
  propriétés observables pertinentes ; comparer directement des identifiants
  internes Git et Pijul n'est pas une définition sémantique suffisante.
- **Fidélité de provenance** : une modification synchronisée garde une
  correspondance vérifiable avec son origine ; les identités et les relations
  d'historique propres à chaque système ne sont pas confondues.
- **Autonomie** : chaque dépôt reste un dépôt utilisable indépendamment. Les
  historiques peuvent différer et les changements peuvent avoir des
  représentations distinctes dans chaque système.
- **Convergence contrôlée** : les changements compatibles peuvent être
  synchronisés sans perte ; répéter l'opération ne crée pas de nouvelles
  copies accidentelles ni de correspondances contradictoires. Cela n'interdit
  pas les copies intentionnelles dans les deux historiques.
- **Traçabilité et sûreté** : les mappings sont justifiés, les imports
  ambigus échouent fermés, et les refs de sortie ne sont publiées qu'après
  validation des invariants exigés.
- **Compatibilité des écosystèmes** : la passerelle ne doit pas invalider les
  mécanismes Git attendus par les outils de l'écosystème Git, ni empêcher le
  travail natif côté Pijul.

Cette relation n'impose ni l'égalité des identifiants de commits et de changes,
ni l'identité des graphes d'historique. La préservation exacte de la topologie
Git doit rester une garantie distincte, explicitement activée et prouvée si
elle est nécessaire à un mode donné.

### 12.4 Périmètre de la gestion des conflits

Le périmètre est la gestion de configuration. La détection de conflits
sémantiques métier que ni le modèle de modifications ni les outils de gestion
de versions ne détectent est **hors périmètre** ; la plateforme ne promet pas
de détecter une incompatibilité comportementale silencieuse.

Le rôle spécifique de Pijul est de traiter les conflits natifs que son modèle
de changes, de dépendances et de commutation fait apparaître. Le contrat
d'arbitrage retenu est :

**A — seuls les conflits natifs Pijul déclenchent un arbitrage.** Toute
situation à arbitrer doit être représentée comme un conflit résultant d'une
opération Pijul définie (application/commutation selon la sémantique exacte
de la version ciblée). La passerelle ne doit ni substituer à ce signal une
heuristique métier externe, ni qualifier de « conflit métier » un cas que le
modèle Pijul ne fait pas apparaître. Un conflit Git purement mécanique peut
être résolu par la voie Pijul uniquement si le résultat est établi par le
contrat de traduction et les opérations Pijul ; il ne faut pas supposer que
tout conflit Git disparaîtra automatiquement.

Les observations de conflits doivent être associées à des scénarios
d'opérations appariés, pas à une propriété intrinsèque d'un commit. La
relation entre un conflit Git de fusion et un conflit natif Pijul ne peut pas
être une simple égalité booléenne universelle : il faut préciser quelle
opération Pijul représente la fusion des changements concurrents, et quelle
garantie est attendue lorsque Pijul ne signale pas de conflit.

### 12.5 Ce que cela change pour la preuve

Le prédicat Lean actuel `P0Equivalent` reste un critère provisoire pour un
corpus synthétique ; il ne formalise pas à lui seul l'équivalence de cette
plateforme. Le prochain modèle formel doit distinguer :

1. les deux états/historiques natifs et leurs arbres observables ;
2. les correspondances de provenance entre commits Git et changes Pijul,
   sans imposer une bijection naïve entre les deux historiques ;
3. les points de synchronisation et les opérations qui les relient ;
4. les conflits natifs Pijul produits par les scénarios de concurrence ;
5. les propriétés d'autonomie, d'idempotence, de convergence et de publication
   sûre ;
6. les données effectivement disponibles par les commandes Git lors de
   l'amorçage d'un dépôt existant.

La preuve finale devra établir une simulation conditionnelle à des hypothèses
explicites sur la version et la sémantique de Pijul utilisée. Tant que le
cœur d'import/export Pijul reste une boîte noire non vérifiée, la preuve de la
passerelle ne peut pas prétendre démontrer ces hypothèses elle-même.

**Validation :** cette section formalise l'intention et les obligations à
prouver ; elle n'est ni une preuve Lean compilée, ni une validation du
comportement de Pijul. Aucun test ni workflow Actions n'a été exécuté pour
cette mise à jour.

## 13. Définitions formelles de l'équivalence de plateforme (25/10/2026)

Les définitions Lean correspondantes sont ajoutées à la section 7 et suivantes de
`formal/lean/Bridge/Spec.lean`. Elles constituent une **couche de définition**,
pas une preuve que le programme actuel satisfait le contrat.

### 13.1 État observable

Un arbre observable associe chaque chemin canonique à une entrée absente ou à
l'une des entrées suivantes : fichier (octets + bit exécutable), lien symbolique
(cible en octets), gitlink/submodule (commit référencé). Les identifiants de
blob Git ne sont donc pas comparés directement aux identifiants internes Pijul.
Le traitement des chemins, permissions et métadonnées doit être fixé par
l'adaptateur concret ; les entrées non prises en charge doivent faire l'objet
d'une précondition ou d'un rejet explicite.

### 13.2 État de plateforme et provenance

L'état abstrait contient une vue Git, une vue Pijul et un registre de provenance.
La provenance est une **relation** commit Git ↔ change Pijul, et non une
bijection imposée par définition. Le contrat distingue :
- la validité de chaque lien (commit et change existants) ;
- la couverture des commits dont la traduction exige un change ;
- la traçabilité des changes créés par la passerelle ;
- la fermeture des dépendances Pijul dans l'état sélectionné.

Les commits vides ou purement métadonnées doivent être traités explicitement :
ils ne doivent pas provoquer l'invention d'un change artificiel. Si leur
topologie ou leurs métadonnées doivent être préservées, cela relève d'un
contrat complémentaire.

### 13.3 Cohérence et arbitrage

Deux états sont distingués :

- **Point de synchronisation cohérent** : arbres observables équivalents,
  mappings valides et suffisamment couverts, changes créés par la passerelle
  traçables, dépendances fermées, aucun conflit natif Pijul en attente.
- **Arbitrage en attente** : au moins un conflit natif Pijul est exposé ; les
  invariants de provenance et de dépendances restent valides, mais aucune
  équivalence finale des arbres n'est revendiquée avant résolution.

La décision d'arbitrage dépend exclusivement des conflits natifs rapportés par
l'opération Pijul ciblée. Le résultat d'une fusion Git et celui de l'opération
Pijul appariée sont enregistrés séparément : aucun axiome ne suppose qu'ils
signalent les mêmes conflits. Un conflit Git peut être considéré comme traité
sans arbitrage uniquement si l'opération Pijul correspondante ne rapporte pas
de conflit natif. Cette définition ne prouve pas que cette opération existe
toujours, ni que Pijul élimine tous les conflits de fusion Git.

### 13.4 Autonomie et synchronisation

L'autonomie signifie que chaque dépôt peut continuer à évoluer selon ses
opérations natives ; elle **n'impose pas** que les deux arbres restent égaux
après chaque modification unilatérale. La divergence est permise entre deux
points de synchronisation. L'idempotence est définie au niveau des observations
(arbre Git, arbre Pijul, liens de provenance et conflits) : répéter une
synchronisation ne doit pas modifier ces observations, même si les représentations
internes natives ne sont pas identiques.

L'amorçage sur un dépôt Git ancien ne peut utiliser que l'historique, les refs,
les arbres et les autres informations réellement exposées par les interfaces
Git. La passerelle doit rejeter ou signaler explicitement les cas qu'elle ne
peut pas traduire ; elle ne peut pas exiger des métadonnées historiques
indisponibles ni inventer de provenance.

### 13.5 Obligations encore à démontrer

Les prédicats Lean rendent les obligations explicites, mais ne constituent pas
les théorèmes de correction eux-mêmes. Il reste à prouver la simulation des
opérations concrètes, la fidélité de traduction, la préservation des
dépendances et des conflits natifs, l'idempotence, la publication sûre et
l'autonomie après synchronisation. Tant que la sémantique Pijul utilisée et
son implémentation ne sont pas reliées au modèle, la preuve reste conditionnelle
aux hypothèses de cette frontière.

**Validation au 25/10/2026 :** les définitions Lean ont été écrites mais ne sont
pas compilées dans cette session. Aucun test Rust/Pijul ni workflow Actions n'a
été exécuté. Ne pas présenter ce commit comme une preuve validée.
