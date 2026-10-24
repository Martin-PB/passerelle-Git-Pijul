# Protocole de vérification locale Linux

Ce protocole est la référence pour vérifier la branche `agent/p0-synthetic-roundtrip` de la passerelle Git–Pijul. Il ne déclenche **aucune GitHub Action**, ne pousse rien et ne modifie pas le code source pendant l'exécution.

## 1. Récupérer exactement la branche à vérifier

Depuis un clone du dépôt :

```sh
git fetch origin agent/p0-synthetic-roundtrip
git switch agent/p0-synthetic-roundtrip
git pull --ff-only origin agent/p0-synthetic-roundtrip
git status --short --branch
git rev-parse HEAD
```

Si des modifications locales existent, ne pas les écraser : les sauvegarder ou utiliser un worktree distinct avant de changer de branche. Noter le SHA de HEAD avec le rapport.

## 2. Prérequis

Obligatoires :
- Git ;
- Rust et Cargo (installation via rustup ou les paquets Linux approuvés).

Pour la preuve formelle :
- Elan/Lean 4 via le fichier `lean-toolchain` du dépôt ;
- Lake fourni par cette installation. La version attendue est Lean `v4.19.0`.

Pour les tests qui touchent le vrai moteur :
- Pijul **1.0.0-beta.24**, version visée par le design actuel ;
- `pijul-export` externe seulement pour le test de round-trip, si ce test est activé.

Le script ne télécharge ni n'installe ces dépendances. Ne pas remplacer silencieusement la version beta de Pijul par une autre : consigner la version réellement utilisée. Les résultats obtenus avec une autre version sont exploratoires et ne valident pas le comportement ciblé.

## 3. Exécution normale

À la racine du dépôt :

```sh
bash scripts/verify-local.sh
rc=$?
printf 'code de sortie: %s\\n' "$rc"
```

Le script exécute :
1. contrôle du formatage Rust (`cargo fmt --all -- --check`) ;
2. tests Rust de toutes les cibles (`cargo test --all-targets`) ;
3. build Lean (`lake build Bridge`) si Lake est installé ;
4. vérifications de présence/version de Pijul ;
5. signalement des suites de bout en bout volontairement ignorées.

Les journaux sont conservés dans `.local-verification/<horodatage-UTC>/`, notamment `summary.txt` et un journal par commande. Ce répertoire est local et doit rester hors du commit.

### Interprétation des codes de sortie

- `0` : toutes les étapes configurées ont réussi et aucun contrôle n'a été ignoré. Cela ne signifie pas, à lui seul, que la preuve de correction de bout en bout est achevée.
- `1` : au moins une commande de vérification a échoué.
- `2` : exécution incomplète (prérequis absent, vérification ignorée ou avertissement).

Un test ignoré ou un outil absent n'est jamais un PASS. Lire le résumé **et** les journaux de la commande qui a échoué.

## 4. Tests de bout en bout optionnels

N'activer les tests ignorés qu'après avoir vérifié la présence et la version des outils :

```sh
pijul --version
command -v pijul-export
RUN_IGNORED_ROUNDTRIP=1 bash scripts/verify-local.sh
```

Le test `roundtrip_property_end_to_end` est actuellement ignoré par conception tant que l'import, la résolution déterministe du corpus et l'export ne sont pas validés. Son activation peut donc révéler un blocage de conception ou d'implémentation ; ne pas supprimer `#[ignore]` simplement pour obtenir un résultat vert.

## 5. Test bloquant branches sœurs + réexécution

Le défaut à lever est précis : importer deux branches sœurs successivement dans un même dépôt Pijul beta.24 peut supprimer le channel de leur base commune ; le cache OID peut toutefois persister. Le second import ou un rerun depuis la base peut alors échouer. C'est une hypothèse de conception à vérifier sur le binaire réel, pas une preuve issue du seul code source.

Le script accepte :

```sh
RUN_SIBLING_RERUN_TEST=1 bash scripts/verify-local.sh
```

Le test dédié `sibling_branches_then_rerun_preserves_base_and_mapping` est maintenant écrit dans `tests/roundtrip.rs`, ignoré par défaut et vérifie explicitement que la version observée est `1.0.0-beta.24`. Le script vérifie son existence, puis le lance explicitement avec `--ignored`. Ce test n'a pas encore été exécuté : sa présence ne valide pas le scénario.

Le test utilise uniquement des dépôts synthétiques et couvre :
1. une base commune et deux branches sœurs distinctes ;
2. import de la première branche ;
3. import de la seconde branche dans le même dépôt Pijul ;
4. rerun de la première branche après l'import de la seconde ;
5. présence et attribution des mappings OID→change après les opérations ;
6. vérification explicite que le binaire utilisé est Pijul 1.0.0-beta.24.

Comparer aussi les arbres de fichiers ne suffit pas à établir l'identité des changes ou la préservation des conflits.

## 6. Preuves Lean : ce qu'elles signifient

La commande :

```sh
lake build Bridge
```

vérifie que la bibliothèque Lean se compile avec le toolchain épinglé. Elle ne prouve que les théorèmes présents dans le modèle abstrait. La spécification actuelle formalise notamment un DAG Git muni d'un rang, l'ascendance, l'égalité extensionnelle des arbres, le contrat P0 et des propriétés du ledger. Elle **ne modélise pas encore la sémantique complète des patches Pijul**, et ne démontre pas que le binaire `pijul git` réalise ce modèle.

Les étapes formelles restantes comprennent :
- une relation de raffinement explicite entre commits Git, changes Pijul et états matérialisés ;
- la fidélité des arbres et la préservation des dépendances ;
- une définition opérationnelle et appariée des scénarios de conflits Git/Pijul ;
- l'unicité et la justification du mapping OID→change ;
- l'idempotence du rerun ;
- la publication sûre (aucune sortie visible avant validation des invariants).

Ne pas introduire d'axiomes qui posent directement la correction souhaitée. Chaque hypothèse sur l'outil externe doit rester visible dans l'énoncé du théorème.

## 7. Rapport à renvoyer après les tests

Envoyer :
- SHA de HEAD et état Git ;
- sortie complète de `summary.txt` ;
- journaux de chaque FAIL ;
- versions de `git`, `rustc`, `cargo`, `lake`/Lean et `pijul` ;
- toute commande lancée manuellement en dehors du script.

Masquer les chemins privés ou informations sensibles avant partage. Ne pas joindre les dépôts synthétiques temporaires sauf si un test échoue et qu'un cas minimal reproductible est nécessaire.

## État initial du protocole

Le script et ce document sont des outils de validation, pas des résultats de validation. Tant qu'ils ne sont pas exécutés sur la machine Linux, tous les tests et le build Lean restent **NOT RUN**. Aucune GitHub Action n'est nécessaire ni déclenchée par ce protocole.
