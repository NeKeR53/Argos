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
  - **Déployée pour un autre** (décidé le 2026-11-01) : l'Assistant ou un Agent peut créer une surface pour l'Assistant ou n'importe quel Agent (`surface_create` avec `agent`), par exemple un Agent spécialisé dans les cockpits qui construit pour les autres. Elle appartient au destinataire : les messages et événements de la page lui arrivent. Il en est prévenu par un message, avec la consigne du constructeur (`brief`). Le constructeur peut encore la modifier, la nourrir et la supprimer. Jamais pour un Worker.
- **D27 — Hébergées par l'app**, jamais par un serveur lancé par l'agent. L'agent fournit HTML/CSS/JS via un outil ; un SDK injecté fournit un composant chat standard (markdown + images) et le bridge `argos.send` / `argos.on`.
- **D28 — Sandbox** : webview avec CSP stricte, aucun accès système, seul le bridge.
  - **Aussi dans un navigateur, sur le Mac ou un autre appareil du réseau local** (décidé le 2026-11-01) : le daemon sert chaque surface (`[surfaces] listen`, défaut `0.0.0.0:7890` ; `/surfaces/<id>`, index sur `/`). La page servie n'est qu'un hôte : même iframe bac à sable, même CSP, même SDK ; le pont passe par un WebSocket propre à la surface, qui ne permet que `send`, `emit`, `call` et le flux de la page.
  - **Accès** (choix de l'utilisateur) : depuis le Mac lui-même (pair et `Host` sur la boucle locale), sans jeton. Depuis un autre appareil, il faut l'avoir **associé** : l'index, sur le Mac seulement, montre un QR code qui porte un jeton propre aux surfaces (`$ARGOS_HOME/surfaces-token`, distinct de celui de l'API) ; l'appareil qui le scanne (ou qui tape le code court affiché dessous) reçoit son propre secret en cookie (`HttpOnly`, un an) et se révoque seul depuis l'index du Mac. **Les liens des agents ne portent pas de clé** : ils peuvent passer par Telegram, qui n'est pas chiffré de bout en bout, sans ouvrir l'accès. Renouveler la clé change le QR code sans toucher aux appareils associés. Chaque page s'installe sur l'écran d'accueil. Les liens nomment le Mac par son nom Bonjour (`<nom>.local`), ou `[surfaces] public_url`. Limite : HTTP en clair, la clé et les secrets peuvent être lus par qui écoute le Wi-Fi. Restent refusés : un WebSocket ouvert par une page d'une autre origine, l'encadrement des pages.

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
- **D44 — Synchro de workspace entre nœuds** (décidé le 2026-10-31). Le workspace d'un Agent placé sur un nœud peut avoir une copie locale sur le hub, synchronisée dans les deux sens et en continu, pour lancer et déboguer le projet sur le Mac pendant que l'Agent le modifie sur son nœud. Ni commit, ni push, ni dépôt intermédiaire. Les règles :
  - **Moteur natif, sur la connexion hub → nœud** : aucun outil externe, aucun port ni process de plus sur le nœud. L'infrastructure copie des fichiers ; elle ne décide rien (D5).
  - **Ouverte par l'utilisateur**, depuis la console ou par l'Assistant à sa demande. Ni les Agents ni les Workers n'en ouvrent. La copie locale part d'un dossier vide ou absent : la première passe copie le nœud vers le hub, sans fusionner deux arbres étrangers.
  - **Hors synchro** : `.git/` (l'historique reste sur le nœud, les commits s'y font), ce qu'ignore le `.gitignore` du projet, et une liste d'exclusions propre à la synchro. Les dossiers de build (`target/`, `node_modules/`, `dist/`…) restent ainsi propres à chaque machine : on compile chez soi, pour son OS et son architecture.
  - **Conflit** (un fichier modifié des deux côtés depuis le dernier état commun) : la version du nœud, celle de l'Agent, garde le chemin ; celle du hub est conservée à côté sous `<nom>.conflit-hub`, des deux côtés, et une escalade prévient l'utilisateur. Une modification l'emporte sur une suppression. Rien n'est jamais perdu en silence.
  - **Nœud injoignable** : la copie locale reste utilisable et modifiable ; les deux côtés se rapprochent au retour du nœud, avec la même règle de conflit.
  - **Pas encore** : la synchro entre deux nœuds dont aucun n'est le hub (le cas du hub sur le VPS et du Mac en nœud) passera par le hub ; à concevoir avec les vrais serveurs.

## 13 quinquies. Messagerie

