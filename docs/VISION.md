# Argos — Vision et décisions

Argos est un assistant IA personnel qui délègue à une hiérarchie d'agents. On parle à un seul Assistant, à la voix ; il orchestre des Agents durables, qui exécutent leurs missions et délèguent au besoin à des Workers éphémères. Chaque agent est une vraie session `claude` (Claude Code) ou `codex`.

Ce document fige les décisions issues de la session de cadrage du 2026-10-25. Le vocabulaire est défini dans [GLOSSARY.md](GLOSSARY.md). Toute décision remise en cause doit être modifiée ici, pas contournée dans le code.

## 1. Hiérarchie

- **D1 — Trois niveaux fixes** : Assistant (L0, unique) → Agents (L1) → Workers (L2). Pas de profondeur arbitraire.
- **D2 — Interaction humaine** : l'utilisateur parle à l'Assistant (voix) et, quand un Agent est escaladé, traite avec lui dans sa fenêtre (texte + surfaces). Jamais avec un Worker.
- **D3 — Voix réservée à l'Assistant.** Les Agents se traitent en texte et via leurs surfaces ; l'Assistant se met en retrait pendant ce temps.

## 2. Architecture

- **D4 — Agents = vrais process CLI** `claude` / `codex`, pilotés en subprocess avec sortie structurée (`stream-json` / JSONL), pas de parsing de terminal.
- **D5 — App hôte « bête »** : l'infrastructure exécute, elle ne décide pas. Toute l'intelligence d'orchestration vit dans les sessions agents.
- **D6 — Daemon Rust headless** sur chaque nœud (process, surfaces, mémoire, événements, API) + **GUI Tauri 2** (cœur Rust, front React/TS) sur le Mac. Workspace Rust avec un crate partagé `argos-core`.
- **D7 — API du daemon en WebSocket dès la phase 1**, utilisée par la GUI même en local, pour que le multi-nœud ne demande aucune réécriture.
- **D8 — Outils de contrôle exposés aux agents par un serveur MCP fourni par le daemon**, avec un jeu d'outils différent par niveau (L0 : gestion des Agents ; L1 : gestion des Workers + board ; L2 : compte rendu de tâche).

## 3. Cycle de vie des sessions

- **D9 — Process persistant tant que l'agent est actif**, parké (process arrêté) au repos, repris par son session-id au besoin.
- **D10 — Reprise après crash** : le daemon réhydrate les Agents depuis le registre et leurs session-ids ; les tâches des Workers en cours passent `interrupted` et l'Agent les relance.
- Vérifié le 2026-10-25 (claude 2.1.310, codex-cli 0.164.0) :
  - `claude -p --input-format stream-json --output-format stream-json --verbose` tient plusieurs tours dans un seul process ; `--resume <session_id>` reprend la mémoire. D9 tient tel quel.
  - Codex tourne en `codex app-server` (JSON-RPC sur stdio) : plusieurs tours par thread, interruption par `turn/interrupt`, reprise par `thread/resume` dans un nouveau process. D9 tient aussi pour Codex (confirmé en T1).

## 4. Remontée d'information

