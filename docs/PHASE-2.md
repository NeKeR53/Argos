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

## Côté GUI (réalisé par un sous-agent, fusionné)

- **Onglets** dans le panneau de l'Assistant et des Agents : « Chat » plus un onglet par surface (titre, badge cockpit / artifact). La liste est lue à la sélection et tenue à jour par `surface_updated`. Une suppression demande confirmation.
- **Iframe** `sandbox="allow-scripts"` seul. `buildSrcdoc` place en tête une CSP sans aucun réseau (`default-src 'none'`, `connect-src 'none'`, `frame-src 'none'`, `form-action 'none'`, `worker-src`, `object-src` et `base-uri` fermés ; seuls `data:` et `blob:` pour les images et les médias), puis le SDK, puis le HTML de l'agent.
- **SDK** `window.argos` (`apps/gui/src/surface/sdk.js`) : `surface`, `send`, `emit`, `on('data' | 'message' | 'busy')` (avec rejeu de ce qui précède l'abonnement, et désabonnement), et `chat(élément)`, le chat standard en markdown minimal fait maison.
  - Piège trouvé pendant la vérification : sans `allow-forms`, un `<form>` ne se soumet pas, même intercepté. Le chat standard n'en utilise pas, et le contrat des agents l'interdit.
- **Pont** (`surface/bridge.ts`) : la console n'accepte que les messages de la `contentWindow` de l'iframe, au format du SDK, de 64 Kio au plus. L'iframe n'a jamais accès à l'API Tauri ni au WebSocket.
- **Flux par surface** : envois, messages de l'agent, état du tour et données poussées, rejoués à chaque chargement. L'historique survit à un changement d'onglet ou à une nouvelle version.
- **Tests** : 55 tests vitest au total, dont 19 pour les surfaces (`buildSrcdoc`, réducteur, filtrage du pont).

## Vérification de bout en bout (2026-10-26)

- **Daemon réel**, Assistant sur Claude Haiku, console servie par Vite, pilotée par agent-browser :
  1. demande écrite dans la console : « Crée-moi un cockpit « Compteur »… » ;
  2. l'onglet « Compteur » apparaît après 10 s ;
  3. un clic sur « +1 » *dans la page de l'agent* lui envoie l'événement ;
  4. l'Assistant pousse la valeur et la page affiche 1.
  - Coût : 0,009 $.
- **Vérifications du sous-agent sur faux CLI** : une requête réseau depuis la page est bloquée, un envoi depuis le chat de la surface arrive à l'agent avec son préfixe, un `emit` aussi, la page se recharge à une nouvelle version, la suppression fonctionne.

## Limites

- Une page peut encore *naviguer* elle-même vers une URL externe (`location = …`) : la CSP ne l'empêche pas. Dans l'app Tauri, il faudra intercepter ces navigations côté Rust (phase 3, avec la fenêtre de l'overlay).
- La fréquence des messages du pont n'est pas bornée, seulement leur taille.
- Envoyer une image *vers* un agent n'est pas encore possible : le daemon ne transmet que du texte.
- Aucune ressource externe dans une page (pas de CDN) : les bibliothèques doivent être en ligne.
