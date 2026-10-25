# Argos — Vision et décisions

Argos est un assistant IA personnel qui délègue à une hiérarchie d'agents. On parle à un seul Assistant, à la voix ; il orchestre des Agents durables, qui orchestrent des Workers éphémères. Chaque agent est une vraie session `claude` (Claude Code) ou `codex`.

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

- **D13 — Chaque niveau ne contrôle que le niveau juste en dessous.** L'Assistant n'adresse jamais un Worker.
- **D14 — Deux plans :**
  - **Plan de contrôle, déterministe, appliqué par le daemon** (aucun LLM) : stop, kill, park, resume, move, retire, inspect, réordonner le board. Un stop tue le sous-arbre de process.
  - **Plan cognitif, livré à l'agent** : assigner une mission, replanifier.
  - Reprioriser = déterministe par défaut (réordonne le board, option : parker la tâche courante de priorité inférieure).
- **D15 — Verbes Assistant → Agent** : créer, retirer, assigner une mission, inspecter, réprioriser, parker/réveiller, stopper/annuler, déplacer de nœud, ouvrir un canal. **Agent → Worker** : spawn, assigner la tâche, inspecter, réprioriser, pause, kill.
- **D16 — Panic stop** global dans l'overlay (bouton + raccourci), arrêt dur de tout l'arbre.

## 6. Tâches et registre

- **D17 — Le board appartient à l'Agent** (backlog, priorités, états `pending / in-progress / blocked / done / cancelled`). L'Agent découpe toute mission en tâches avant d'exécuter.
- **D18 — Un Worker = une tâche**, éphémère, sans mémoire persistante.
- **D19 — L'Assistant manipule des missions**, pas des tâches ; il lit les boards et peut réinjecter/réprioriser au niveau mission.
- **D20 — Registre des Agents à base de chartes** (nom, périmètre, ressources possédées). Nouvelle mission : réutiliser l'Agent dont la charte correspond, sinon en créer un. Création autonome + notification, réglable en confirmation obligatoire.

## 7. Modèles

- **D21 — Cascade de choix** : l'utilisateur fixe moteur + modèle de l'Assistant ; l'Assistant choisit pour les Agents, l'Agent pour ses Workers. Override utilisateur toujours prioritaire.
- **D22 — Politique de modèles** : profils sémantiques éditables (ex. `code` → Claude Code + Opus 5.5 ; `léger` → Codex + `gpt-6-luna`), chaîne de repli quand un quota est épuisé (détecté par le daemon, notification courte), plafonds globaux. L'agent choisit un profil, pas un modèle en dur.

## 8. Mémoire

- **D23 — Mémoire persistante pour l'Assistant et les Agents** (pas les Workers), en fichiers Markdown sur le nœud de l'agent : index, journal, notes de domaine, registre des enfants. Deux étages : notes libres écrites par l'agent + état structuré écrit par le daemon.
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

- **D32 — Étoile sur SSH** : chaque daemon écoute en local, les nœuds distants sont atteints par tunnel SSH. Le **hub est sur le VPS** et héberge l'Assistant ; le Mac est un client (GUI + voix). Les daemons distants restent autonomes quand le hub est absent et mettent les escalades en file.
- **D33 — Ajout de nœud = privilège humain.** Depuis la console : détection OS/arch, push du binaire cross-compilé, installation en service (systemd/launchd), enregistrement. L'Assistant place des agents uniquement sur des nœuds enregistrés.

## 12. Sécurité

- **D34 — Garde-fous déterministes** plutôt que validation action par action : répertoire borné, profil de capacités, allow/deny-list de commandes, chemins protégés, politique réseau, plafonds de dépense.
- **D35 — Les actions irréversibles ou hors scope sont escaladées**, pas exécutées.
- **D36 — Isolation configurable par nœud** (overridable par agent) : conteneur par défaut sur les serveurs, répertoire borné en local.

## 13. Voix et interface

- **D37 — Push-to-talk.** STT et TTS **en streaming, locaux sur le Mac** (ex. Whisper.cpp + TTS neural local), backends API payants activables en option. Barge-in : VAD + annulation d'écho, le TTS se coupe dès que l'utilisateur parle. Seul le texte part vers le hub.
- **D38 — Shell** : orbe toujours au premier plan (repos / écoute / réflexion / parole), console invocable au raccourci (arbre des Agents, état global, escalades, panic stop), surfaces en fenêtres flottantes indépendantes.
- **D39 — Coût par agent** visible dans la console.

## 14. Phases

1. **Noyau local** — Mac seul, sans voix : daemon, sessions persistantes + parking, hiérarchie, serveur MCP de contrôle, boards, mémoire, politique de modèles basique ; GUI console + fenêtres d'agents en chat texte. Réussite : on parle à l'Assistant en texte, il crée un Agent, l'Agent lance des Workers, l'arbre est visible, le stop fonctionne.
2. **Surfaces** — SDK, cockpits, artifacts, bridge.
3. **Voix et overlay** — orbe, push-to-talk, STT/TTS streaming, barge-in.
4. **Multi-nœud** — mesh SSH, hub VPS, provisioning, conteneurs, sync mémoire + miroir Obsidian.
5. **Événements** — cron, interrupteurs, connecteurs, watcher custom.
