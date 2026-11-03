# Phase 6 — Nœuds pairs (D48)

Objectif : plus de hub. Chaque nœud fait tourner le même daemon complet et possède ses agents en entier. Les nœuds sont reliés par des tunnels SSH maillés. Un nœud éteint (le Mac en veille) n'empêche pas les autres de fonctionner normalement. L'Assistant vit sur un nœud et se déplace comme un Agent, avec ses canaux : sur hermes, on lui parle par Telegram même Mac éteint. L'application se connecte à chaque nœud joignable.

Décisions de l'utilisateur (2026-11-03) : un seul Assistant, déplaçable ; l'application se connecte à tous les nœuds ; liens par tunnels SSH maillés.

## État (2026-11-03)

- **T1 à T4 faits**, testés sur nœuds simulés (`crates/argos-daemon/tests/nodes.rs` : annuaire et Assistant unique, ordres et rapports à travers les nœuds avec file d'attente pendant une coupure, déplacement complet d'un Agent aller-retour, Assistant déplacé avec son canal Telegram, passation d'un ancien hub, nom refusé, tunnel SSH, synchro D44 entre pairs) et côté app (`apps/gui/src/mesh.test.ts`), puis de bout en bout avec deux daemons et l'app dans un navigateur.
- **T5** : migration de l'installation du Mac et de hermes, voir plus bas.
- **T6** : archive `Argos-Node-Linux-arm64.tar.gz` (daemon seul) dans la release, `install.sh --node <nom>`, `argos-daemon update` sur Linux arm64.
- **Limites connues** : une image jointe ne part pas vers un agent d'un autre nœud par le daemon (l'app l'envoie directement au bon nœud) ; l'arrêt d'urgence de l'app part à chaque nœud joignable, celui d'un nœud seul n'arrête que ses agents ; `function_list` ne montre que les fonctions des agents du nœud de l'appelant.

## Point de départ (phase 4)

- Le hub (le Mac) tient tout l'état de contrôle : registre, boards, déclencheurs, fonctions, surfaces, escalades, plan de contrôle. Un nœud ne fait qu'exécuter des sessions et garder des mémoires, et parke ses sessions quand le hub disparaît.
- La connexion est ouverte par le hub et ne sert que dans un sens : ordres du hub, événements et requêtes MCP du nœud.
- `move_agent` (D47) déplace la mémoire et le workspace, mais la fiche, le board et les déclencheurs restent sur le hub.

## Tranches

Chaque tranche se livre seule, testée sur des nœuds simulés (plusieurs daemons sur le Mac, faux CLI, faux `ssh`), puis sur hermes.

### T1 — Lien symétrique entre pairs
- Un seul mode : chaque daemon est complet. `argos-daemon node` disparaît, ou reste un alias le temps de la migration.
- Configuration `[peers.<nom>]` (`ssh` ou `url`, `remote`, jeton), qui remplace `[nodes.<nom>]`. Chaque nœud a un nom (`[node] name`).
- Un lien ouvert par l'un des deux pairs porte des appels dans les deux sens. Deux pairs qui s'ouvrent mutuellement gardent un seul lien.
- Annuaire du maillage : chaque nœud annonce ses agents (id, nom, niveau, parent, état) à ses pairs et tient la liste reçue des autres.
- Fait quand : deux nœuds simulés se voient, chacun liste les agents de l'autre, et un nœud tué disparaît de l'annuaire sans gêner l'autre.

### T2 — Hiérarchie à travers les nœuds
- Le parent d'un agent peut vivre sur un autre nœud. Les verbes de l'Assistant (mission, parker, réveiller, stopper, configurer, inspecter, lister) deviennent des appels au nœud qui détient l'agent.
- Rapports, escalades et dialogue latéral vers un agent d'un autre nœud passent par le lien. Destinataire injoignable : le message attend dans une file persistante (registre du nœud émetteur) et part à la reconnexion, dans l'ordre.
- Un Agent sans son Assistant continue sa mission ; ses rapports attendent.
- Les déclencheurs, fonctions et surfaces d'un agent tournent sur son nœud (fin de l'écart D31).
- Fait quand : l'Assistant sur un nœud A donne une mission à un Agent sur B ; on tue A ; B finit la mission et met son rapport en file ; A relancé reçoit le rapport.