- **D11 — Résumé à chaque saut** : Worker → Agent (détail), Agent → Assistant (résumé ou décision), Assistant → utilisateur (une ligne ou décision).
- **D12 — Silencieux et autonome par défaut.** L'Assistant ne dérange que sur : escalade explicite d'un Agent, fin d'une tâche demandée, dépassement d'un seuil (coût, irréversibilité, ambiguïté), blocage. Verbosité réglable. Le détail est disponible (cockpit de l'Agent), jamais poussé.

## 5. Contrôle

- **D13 — Chaque niveau ne contrôle que le niveau juste en dessous.** L'Assistant n'adresse jamais un Worker. Le *contrôle* reste hiérarchique ; la *conversation* entre Agents est permise (D40).
- **D14 — Deux plans :**
  - **Plan de contrôle, déterministe, appliqué par le daemon** (aucun LLM) : stop, kill, park, resume, move, retire, inspect, réordonner le board. Un stop tue le sous-arbre de process.
  - **Plan cognitif, livré à l'agent** : assigner une mission, replanifier.
  - Reprioriser = déterministe par défaut (réordonne le board, option : parker la tâche courante de priorité inférieure).
- **D15 — Verbes Assistant → Agent** : créer, renommer, retirer, assigner une mission, inspecter, réprioriser, parker/réveiller, stopper/annuler, déplacer de nœud, ouvrir un canal. **Agent → Worker** : spawn, assigner la tâche, inspecter, réprioriser, pause, kill.
- **D16 — Panic stop** global dans l'overlay (bouton + raccourci), arrêt dur de tout l'arbre.

## 6. Tâches et registre

- **D17 — Le board appartient à l'Agent** (backlog, priorités, états `pending / in-progress / blocked / done / cancelled`). L'Agent découpe toute mission en tâches avant d'exécuter. Il les réalise lui-même par défaut et tient leur état à jour. Il appelle un Worker si et seulement si le gain attendu porte sur le temps **et** les tokens, en comptant le lancement, le contexte transmis, les échanges et la vérification ; sinon, ou en cas de doute, il exécute directement (décision du 2026-10-28).
- **D18 — Un Worker = une tâche**, éphémère, sans mémoire persistante.
- **D19 — L'Assistant manipule des missions**, pas des tâches ; il lit les boards et peut réinjecter/réprioriser au niveau mission.
- **D20 — Registre des Agents à base de chartes** (nom, périmètre, ressources possédées). Nouvelle mission : réutiliser l'Agent dont la charte correspond, sinon en créer un. Création autonome + notification, réglable en confirmation obligatoire.

## 7. Modèles

- **D21 — Cascade de choix** : l'utilisateur fixe moteur + modèle de l'Assistant ; l'Assistant choisit pour les Agents, l'Agent pour ses Workers. Override utilisateur toujours prioritaire.
- **D22 — Politique de modèles** : profils sémantiques éditables (ex. `code` → Claude Code + Opus 5.5 ; `léger` → Claude Haiku 5.5, puis Codex `gpt-6-luna`), chaîne de repli quand un quota est épuisé (détecté par le daemon, notification courte), plafonds globaux. L'agent choisit un profil, pas un modèle en dur. **Le moins cher qui suffit** : `léger` est le profil par défaut, pour les Agents et Workers qui lisent, trient ou résument ; choisir `code` pour un Agent qui programme ; `standard` est réservé au jugement fin et aux textes soignés (décision de l'utilisateur, 2026-10-27).

## 8. Mémoire