- **D45 — Canaux de discussion** (décidé le 2026-11-01, filet de sécurité ajouté le même jour). Un canal est une voie par laquelle l'utilisateur parle à l'Assistant hors de la console et de l'orbe : Telegram d'abord, d'autres messageries plus tard. Le daemon transporte, l'Assistant choisit où il répond (D5). Pas d'Agent intermédiaire : il ajouterait deux tours de LLM par échange, de la latence en vocal et une reformulation, alors que l'utilisateur ne parle qu'à l'Assistant (D2, D3). Les règles :
  - **Type et canal** : un type de canal est un connecteur codé dans le daemon (`telegram`, puis d'autres), qui ne fait que transporter. Un canal est une instance configurée d'un type, nommée par son identifiant : `[channels.<id>]` dans `config.toml` avec `type = "<type>"`, son unique interlocuteur et ses secrets hors de `config.toml`. Ajouter une messagerie, c'est ajouter un type ; ajouter un compte ou un bot, c'est ajouter un canal.
  - **Capacités déclarées par le type** : texte, vocal reçu, vocal envoyé, boutons, pièces jointes, longueur maximale d'un message. Ce que le canal ne sait pas faire est refusé ou dégradé par le daemon (vocal → texte, boutons → options numérotées), jamais ignoré en silence.
  - **Un seul interlocuteur par canal** : l'utilisateur. Un inconnu n'obtient rien. Aucun port ouvert quand le type permet de s'en passer.
  - **Entrée** : un message reçu est remis à l'Assistant au nom de l'utilisateur, préfixé par son canal, par exemple « [Canal telegram] » ou « [Canal telegram, message vocal transcrit] ». Jamais à un Agent ni à un Worker. Les vocaux sont transcrits en local, avec le moteur de `[voice]`.
  - **Sortie, décidée par l'Assistant** : outil `channel_send(channel, text, voice?)`, réservé à l'Assistant, et `channel_list` pour connaître les canaux actifs et leurs capacités. L'Assistant s'en sert pour répondre ailleurs que sur le canal de la question, ou pour écrire de sa propre initiative : prévenir l'utilisateur qui l'a demandé, ou lui donner un résultat attendu. Il condense pour le canal. La console garde tout le fil, quel que soit le canal.
  - **Filet de sécurité** : un tour ouvert par un message venu d'un canal, qui se termine sans aucun `channel_send`, renvoie sur ce canal la dernière réponse de l'Assistant, en vocal pour un vocal. Règle mécanique, sans jugement (D5) : au premier essai réel, l'Assistant a répondu dans la console sans appeler l'outil, et l'utilisateur n'a rien reçu.
  - **Escalades** : réglage par canal (`escalations = true`). Le daemon les y annonce lui-même, une fois chacune, avec les options en boutons si le canal en a. Exception explicite à D2 : l'utilisateur peut répondre à un Agent escaladé depuis un canal, sans dialogue au-delà de cette réponse.
  - **Garde-fou déterministe** (D34) : un plafond d'envois par heure et par canal, au-delà duquel le daemon refuse `channel_send` et le signale à l'Assistant.
  - **Sur le hub seulement**, comme l'Assistant.

- **D43 — Telegram, premier type de canal** (décidé le 2026-10-31, rattaché à D45 le 2026-11-01). Depuis son téléphone, l'utilisateur écrit ou envoie un vocal à un bot Telegram personnel. Ce qui est propre à Telegram :
  - **Capacités** : texte, vocal dans les deux sens, boutons. Ni photo ni document pour l'instant. Au-delà de 1 024 caractères, la légende d'un vocal est suivie d'un message texte.
  - **Interlocuteur** : le `chat_id` configuré. Long polling `getUpdates` : aucun port ouvert. Jeton dans un fichier à part.
  - **Vocaux** : conversion `ffmpeg`, transcription `whisper-cli`, synthèse `say` ou Kokoro selon `[voice]`, envoi en Ogg/Opus. Seul Telegram voit le son et le texte.
  - **Réponse libre à une escalade** : en citant le message qui l'annonce, en texte ou en vocal.
  - **Limite assumée** : les conversations avec un bot ne sont pas chiffrées de bout en bout, Telegram peut les lire.

## 13 sexies. Installation et mises à jour

- **D46 — Installation guidée et mises à jour maison** (décidé le 2026-11-01). Un point d'entrée par système installe Argos depuis la dernière release ; le daemon sait ensuite se mettre à jour, avec l'app.
  - **Installeur** : `install.sh` (macOS, Linux) et `install.ps1` (Windows) détectent le système et l'architecture, téléchargent l'archive de la release et vérifient sa somme SHA-256, installent l'app et le daemon, déclarent le service (launchd, systemd user ; aucun sous Windows, où `Argos.cmd` lance le daemon), puis écrivent `install.toml` à côté du daemon : où sont l'app et le service. Au lancement, l'app démarre le daemon s'il n'écoute pas : par son service s'il en a un (et que `ARGOS_HOME` n'est pas changé), sinon `argos-daemon hub` détaché, qui survit à l'app.
  - **Configuration confiée à l'IA, chemin déterministe à défaut** : si `claude` ou `codex` est présent et que l'utilisateur accepte, l'installeur lui passe la consigne de `argos-daemon setup prompt` (moteur et authentification, droits, voix, mot de déclenchement, Telegram, micro macOS) ; l'IA demande avant d'installer quoi que ce soit et ne voit jamais un jeton. Sinon quelques questions. Dans les deux cas, la config s'édite par `argos-daemon config set`, qui garde les commentaires et refuse un fichier invalide.
  - **Mises à jour, solution maison plutôt que `tauri-plugin-updater`** : le plugin ne remplace que l'app (pas le daemon, son service ni la config), attend ses propres paquets (`.app.tar.gz`, installeur NSIS/MSI, AppImage) et une signature minisign obligatoire. Argos publie des archives app + daemon non signées : le daemon les lit telles quelles.
  - **Déroulé** : `argos-daemon update` cherche parmi les releases (préversions comprises) une version plus haute ayant l'archive de la plateforme ; `update apply` la télécharge, vérifie la somme, fait dire sa version au nouveau daemon avant de toucher à rien, remplace l'app puis les fichiers déjà présents à côté du daemon puis le daemon (l'ancien gardé en `.previous`), fait migrer la config par le nouveau binaire (`config init`, ancien fichier gardé), et relance le service. Depuis la console, le daemon lance ce même process, détaché de son groupe pour survivre à sa propre relance ; l'avancement passe par `$ARGOS_HOME/updates/status.json`. La console propose la version, suit l'installation et relance l'app une fois remplacée.
  - **Migrations de config** : reconnues à ce qu'elles trouvent dans le fichier, idempotentes, quel que soit l'écart de versions. Première : `[telegram]` devient `[channels.telegram]`.
  - **Limite assumée** : la somme SHA-256 vient de la même release que l'archive : elle protège d'un fichier corrompu, pas d'un compte GitHub compromis. Une signature des sommes (minisign) viendra avec la signature des binaires.
- **D47 — Déplacer un Agent de nœud** (décidé le 2026-11-03 ; verbe de D15). L'Assistant déplace l'un de ses Agents vers un autre nœud, ou vers le hub, par `move_agent(agent, node, workdir?)`. L'Agent reste le même : même id, charte, profil, board, déclencheurs, fonctions, surfaces et escalades (tout cela vit sur le hub).
  - **Mémoire** : elle part avec lui, historique git compris (`git bundle`), et devient la mémoire du nœud d'arrivée. L'ancienne copie est mise de côté sur le nœud de départ (`memory-moved/<id>-<date>`), jamais effacée. Le miroir du hub suit.
  - **Workspace** : un dossier dédié (celui que crée Argos par défaut) est copié en entier, `.git` et fichiers ignorés compris, vers le dossier dédié du nœud d'arrivée. L'ancienne copie reste en place. Un dossier de projet (D42) n'est pas copié : `workdir` désigne le dossier du projet sur le nœud d'arrivée, sans quoi le déplacement est refusé. Fichiers de plus de 100 Mo non copiés, signalés.
  - **Session** : la conversation du moteur reste sur l'ancienne machine ; comme pour un changement de workspace (D42), le prochain message ouvre une session neuve avec la mémoire Argos.
  - **Refusé** pendant un tour, tant qu'un Worker non retiré ou une synchro de workspace (D44) subsiste, ou si le nœud de départ ou d'arrivée est injoignable. Rien n'est changé dans le registre tant que mémoire et workspace ne sont pas arrivés.

## 14. Phases

1. **Noyau local** — Mac seul, sans voix : daemon, sessions persistantes + parking, hiérarchie, serveur MCP de contrôle, boards, mémoire, politique de modèles basique ; GUI console + fenêtres d'agents en chat texte. Réussite : on parle à l'Assistant en texte, il crée un Agent, l'Agent lance des Workers, l'arbre est visible, le stop fonctionne.
2. **Surfaces** — SDK, cockpits, artifacts, bridge.
3. **Voix et overlay** — orbe, push-to-talk, STT/TTS streaming, barge-in.
4. **Multi-nœud** — mesh SSH, hub VPS, provisioning, conteneurs, sync mémoire + miroir Obsidian.
5. **Événements** — cron, interrupteurs, connecteurs, watcher custom.
