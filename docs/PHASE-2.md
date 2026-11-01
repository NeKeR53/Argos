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

## Surface déployée pour un autre agent (2026-11-01, D26)

- `surface_create` accepte `agent` (nom, id ou « Assistant ») et `brief`. La surface appartient au destinataire ; la colonne `builder` (migration v12, `SurfaceMeta.builder`) garde l'agent qui l'a déployée.
- Le destinataire reçoit `[Surface « titre » (s-…) déployée pour toi par …]`, la consigne, et les outils qui s'y appliquent ; le fait est noté au journal des deux.
- `surface_update`, `surface_push` et `surface_delete` acceptent le propriétaire ou le constructeur ; `surface_list` rend aussi les surfaces déployées pour d'autres.
- Test : `tests/surfaces.rs`, `an_agent_deploys_a_surface_for_another_and_both_can_edit_it`.

## Dans un navigateur (2026-11-01, D28)

- **Serveur HTTP du hub** (`argos_daemon::surface_web`), sur `[surfaces] listen` (défaut `0.0.0.0:7890`, joignable du réseau local) :
  - `GET /` : les surfaces de l'Assistant et des Agents, par agent ;
  - `GET /surfaces/<id>` : page hôte, barre de titre et iframe `sandbox="allow-scripts"` ; le code de l'agent y est composé comme par `buildSrcdoc` (même CSP, `markdown.js` et `sdk.js` repris tels quels par `include_str!`) ;
  - `GET /surfaces/<id>/ws` : le pont. Le daemon envoie `page` (méta et `srcdoc`, à nouveau à chaque version), `feed` (tout l'historique à la connexion, avec `busy`, puis ligne à ligne), `call_result`, `deleted` ; la page envoie `send`, `emit`, `call`, bornés comme au pont de la console.
- **Le flux** est tenu par le daemon depuis son démarrage (500 lignes par surface) : textes partis de la surface (reconnus à leur préfixe, d'où qu'ils viennent), messages de l'agent, état du tour, données poussées. Une page rechargée ou ouverte plus tard retrouve la conversation ; un redémarrage du daemon la vide.
- **Accès** : sans clé si le pair *et* le `Host` sont sur la boucle locale (le Mac lui-même ; un `Host` étranger, DNS rebinding, est traité en autre appareil). Sinon un **appareil associé** (`surface_web::devices`) : son propre secret en cookie `argos_surfaces` (`HttpOnly`, un an), dont `$ARGOS_HOME/surfaces-devices.json` ne garde que l'empreinte BLAKE3, avec le nom déduit du navigateur (« iPhone · Safari ») et les dates d'association et de dernier passage (réécrite au plus une fois par heure).
  - **Associer** : la clé d'association (`$ARGOS_HOME/surfaces-token`) en `?k=`, dans le QR code (réponse 303 qui pose le secret et retire la clé de l'adresse) ; ou le **code court** qui en dérive (`ABCD-EFGH`, 8 caractères sur 32, BLAKE3 de la clé), tapé dans le formulaire de la page 401 (`POST /pair`, retour au chemin demandé). Dix codes faux en dix minutes bloquent les essais. Un ancien cookie qui porte la clé elle-même devient un appareil.
  - **Sur le Mac seulement**, l'index montre le QR code, le code, et la liste des appareils, chacun révocable (`POST /devices/<id>/revoke`) ; « Renouveler le code » change la clé (`POST /pairing/renew`) sans toucher aux appareils associés. Ces POST sont refusés depuis un autre appareil, et sans `Origin` de la page.
  - `Origin` d'une requête égale à celle de la page ; `frame-ancestors 'none'` et `X-Frame-Options: DENY` ; `no-store` ; `Referrer-Policy: same-origin` (avec `no-referrer`, un formulaire enverrait `Origin: null`).
- **Écran d'accueil** : chaque page déclare un manifeste (`/manifest.webmanifest`, `/surfaces/<id>/manifest.webmanifest`, demandés avec le cookie : `crossorigin="use-credentials"`), `apple-touch-icon` et `apple-mobile-web-app-capable` ; icônes publiques : `/icon.svg` (celle de l'app, `apps/gui/src-tauri/icons/icon.svg`), `/icon-180.png` et `/icon-512.png` (tirés de la même, fond plein, dans `src/surface_web/`). Pas de service worker : il demande HTTPS. Une app installée sur iOS peut ne pas avoir les cookies de Safari ; elle s'associe alors avec le code court.
- **Lien** : `surface_create` rend l'adresse de la surface, `surface_list` la donne pour chacune : `http://<nom Bonjour>.local:7890/surfaces/<id>`, **sans clé** (`scutil --get LocalHostName`, sinon `hostname`), ou `[surfaces] public_url`. En écoute locale seule, `http://127.0.0.1:7890/…`. Le daemon affiche les deux index au démarrage.
- **Tests** : `tests/surface_web.rs` (index et page hôte, hôte et origine étrangers refusés, appareil associé par le QR code puis son secret, ancien cookie, QR code et code sur la machine seulement, manifeste et icônes, code court, révocation depuis la machine seulement, renouvellement, pont de bout en bout) et 5 tests unitaires.
- **Pas encore** : un bouton « Ouvrir dans le navigateur » dans la console ; un hub distant (VPS, D32) sert sur *sa* boucle locale, il faudra un tunnel SSH vers ce port.

## Limites

- Une page peut encore *naviguer* elle-même vers une URL externe (`location = …`) : la CSP ne l'empêche pas. Dans l'app Tauri, il faudra intercepter ces navigations côté Rust (phase 3, avec la fenêtre de l'overlay).
- La fréquence des messages du pont n'est pas bornée, seulement leur taille.
- **Pièces jointes** (console → agent) : `send` accepte `attachments` (nom, type, base64 ; 8 au plus, 10 Mo chacune). Le trombone, le glisser-déposer ou le collage d'une capture les ajoutent. Module `argos_daemon::attachments` :
  - les images (png, jpeg, gif, webp) partent en entrée native du moteur : blocs `image` pour Claude, `{"type":"image","url":"data:…"}` pour Codex (vérifié sur le schéma de `codex app-server` 0.164) ;
  - un fichier texte de 100 Kio au plus est recopié dans le message ;
  - un autre fichier (PDF…) est enregistré dans `$ARGOS_HOME/attachments/<agent>/`, et l'agent reçoit son chemin. Vers un agent d'un autre nœud, seuls les images et le texte passent.
  - Une surface ne peut toujours pas envoyer d'image. Le rejeu après un repli de modèle ne renvoie que le texte.
- Aucune ressource externe dans une page (pas de CDN) : les bibliothèques doivent être en ligne.