### T3 — Tout se déplace, l'Assistant compris
- `move_agent` transfère tout ce que l'agent possède, directement de pair à pair : fiche, board, déclencheurs, fonctions, surfaces, escalades, capacités, puis mémoire et workspace (D47). Le nœud de départ ne garde rien d'actif (mémoire et dossier mis de côté).
- L'Assistant se déplace de la même façon, à la demande de l'utilisateur : sa mémoire, ses canaux (configuration et jeton Telegram transmis par le lien SSH), ses files. Le canal s'arrête au départ et reprend à l'arrivée.
- Un seul Assistant dans le maillage : un nœud n'en crée un que si l'annuaire n'en connaît aucun.
- Fait quand : l'Assistant passe du Mac à hermes, le Mac est éteint, et on lui parle par Telegram.

### T4 — Application multi-nœuds
- La configuration de l'app liste les nœuds (adresse ou hôte SSH, jeton). Elle se connecte à chacun, ouvre elle-même les tunnels SSH, et assemble arbre, fils, surfaces et carte des nœuds.
- Un nœud injoignable disparaît de la vue, sans bloquer le reste. La voix et la console parlent à l'Assistant sur son nœud.
- Fait quand : l'app montre l'arbre complet du maillage ; Mac seul allumé avec hermes injoignable, l'app reste utilisable pour les agents du Mac.

### T5 — Migration de l'installation actuelle
- Le hub du Mac devient le nœud `mac`. Le nœud `hermes` devient un pair complet. Les agents déjà placés sur hermes y reçoivent leur fiche, leur board et leurs déclencheurs.
- Fait quand : l'installation de tous les jours tourne en pairs, sans perte d'agent, de board ni de mémoire.

### T6 — Installation d'un nœud
- Release `linux-arm64` et `install.sh --node <nom>` (constat de l'essai sur le Pi, docs/PHASE-4.md), écriture de `install.toml` pour les mises à jour.

## Conception retenue

- **Un seul mode de daemon.** `argos-daemon hub` et `argos-daemon node` lancent le même daemon complet (`node` reste un alias). Le nom du nœud et l'écoute des pairs viennent de `[node]` dans `config.toml` (repris de `node.toml` s'il existe).
- **`record.node` veut dire « détenteur ».** `None` : l'agent appartient à ce nœud, qui le fait tourner. `Some(pair)` : c'est une **copie d'annuaire** d'un agent détenu par ce pair, jamais lancée ici. Le registre de chaque nœud contient donc ses agents et les copies de ceux des autres, avec les mêmes ids. Les contraintes (un seul Assistant, noms uniques) valent pour tout le maillage.
- **Annuaire.** À l'ouverture d'un lien, chaque nœud envoie la fiche de tous ses agents ; puis chaque changement d'un de ses agents part à tous ses pairs. Le récepteur met à jour sa copie (`node = Some(émetteur)`).
- **Lien entre pairs** (`src/peer/`). WebSocket avec jeton, ouvert par l'un ou l'autre (tunnel SSH ou URL directe), qui sert ensuite dans les deux sens : appels numérotés avec réponse (`PeerCall`) et annonces d'annuaire. Deux liens entre les mêmes pairs : on garde celui ouvert par le nom le plus petit.
- **Appels entre pairs.**
  - `Tool` : un outil MCP qui vise un agent détenu ailleurs (mission, configuration, déplacement, fonction…) est exécuté par son détenteur, avec l'appelant tel que l'annuaire le connaît.
  - `Deliver` : un message pour un agent détenu ailleurs (rapport, escalade, question, dialogue latéral, déclencheur). Pair injoignable : file persistante (`outbox`), vidée dans l'ordre à la reconnexion.
  - `Memory` : opération sur la mémoire d'un agent, chez son détenteur.
  - `Sync` : arbre d'un workspace, pour la copie d'un déplacement et la synchro D44.
  - `Adopt` : le pair reçoit tout ce que possède un agent qui arrive (fiche, board, déclencheurs, fonctions, surfaces, escalades, capacités, mémoire, canaux pour l'Assistant) et en devient le détenteur.
- **Passation au premier démarrage** (T5). Les fiches qu'un ancien hub tenait pour des agents placés sur un nœud (colonne `handover`) sont transmises à ce nœud par `Adopt` à la première connexion ; mémoire et workspace y sont déjà.
- **Application** (T4). Elle lit la liste des nœuds dans `[gui.nodes]`, se connecte à chacun (tunnel SSH ouvert par l'app) et ne prend de chaque nœud que ses propres agents.

## Questions ouvertes

- Identifiants : les ids d'agent sont déjà des UUID, uniques sans coordination. Les ids de board, de déclencheur et de surface doivent l'être aussi pour voyager.
- Conflits d'écriture : un agent n'a qu'un détenteur à la fois. Pendant un déplacement, le nœud de départ refuse les écritures, puis renvoie vers le nœud d'arrivée.
- Sécurité : le jeton d'un pair donne accès à tous ses agents. Le transfert d'un jeton Telegram passe par le tunnel SSH.
