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
4. **Tri** — trois issues seulement :
   - `Clean` — les changes commutent ;
   - `AutoResolved` — règle déterministe (contenu identique, lint, sémantique `union`/`ours`) ; la résolution est un change dédié, étiqueté pour l'audit ;
   - `Operator` — arbitrage métier réel, ou binaire (toujours opérateur).
5. **Opérateur** — voir ci-dessous.
6. **Traduction retour** — le channel `integration` devient une branche Git :
   - `--export-mode linear` : un commit par change, ordre des dépendances, auteurs/dates/messages préservés ;
   - `--export-mode merge-topology` : un merge commit par branche entrante, topologie de fusions lisible sur n'importe quel hébergement Git.

## État d'implémentation

- **Ingest Git (première tranche)** : implémentation en cours sur une branche dédiée. Le miroir bare est initialisé explicitement, les refs sont épinglées, et une tête différente du SHA attendu est refusée. Des tests d'intégration sur dépôts synthétiques couvrent ces cas.
- **Traduction Git → Pijul (premier chemin câblé)** : `merge-prs` résout les refs épinglées, importe base et têtes via `pijul git`, vérifie le mapping commit-parent et prépare les channels `pr-<n>` / `integration`. Ce chemin n’est pas encore validé à l’exécution et les commits de merge Git sont refusés explicitement.
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

Les commandes suivantes décrivent l'interface cible ; le pipeline merge et
l'opérateur persistant ne sont pas encore implémentés :

```sh
passerelle merge-prs 42 43 --base main --mirror .gateway-mirror --workdir .gateway-work --export-mode merge-topology
passerelle operator queue
passerelle operator submit --change <hash> --note "choix de l'approche de la branche a"

# Greffon frontal (exemple futur) : un hébergement traduit sa sémantique en refs.
passerelle github merge-prs 42 43 44 --base main   # -> refs/pull/<n>/head
```

## Roadmap

- [ ] **P0 — Preuve de round-trip** : importer 3 branches en conflit, fusionner, exporter ; `export(import(x))` vert sur le corpus synthétique
- [ ] **P1 — Auto** : règles de tri + auto-résolution des conflits triviaux
- [ ] **P2 — Opérateur** : file persistante + CLI interactif + changes de résolution audités
- [ ] **P3 — Production** : daemon persistant, multi-dépôts, greffons d'hébergement (merge queue GitHub, équivalents GitLab/Nest) en frontaux minces du cœur

## Licence

MIT — voir [LICENSE](LICENSE).
