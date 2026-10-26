# Phase 2 — Surfaces

Objectif (D26–D28) : l'Assistant et les Agents montrent à l'utilisateur des pages HTML.
- **Cockpit** : vivant et bidirectionnel ; l'agent le met à jour et lui pousse des données, la page lui envoie des messages et des événements.
- **Artifact** : résultat figé.

## Choix d'architecture

- **Le daemon stocke, la GUI héberge.** Le daemon garde chaque surface en base (métadonnées et code HTML, schéma v4) et ne sert jamais ce contenu au réseau. La GUI la récupère par l'API WebSocket et l'affiche dans un iframe bac à sable (`sandbox="allow-scripts"` sans `allow-same-origin`, `srcdoc`), avec une CSP stricte sans aucun accès réseau (D28).
- **Le pont** passe par `postMessage` entre l'iframe et la console, puis par le WebSocket jusqu'au daemon. Pas de serveur web supplémentaire ni de nouvelle authentification, et ce sera transparent à travers le hub en phase 4.
- **Le SDK** `window.argos` est injecté avant le HTML de l'agent :
  - `argos.send(texte)` et `argos.emit(nom, données)` vers l'agent ;
  - `argos.on('data' | 'message' | 'busy', f)` ;
  - `argos.chat(élément)`, le composant de discussion standard (D27) ;
  - `argos.surface`, qui décrit la page.

## Ce qui est en place côté daemon

- **Types** (`argos-core`) : `SurfaceKind`, `SurfaceMeta` (id `s-…`, agent, nature, titre, version incrémentée à chaque nouveau code), `SurfaceDoc`.
- **Protocole** :
  - commandes `surfaces`, `surface`, `surface_send`, `surface_emit`, `surface_delete` ;
  - événements `surface_updated { surface, deleted }` et `surface_data { surface, data }`.
  - Ce qu'une surface envoie est remis à son agent, au nom de l'utilisateur, préfixé `[Depuis la surface « titre » (s-…)]` ou `[Événement « nom » de la surface « titre » (s-…)]`.
- **Stockage** (`registry::surfaces`) : seuls l'Assistant et les Agents ont des surfaces ; titre obligatoire ; code non vide de 2 Mo au plus.
- **Outils** (Assistant et Agents) : `surface_create`, `surface_update` (nouvelle version, la page se recharge), `surface_push` (JSON, ou texte tel quel), `surface_list`, `surface_delete`. Une surface n'est modifiable que par son agent.
- **Contrats** : quand préférer une page à du texte, cockpit ou artifact, tout en ligne puisqu'il n'y a pas de réseau, API du SDK, sobriété, mise à jour plutôt que création.
- **Tests** : 2 tests du stockage, 2 de bout en bout pour les outils, 1 test d'API. Recette réelle `the_assistant_builds_a_live_cockpit_and_feeds_it` : sur demande, l'Assistant (Haiku) crée un cockpit « Compteur » branché sur le SDK ; à l'événement de la page, il pousse `{"valeur": 1}`. Passée le 2026-10-26.

## Côté GUI

Réalisé en parallèle par un sous-agent : onglets de surfaces dans le panneau d'un agent, `buildSrcdoc`, le SDK, le pont, la suppression. Voir sa section une fois fusionnée.

## Limites

- Envoyer une image *vers* un agent n'est pas encore possible : le daemon ne transmet que du texte.
- Aucune ressource externe dans une page (pas de CDN) : les bibliothèques doivent être en ligne.
