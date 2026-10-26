# Passerelle Git ↔ Pijul

Une passerelle **bidirectionnelle** entre Git et Pijul : importer des dépôts
Git vers Pijul, y résoudre des fusions qui bloquent (automatiquement quand
c'est trivial, par un opérateur quand c'est un arbitrage), puis **exporter
le résultat vers Git** conformément aux instructions de l'opérateur.

Pijul devient un moteur de fusion-as-a-service pour des dépôts Git : on
exporte le problème, on résout dans un modèle où le conflit est un *état*
manipulable et non un échec, on réimporte la solution.

## Périmètre : bidirectionnel, agnostique de l'hébergement

La passerelle connaît **deux mondes, et deux seulement** : un dépôt Git
(Refs, SHAs, objets) et un dépôt Pijul (changes, channels). Les deux sens
sont des citoyens de première classe :

| Sens | Mécanique | Backlog |
|---|---|---|
| Git → Pijul | `pijul git` (import de l'histoire, boîte noire) + ledger | 2.2 |
| Pijul → Git | export `linear` (un commit par change) ou `merge-topology` (topologie de fusions) — audit de `pijul-export` (fast-export + marks) avant d'écrire | 2.3 |

**Aucun hébergement** (GitHub, GitLab, le Nest…) n'est ciblé par le cœur.
La sémantique d'hébergement — PRs, événements, merge queues — relève de
**greffons frontaux** qui la traduisent en simples refs Git à fusionner
(`refs/pull/<n>/head` chez GitHub, l'équivalent ailleurs). Le cœur ne
parle que git. Les mentions de PR dans ce README décrivent la convention
de nommage (`channel pr-<n>`) utilisée quand un greffon fournit des
branches numérotées — le cœur, lui, n'exige rien de tel.

## Dictionnaire des concepts

| Git | Pijul |
|---|---|
| commit | change (patch) |
| branche | channel |
| merge commit | ensemble de changes appliqués |
| rebase | (inutile : commutation) |
| conflit de merge (échec) | état de conflit (first-class) |
| cherry-pick | application d'un change |

## Pipeline

1. **Ingest** — fetch des refs entrantes dans un miroir Git bare, avec historique complet. Le cœur ne suppose pas que les frontières d'un clone shallow soient compatibles avec l'import de l'histoire par `pijul git`.
2. **Traduction aller** — `pijul git` : un channel interne par commit importé ; les channels nommés (`pr-<n>`, `integration`) restent la surface opérateur. Le ledger enregistre `SHA ↔ change-hash` pour l'idempotence et l'audit.
3. **Fusion dans Pijul** — un channel `integration` reçoit les changes des branches. Les patchs indépendants **commutent** : une classe entière des « conflits » Git disparaît ici.
4. **Tri** — trois issues, fondées sur le résultat du modèle natif Pijul :
   - `Clean` — aucun conflit natif Pijul non résolu n'est signalé ;
   - `AutoResolved` — futur traitement déterministe d'un conflit natif Pijul (à implémenter) ; la résolution doit devenir un change dédié et auditable ;
   - `Operator` — conflit natif Pijul encore non résolu après les éventuelles règles automatiques.
   
   Un conflit de fusion Git ne déclenche donc pas, à lui seul, un arbitrage : si les changes commutent dans Pijul, aucun arbitrage n'est requis. La passerelle ne crée pas de catégorie séparée de « conflit métier » et ne déduit pas un conflit à partir de marqueurs textuels Git.
5. **Opérateur** — voir ci-dessous.
6. **Traduction retour** — le channel `integration` devient une branche Git :
   - `--export-mode linear` : un commit par change, ordre des dépendances, auteurs/dates/messages préservés ;
   - `--export-mode merge-topology` : un merge commit par branche entrante, topologie de fusions lisible sur n'importe quel hébergement Git.

## État d'implémentation

- **Ingest Git (première tranche)** : implémentation en cours sur une branche dédiée. Le miroir bare est initialisé explicitement, les refs sont épinglées, et une tête différente du SHA attendu est refusée. Des tests d'intégration sur dépôts synthétiques couvrent ces cas.
- **Traduction Git → Pijul (premier chemin câblé)** : `merge-prs` résout les refs épinglées, importe base et têtes via `pijul git`, vérifie le mapping commit-parent et prépare les channels `pr-<n>` / `integration`. Le premier triage lit `pijul status` : les conflits explicitement signalés sont exposés à l’opérateur ; si beta.24 signale seulement des modifications non enregistrées ou des fichiers non suivis, ces chemins sont aussi envoyés en revue opérateur sans être présentés comme des conflits sémantiques. L’export refuse ces états non propres. Les tests locaux couvrent le triage ciblé et la relance de branches sœurs, mais pas le round-trip complet ; les commits de merge Git sont refusés explicitement.
- **Export Pijul → Git** : un adaptateur expérimental `linear` peut invoquer un binaire externe `pijul-export` (non embarqué), puis vérifie l’ascendance depuis la base et l’égalité de l’arbre Git avec le working copy Pijul. `merge-topology` reste non implémenté. Aucun push distant n’est effectué.
- **P0 round-trip** : non validé. Le test de bout en bout reste ignoré tant que l'import et l'export réels ne sont pas implémentés. Aucun résultat NOT RUN ne doit être présenté comme PASS.

## Frontière opérateur (stricte)

Le contrat de la passerelle avec l'extérieur se limite à :

```
la passerelle EXPOSE   -> ConflictReport  (état de conflit Pijul + contexte des branches)
la passerelle REÇOIT   -> ResolutionChange (un hash de change à appliquer)
```

**Comment** le change de résolution est produit ne regarde pas la passerelle :
un opérateur seul, ou un opérateur assisté par un LLM (par ex. via
[mcp-server-pijul](https://github.com/Martin-PB/MCP-Pijul) branché sur le
dépôt de travail de la passerelle), est **strictement la même opération** —
indiscernable dans le pipeline, le ledger et l'audit. La passerelle ne
connaît, ne tague et ne favorise aucun producteur de résolution.

## Propriété d'acceptation n°1

```
export(import(x)) == x   (sans perte sur le texte)
```

Le harnais synthétique est défini dans `tests/roundtrip.rs` et
`tests/harness/`. Les tests actuellement exécutables vérifient notamment
que les scénarios annoncés conflictuels le sont réellement sous Git et que
l'inventaire de texte est sans perte ; le test de bout en bout reste bloqué
sur l'implémentation des backlogs 2.2 et 2.3.

## Vérification locale Linux

Le protocole reproductible pour tester Rust, compiler la bibliothèque Lean et recueillir les journaux est documenté dans [`docs/local-verification.md`](docs/local-verification.md). Le protocole d'accès concurrent au miroir et au workspace est décrit dans [`docs/concurrency.md`](docs/concurrency.md).

L'installation de Lean sur Ubuntu (elan + toolchain épinglé) et le plan d'avancement de la spécification sont décrits dans [`docs/lean-setup.md`](docs/lean-setup.md) :

```sh
bash scripts/setup-lean.sh
```

Depuis la racine du dépôt :

```sh
bash scripts/verify-local.sh
```

Le script n'installe pas les outils et ne déclenche aucune GitHub Action. Une exécution incomplète retourne un code non nul. Le build Lean valide uniquement les théorèmes du modèle abstrait ; la preuve de raffinement vers le moteur Pijul réel et le round-trip de bout en bout restent des objectifs non démontrés.

## Usage

L’ingest générique prend un remote et des mappings source=destination, sans dépendre
d’un hébergeur particulier. Pour utiliser ensuite `merge-prs`, épingle la base
sous `refs/remotes/base/<nom>` et les têtes sous `refs/remotes/pr/<numéro>` :

```sh
passerelle ingest --remote ../repo-source --workdir .gateway-mirror \\
  --ref refs/heads/main=refs/remotes/base/main \\
  --ref refs/heads/pr-42=refs/remotes/pr/42 \\
  --ref refs/heads/pr-43=refs/remotes/pr/43
```

Le pipeline d'import est partiellement câblé. `operator queue` lit les conflits
du channel `integration` ; `operator submit` exige le chemin exact d'un conflit
affiché par cette commande, vérifie qu'il est encore présent, applique le change,
puis relance le triage. La commande ne réussit que si le triage confirme que ce
chemin n'est plus en conflit ; sinon, elle renvoie une erreur et journalise
l'échec ou le conflit persistant dans `gateway-audit.jsonl`. Cette vérification
n'est pas encore une file persistante : les conflits sont relus depuis Pijul.
L'attribution des conflits aux PRs reste à faire. Le mode `linear` peut tenter
un export local si `pijul-export` est installé séparément ; `merge-topology`
reste incomplet :

```sh
passerelle merge-prs 42 43 --base main --mirror .gateway-mirror --workdir .gateway-work --export-mode merge-topology
passerelle operator queue
passerelle operator submit --path src/lib.rs --change <hash> --note "choix de l'approche de la branche a"

# Greffon frontal (exemple futur) : un hébergement traduit sa sémantique en refs.
passerelle github merge-prs 42 43 44 --base main   # -> refs/pull/<n>/head
```

## Reprise après interruption d'un import

Avant chaque appel mutateur à `pijul git`, la passerelle écrit une intention
durable dans `.bridge/import-intents/<git-sha>.json`. Après avoir identifié
le change Pijul, elle enregistre d'abord ce hash dans l'intention, sauvegarde
ensuite le mapping dans le ledger, puis retire l'intention. Une interruption
après la sauvegarde du ledger permet ainsi de comparer le hash exact avant
d'effacer le journal.

Si le processus s'arrête après la mutation Pijul mais avant la sauvegarde du
mapping, l'intention reste présente. Une nouvelle tentative **s'arrête sans
rejouer l'import** et demande une récupération explicite : examiner le channel
Pijul et le ledger, établir le mapping sans ambiguïté, puis suivre le runbook
[`docs/import-recovery.md`](docs/import-recovery.md). Celui-ci décrit la
préservation et le triage, mais il ne fournit pas encore de commande de réparation
du ledger. Ne pas supprimer manuellement l'intention ni inventer un mapping
sans preuve suffisante. Cette version n'automatise pas cette réparation et ne
prétend pas pouvoir retrouver un channel OID que Pijul aurait supprimé.

## Roadmap

- [ ] **P0 — Preuve de round-trip** : importer 3 branches en conflit, fusionner, exporter ; `export(import(x))` vert sur le corpus synthétique
- [ ] **P1 — Auto** : règles de tri + auto-résolution des conflits triviaux
- [ ] **P2 — Opérateur** : file persistante + CLI interactif + changes de résolution audités
- [ ] **P3 — Production** : daemon persistant, multi-dépôts, greffons d'hébergement (merge queue GitHub, équivalents GitLab/Nest) en frontaux minces du cœur

## Licence

MIT — voir [LICENSE](LICENSE).
