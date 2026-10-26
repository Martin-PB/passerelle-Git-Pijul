# Concurrence et accès au workspace

Le dépôt Pijul et le ledger sont un état partagé : deux imports concurrents peuvent
observer des deltas qui se chevauchent, écraser des checkpoints du ledger ou
interférer avec la préparation du channel `integration`. Le journal d'intention
par SHA ne suffit pas à sérialiser des imports différents.

## Verrous du CLI

Le CLI prend des verrous consultatifs exclusifs, maintenus pendant toute la
commande :

- `ingest` verrouille le miroir Git, via un fichier voisin du miroir ;
- `merge-prs` verrouille d'abord le miroir Git, puis le workspace passerelle ;
- `operator queue` et `operator submit` verrouillent le workspace passerelle.

Si un autre processus détient le verrou requis, la commande échoue immédiatement
avec un message demandant de réessayer après sa fin. Elle ne doit pas continuer
sans verrou. Le verrou est libéré par le système d'exploitation à la fin du
processus, y compris après un arrêt brutal ; le fichier de verrou peut rester
présent sur disque et **sa présence seule ne signifie pas qu'il est détenu**.

L'ordre de verrouillage est toujours miroir puis workspace pour les commandes qui
ont besoin des deux. Les commandes qui n'en ont besoin que d'un n'acquièrent pas
l'autre, ce qui évite un cycle d'attente entre `ingest`, `merge-prs` et les
commandes opérateur.

## Limites

- Il s'agit de verrous consultatifs : les autres outils qui modifient directement
  le miroir ou le dépôt Pijul doivent respecter le même protocole.
- Les fonctions de la bibliothèque Rust ne prennent pas automatiquement le verrou.
  Tout appelant qui les utilise directement pour lire/modifier l'état partagé doit
  conserver un `WorkspaceLock` approprié pendant toute l'opération.
- Les garanties de verrouillage dépendent du support des verrous consultatifs par
  le système de fichiers. La cible prévue est un workspace local Linux.
- Ce mécanisme ne rend pas atomiques les opérations distribuées entre Git, Pijul
  et le ledger ; le journal d'intention et la procédure de reprise restent requis.
