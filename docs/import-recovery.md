# Récupération d'un import interrompu

## Objectif et limites

Le journal d'intention détecte une interruption entre la mutation du dépôt Pijul et la sauvegarde du mapping dans `gateway-ledger.json`. Il évite de rejouer aveuglément un import, mais **ne répare pas automatiquement** un mapping manquant.

Cette procédure sert à préserver l'état et à déterminer si la reprise normale est sûre. Elle ne constitue pas une commande de réparation du ledger.

**Concurrence :** tant qu'un verrou global de workspace n'est pas implémenté, une seule instance de la passerelle doit importer ou modifier les channels dans un même répertoire de travail à la fois. `create_new` ne sérialise que deux tentatives pour le même SHA ; il ne protège pas deux imports de SHA différents qui partagent le dépôt Pijul et le ledger.

## Avant toute inspection

1. Arrêter la passerelle et empêcher toute autre instance de travailler sur le même répertoire.
2. Faire une copie complète du répertoire de travail de la passerelle, y compris le miroir Git, le dépôt Pijul, `gateway-ledger.json`, `.bridge/import-intents/` et les journaux d'audit. Ne pas travailler sur l'unique copie.
3. Noter les versions de Git, de la passerelle et de Pijul. Le comportement d'import ciblé par le projet est celui de Pijul `1.0.0-beta.24`.
4. Ne pas supprimer, renommer ni modifier manuellement le fichier d'intention. Ne pas lancer de nouvelle importation avant d'avoir classé le cas.

## Identifier l'intention

Pour le commit Git `<SHA>`, le fichier attendu est :

```text
.bridge/import-intents/<SHA>.json
```

Il contient l'identité du commit Git, son parent, le SHA de base, le numéro de PR fourni au pont et, si l'observation du delta a abouti, le hash du change Pijul (`pijul_change`). Le ledger est dans `gateway-ledger.json` à la racine du répertoire de travail.

Comparer les identités avec le commit Git épinglé, le parent réel, la base utilisée et le contexte d'import. Toute contradiction, erreur de lecture ou JSON invalide impose un arrêt : ne pas tenter de « corriger » les valeurs à la main.

## Décision de reprise

### Cas A — Le ledger contient déjà le mapping exact

La reprise normale peut réconcilier l'intention si **tous** les points suivants sont vrais :

- la clé et le champ `git_sha` correspondent au SHA de l'intention ;
- la direction est `git-to-pijul` et la PR correspond ;
- le hash `pijul_change` du ledger est identique à celui enregistré dans l'intention ;
- le parent Git et la base correspondent encore à l'intention.

Le code vérifie ces conditions avant de retirer le journal. Si la passerelle refuse la réconciliation, ne pas contourner le refus.

### Cas B — Le ledger ne contient pas le mapping, mais l'intention contient un hash Pijul

Le hash a été observé et inscrit dans l'intention avant la tentative de sauvegarde du ledger. Cela ne prouve pas à lui seul que le mapping est présent et correct dans le dépôt Pijul courant.

- Conserver la copie de secours.
- Vérifier l'état Pijul et l'historique d'audit avec les outils correspondant à la version installée.
- Ne pas fabriquer une entrée de ledger et ne pas retirer l'intention sur la seule base du champ `pijul_change`.
- La passerelle ne fournit pas encore de commande de réparation manuelle vérifiée. Si l'existence et la provenance du change ne peuvent pas être établies sans ambiguïté, arrêter et préparer un cas minimal reproductible.

### Cas C — Le ledger ne contient pas le mapping et `pijul_change` vaut `null`

L'interruption a pu se produire avant la mutation Pijul ou après celle-ci, mais avant l'observation du delta. Le fichier seul ne permet pas de distinguer ces situations.

- Ne pas relancer l'import sur le dépôt original.
- Conserver tous les fichiers et journaux.
- Étudier une copie isolée du dépôt et du miroir pour déterminer si le mapping peut être établi de manière reproductible.
- Si le channel OID nécessaire a disparu ou si plusieurs changes candidats existent, considérer le mapping comme indéterminé. Ne pas choisir arbitrairement un change.

### Cas D — Intention invalide ou état contradictoire

Une intention illisible, un SHA de parent/base incohérent, un ledger différent de l'intention ou une attribution de PR contradictoire est un état de récupération bloqué. Préserver les données et demander une revue technique ; ne pas supprimer le marqueur pour faire repartir le pipeline.

## Informations à fournir pour une analyse

Fournir, après masquage des chemins privés et données sensibles :

- le SHA Git, son parent, la base et le numéro de PR ;
- le contenu de l'intention et l'entrée de ledger correspondante, s'il y en a une ;
- la version exacte de Pijul ;
- la liste des channels et l'état de triage, recueillis en lecture seule ;
- l'étape exacte à laquelle le processus s'est arrêté et les erreurs complètes ;
- si possible, une copie synthétique reproductible, sans données de production.

## Limite produit restante

Avant exploitation en production, il reste à implémenter une commande explicite de récupération qui vérifie l'état Pijul réel, demande confirmation opérateur, écrit le ledger de façon atomique et ne supprime l'intention qu'après validation. Ce runbook ne remplace ni cette commande ni les tests d'interruption avec Pijul beta.24.