- **D23 — Mémoire persistante pour l'Assistant et les Agents** (pas les Workers), en fichiers Markdown sur le nœud de l'agent : index, journal, notes de domaine, registre des enfants. Deux étages : notes libres écrites par l'agent + état structuré écrit par le daemon. La mémoire vit sur le nœud de l'agent, avec son git ; le hub y relaie chaque opération et en garde un miroir sans historique, mis à jour après chaque écriture et à chaque reconnexion. Nœud absent : lecture sur le miroir, écriture refusée (confirmé par l'utilisateur, phase 4).
- **D24 — Discipline de contexte** : la session charge un digest + les notes pertinentes à la demande ; avant la saturation, l'agent résume dans sa mémoire puis purge.
- **D25 — Git local par dossier mémoire** comme journal d'annulation (jamais utilisé pour la synchro). Sync par SSH via le mesh vers le hub, miroir sur le Mac ouvrable dans Obsidian **en consultation seule**.

## 9. Surfaces

- **D26 — Chaque agent possède plusieurs surfaces** de deux natures : **cockpit** (vivant, bidirectionnel, mis à jour en direct) et **artifact** (snapshot autonome).
- **D27 — Hébergées par l'app**, jamais par un serveur lancé par l'agent. L'agent fournit HTML/CSS/JS via un outil ; un SDK injecté fournit un composant chat standard (markdown + images) et le bridge `argos.send` / `argos.on`.
- **D28 — Sandbox** : webview avec CSP stricte, aucun accès système, seul le bridge.

## 10. Événements

- **D29 — Trigger → événement → agent cible.** Types : cron, interrupteur (posé dans une surface ou l'overlay), watcher.
- **D30 — Watchers** : connecteurs déclaratifs (email, fichier, webhook, MQTT/Home Assistant, calendrier ; éventuellement serveurs MCP comme sources) ; script custom sandboxé avec une API `emit()` étroite comme soupape.
- **D31 — Créés par l'Assistant et les Agents** (pas les Workers). Un trigger tourne sur le nœud de ce qu'il observe. Un événement réveille un agent parké.

## 11. Multi-nœud

- **D32 — Étoile sur SSH** : chaque daemon écoute en local, les nœuds distants sont atteints par tunnel SSH. Le **hub est sur le VPS** et héberge l'Assistant ; le Mac est un client (GUI + voix). Les daemons distants restent autonomes quand le hub est absent et mettent les escalades en file. *Pas encore : aujourd'hui un nœud sans hub parke ses sessions (docs/PHASE-4.md).*
- **D33 — Ajout de nœud = privilège humain.** Depuis la console : détection OS/arch, push du binaire cross-compilé, installation en service (systemd/launchd), enregistrement. L'Assistant place des agents uniquement sur des nœuds enregistrés.

## 12. Sécurité

- **D34 — Garde-fous déterministes** plutôt que validation action par action : répertoire borné, profil de capacités, allow/deny-list de commandes, chemins protégés, politique réseau, plafonds de dépense.
  - **Droits des Workers** (décision de l'utilisateur, 2026-10-27) : par défaut ils ont **les droits de l'utilisateur**, sans approbation (commandes, réseau, installations, fichiers hors de leur répertoire), sinon ils ne servent à rien. Réglable : `[workers] access = "full" | "workspace"`. Les Agents partagent ces droits pour exécuter directement leurs tâches ; seul l'Assistant reste sans commande ni écriture (décision du 2026-10-28). Les garde-fous restants passent par le contrat : ce qui demande l'utilisateur en personne (connexion à un compte, mot de passe) est escaladé avec la commande à lancer, et l'irréversible hors tâche est demandé d'abord (D35).
- **D35 — Les actions irréversibles ou hors scope sont escaladées**, pas exécutées.
- **D36 — Isolation configurable par nœud** (overridable par agent) : conteneur par défaut sur les serveurs, répertoire borné en local.

## 13. Voix et interface

- **D37 — Push-to-talk.** STT et TTS **en streaming, locaux sur le Mac** (ex. Whisper.cpp + TTS neural local), backends API payants activables en option. Barge-in : VAD + annulation d'écho, le TTS se coupe dès que l'utilisateur parle. Seul le texte part vers le hub.
- **D38 — Shell** : orbe flottante toujours au premier plan, ou globe animé dans le Dock sur macOS au choix (repos / écoute / réflexion / parole) ; choix mémorisé, le Dock remplace l'orbe flottante. Console invocable au raccourci ou au clic sur le globe (arbre des Agents, état global, escalades, panic stop), surfaces en fenêtres flottantes indépendantes.
- **D39 — Coût par agent** visible dans la console.

## 13 bis. Dialogue entre Agents

- **D40 — Dialogue latéral entre Agents** (décidé le 2026-10-26). Un Agent peut poser une question ou transmettre une information à un autre Agent, par exemple demander à l'Agent « Mail » le dernier courriel d'un contact. Les règles :
  - **Questions et informations seulement, jamais des ordres.** On ne touche ni au board, ni aux Workers, ni à la mémoire d'un autre Agent ; celui qui reçoit décide seul de ce qu'il en fait.
  - **Entre Agents seulement.** Ni l'Assistant (qui garde ses outils de pilotage) ni les Workers n'y prennent part.
  - **Adressé par le nom de l'Agent, acheminé par le daemon, quel que soit le serveur** : l'Agent qui parle ne sait pas où tourne l'autre. Phase 1 : même daemon ; phase 4 : le mesh achemine entre nœuds.
  - **Asynchrone**, comme les comptes rendus : la réponse arrive plus tard comme un message ; personne n'attend.
  - **Visible** : chaque message porte son expéditeur, le dialogue est noté au journal des deux Agents et apparaît dans la console.
  - **Garde-fou déterministe** contre les échanges sans fin : un nombre limité de messages par paire d'Agents sur une fenêtre de temps.

## 13 ter. Fonctions publiées

- **D41 — Fonctions créées par les Agents, exécutées sans LLM** (décidé le 2026-10-27). Un Agent publie des actions déterministes de son domaine, par exemple `set_light(name, state)`. Le daemon en conserve la définition et la signature ; l'Assistant et les autres Agents les découvrent et les appellent directement, sans mission, Worker ni réveil du propriétaire. Seul le propriétaire les crée, remplace ou retire ; les Workers ne les publient ni ne les appellent.
  - **Première version : requêtes HTTP déclaratives**, paramètres scalaires typés, corps JSON, substitutions dans le chemin/query et les données, serveur fixe, secrets d'environnement dans les en-têtes. Pas de code arbitraire. L'exécution se fait sur le nœud du propriétaire, même parké ; un nœud absent fait échouer l'appel sans repli sur le hub.
  - **Surfaces** : `await argos.call(name, arguments)` appelle uniquement une fonction du propriétaire de cette surface via le bridge, puis rend le résultat JSON ou une erreur. Le HTML n'a ni réseau ni identifiants de service. `send` et `emit` gardent leur sens cognitif.
  - **Contrôle** : validation des arguments, délais et tailles bornés, appels journalisés sans arguments ni secrets. Stop/retrait/panic stop annulent les appels en cours, sans pouvoir annuler un effet déjà produit au service distant. Aucune relance automatique d'une action.
  - **Exception explicite à D13/D40** : appeler un service publié autorise seulement l'action exposée ; cela ne donne aucun contrôle sur le board, la mémoire ou les Workers du propriétaire. Les dialogues latéraux restent des questions et informations.

## 13 quater. Workspaces

- **D42 — Rattachement à un workspace** (décidé le 2026-10-28). Un Agent peut travailler dans un dossier de projet existant sur son nœud, choisi à la création (`workdir`), par l’Assistant via `configure_agent`, ou depuis son en-tête dans la console. Par défaut, il reçoit un dossier dédié dans les données Argos. Ses Workers partagent son répertoire. Plusieurs Agents peuvent utiliser le même dossier ; ce rattachement ne crée ni copie ni worktree Git.
  - **Configuration par l’Assistant** : `configure_agent(agent, name?, workdir?, profile?, engine?, model?)` modifie les seuls champs fournis, ensemble dans une transaction. `agent` accepte un nom ou un id. Un profil choisit moteur et modèle selon la politique ; `engine`/`model` permettent un choix exact demandé par l’utilisateur (harness `claude` ou `codex`). `profile` ne se combine pas avec `engine`/`model` ; `model` seul conserve le moteur. Un changement de moteur exige un modèle. Un Agent ou Worker ne configure jamais un autre Agent. La mémoire et l’identité persistent, et les changements de workspace/moteur ouvrent une nouvelle session au prochain message. Le renommage seul ne coupe pas un tour.
  - Le `AGENTS.md` du workspace est chargé au démarrage de chaque session : nativement par Codex, explicitement par Argos pour Claude (qui continue aussi de charger `CLAUDE.md`). Claude reçoit la consigne de lire les éventuels `AGENTS.md` des sous-dossiers avant de les modifier. Sur un nœud, la lecture se fait sur ce nœud.
  - Le changement de dossier est refusé pendant un tour ou tant qu'un Worker non retiré subsiste. Une session inactive est parkée, son identifiant est effacé et le prochain message ouvre une session neuve avec la mémoire Argos conservée. Les fichiers de l'ancien dossier restent en place. Le chemin est absolu sur le nœud de l'Agent ; le hub vérifie l'existence des dossiers locaux ; un dossier distant est vérifié au prochain démarrage sur son nœud.

## 14. Phases

1. **Noyau local** — Mac seul, sans voix : daemon, sessions persistantes + parking, hiérarchie, serveur MCP de contrôle, boards, mémoire, politique de modèles basique ; GUI console + fenêtres d'agents en chat texte. Réussite : on parle à l'Assistant en texte, il crée un Agent, l'Agent lance des Workers, l'arbre est visible, le stop fonctionne.
2. **Surfaces** — SDK, cockpits, artifacts, bridge.
3. **Voix et overlay** — orbe, push-to-talk, STT/TTS streaming, barge-in.
4. **Multi-nœud** — mesh SSH, hub VPS, provisioning, conteneurs, sync mémoire + miroir Obsidian.
5. **Événements** — cron, interrupteurs, connecteurs, watcher custom.
