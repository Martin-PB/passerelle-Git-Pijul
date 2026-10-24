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

1. **Ingest** — fetch des branches entrantes (shallow, `git fetch` de n'importe quel remote).
2. **Traduction aller** — `pijul git` : un channel par branche entrante (convention `pr-<n>` si numérotée), base = branche cible. Le **ledger** enregistre `SHA ↔ change-hash` : idempotence, audit, détection de force-push.
3. **Fusion dans Pijul** — un channel `integration` reçoit les changes de toutes les branches. Les patchs indépendants **commutent** : une classe entière des « conflits » Git disparaît ici.
4. **Tri** — trois issues seulement :
   - `Clean` — les changes commutent ;
   - `AutoResolved` — règle déterministe (contenu identique, lint, sémantique `union`/`ours`) ; la résolution est un change dédié, étiqueté pour l'audit ;
   - `Operator` — arbitrage métier réel, ou binaire (toujours opérateur).
5. **Opérateur** — voir ci-dessous.
6. **Traduction retour** — le channel `integration` devient une branche Git :
   - `--export-mode linear` : un commit par change, ordre des dépendances, auteurs/dates/messages préservés ;
   - `--export-mode merge-topology` : un merge commit par branche entrante, topologie de fusions lisible sur n'importe quel hébergement Git.

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

Prouvée sur un corpus de dépôts tests avant toute feature (voir
`tests/properties.rs` pour les invariants purs, le harnas p0 dans
`tests/harness/` pour le round-trip complet, et `docs/design-import.md`
pour le design des deux sens, briques existantes comprises).

## Usage (forme cible)

```sh
# Cœur : des refs git, point final.
passerelle merge --head origin/branche-a origin/branche-b --base main --export-mode merge-topology
passerelle operator queue
passerelle operator submit --change <hash> --note "choix de l'approche de la branche a"

# Greffon frontal (exemple futur) : un hébergement traduit sa sémantique en refs.
passerelle github merge-prs 42 43 44 --base main   # -> refs/pull/<n>/head
```

## Roadmap

- [ ] **P0 — Preuve de round-trip** : importer 3 branches en conflit, fusionner, exporter ; `export(import(x)) == x` vert sur le corpus
- [ ] **P1 — Auto** : règles de tri + auto-résolution des conflits triviaux
- [ ] **P2 — Opérateur** : file persistante + CLI interactif + changes de résolution audités
- [ ] **P3 — Production** : daemon persistant, multi-dépôts, greffons d'hébergement (merge queue GitHub, équivalents GitLab/Nest) en frontaux minces du cœur

## Licence

MIT — voir [LICENSE](LICENSE).
