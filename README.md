# Passerelle Git ↔ Pijul

Une passerelle qui traduit les concepts Git en concepts Pijul pour résoudre
des fusions qui bloquent côté Git : **récupérer plusieurs PRs Git en conflit,
les traduire en changes Pijul, y résoudre les conflits (automatiquement
quand c'est trivial, par un opérateur quand c'est un arbitrage), puis
renvoyer le résultat dans Git** conformément aux instructions de
l'opérateur.

Pijul devient un moteur de fusion-as-a-service pour des dépôts Git : on
exporte le problème, on résout dans un modèle où le conflit est un *état*
manipulable et non un échec, on réimporte la solution.

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

1. **Ingest** — fetch des branches head des PRs (shallow, API/`git fetch`).
2. **Traduction aller** — `pijul git` : un channel `pr-<n>` par PR, base = branche cible. Le **ledger** enregistre `SHA ↔ change-hash` : idempotence, audit, détection de force-push.
3. **Fusion dans Pijul** — un channel `integration` reçoit les changes de toutes les PRs. Les patchs indépendants **commutent** : une classe entière des « conflits » Git disparaît ici.
4. **Tri** — trois issues seulement :
   - `Clean` — les changes commutent ;
   - `AutoResolved` — règle déterministe (contenu identique, lint, sémantique `union`/`ours`) ; la résolution est un change dédié, étiqueté pour l'audit ;
   - `Operator` — arbitrage métier réel, ou binaire (toujours opérateur).
5. **Opérateur** — voir ci-dessous.
6. **Traduction retour** — le channel `integration` devient une branche Git :
   - `--export-mode linear` : un commit par change, ordre des dépendances, auteurs/dates/messages préservés ;
   - `--export-mode merge-topology` : un merge commit par PR, topologie visible sur GitHub.

## Frontière opérateur (stricte)

Le contrat de la passerelle avec l'extérieur se limite à :

```
la passerelle EXPOSE   -> ConflictReport  (état de conflit Pijul + contexte des PRs)
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

Prouvée sur un corpus de dépôts réels avant toute feature (voir
`tests/properties.rs` pour les invariants purs et le harnas p0 pour le
round-trip complet).

## Usage (forme cible)

```sh
passerelle merge-prs 42 43 44 --base main --export-mode merge-topology
passerelle operator queue
passerelle operator submit --change <hash> --note "choix de l'approche de la PR 42"
```

## Roadmap

- [ ] **P0 — Preuve de round-trip** : importer 3 branches en conflit, fusionner, exporter ; `export(import(x)) == x` vert sur le corpus
- [ ] **P1 — Auto** : règles de tri + auto-résolution des conflits triviaux
- [ ] **P2 — Opérateur** : file persistante + CLI interactif + changes de résolution audités
- [ ] **P3 — Production** : daemon persistant, multi-dépôts, intégration merge queue GitHub

## Licence

MIT — voir [LICENSE](LICENSE).
