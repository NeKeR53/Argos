# Phase 1 — Noyau local

Objectif : sur le Mac seul, sans voix, parler à l'Assistant en texte ; il crée un Agent, l'Agent lance des Workers, l'arbre est visible dans la console, le stop fonctionne. Décisions de référence : [VISION.md](VISION.md).

## Arborescence cible

```
Cargo.toml                 workspace
crates/argos-core/         types partagés : ids, niveaux, états, événements, messages de l'API WS
crates/argos-daemon/       binaire headless : sessions, arbre, contrôle, MCP, boards, mémoire, API WS
apps/gui/                  app Tauri 2 (src-tauri/ en Rust, front React/TS)
docs/
```

## Choix d'implémentation proposés

Ces points ne figurent pas dans la vision ; ils peuvent être contestés avant leur tranche.

- **État canonique du daemon en SQLite** (registre, arbre, boards, missions, sessions). Transactionnel, survit aux crashs. Le daemon en écrit une projection Markdown dans la mémoire de chaque agent (l'« état structuré » de D23).
- **Un seul endpoint MCP sur le daemon (HTTP), un jeton par agent.** Le jeton identifie l'appelant : le daemon sait qui appelle, à quel niveau, et n'expose que les outils de ce niveau. La config MCP est injectée au lancement de chaque CLI.
- **Adaptateur de moteur** : un trait `Engine` (démarrer, envoyer un tour, interrompre, reprendre, flux d'événements normalisés) avec une implémentation `claude` et une `codex`. Les tests utilisent un faux CLI qui rejoue des sorties enregistrées, pour ne pas consommer de tokens.
- **Agents isolés de la config globale de l'utilisateur.** Un `claude` lancé tel quel hérite des MCP, hooks, plugins et CLAUDE.md globaux. Réglé en T1 :
  - Claude : `--setting-sources project --strict-mcp-config`. Le CLAUDE.md global n'est plus chargé, ni les MCP et plugins de l'utilisateur ; ceux passés par `--mcp-config` restent actifs. `--safe-mode` est écarté car il coupe aussi ces MCP-là, et `--bare` parce qu'il exige une clé API (pas d'OAuth).
  - Codex : un `CODEX_HOME` dédié par agent (`prepare_codex_home`), qui contient seulement un lien vers `auth.json` et une config vide. Aucun MCP ni AGENTS.md global n'est chargé.
  - Reste à traiter en T7 : la mémoire automatique de Claude (`memory_paths.auto`) est toujours active et écrirait dans `~/.claude/projects/…/memory/`, en concurrence avec la mémoire d'Argos.
- **Permissions provisoires** en attendant les garde-fous (D34) : Claude en `--permission-mode acceptEdits`, Codex en `approvalPolicy: never` + `sandbox: workspace-write`. Les requêtes d'approbation que Codex enverrait malgré tout reçoivent une erreur, pour que le tour ne reste pas bloqué.

## Faits vérifiés sur les CLI (2026-10-25)

- **claude 2.1.310** : multi-tours dans un process via `--input-format stream-json` ; session-id dans le champ `session_id` ; fin de tour = ligne `type: result` (`result`, `total_cost_usd`, `usage`) ; événement `rate_limit_event` exploitable pour le repli ; transcripts dans `~/.claude/projects/<cwd encodé>/<session-id>.jsonl`. Flags utiles : `--resume`, `--session-id`, `--model`, `--mcp-config`, `--strict-mcp-config`, `--permission-mode`, `--append-system-prompt`, `--max-budget-usd`, `--include-partial-messages`.
- **codex-cli 0.164.0** : `exec --json` émet `thread.started` (`thread_id`), `turn.started`, `item.completed` (`item.text`), `turn.completed` (`usage`) ; `exec resume <thread_id>` reprend ; `-m`, `-s read-only|workspace-write|danger-full-access`, `-C`, `-c key=value`, `--skip-git-repo-check`, `--ignore-user-config`. `app-server` écoute en `stdio://`, `unix://` ou `ws://` et peut générer son schéma (`generate-json-schema`) : à étudier en T1 pour le mode persistant. Modèles vus : `gpt-6.1-sol` (défaut de la config), `gpt-6-luna`.
- **Toolchain** : rustc/cargo 1.96.1 via Homebrew (pas de rustup : il faudra l'installer en phase 4 pour la cross-compilation ARM/Linux), Node 26 + npm 11, Xcode présent. Pas de Docker (requis seulement sur les serveurs, phase 4).

## Tranches

Chaque tranche se termine par quelque chose qui tourne et se teste.

| # | Tranche | Contenu | Dépend de |
|---|---------|---------|-----------|
| T0 ✅ | Squelette | Workspace Cargo, crates vides, app Tauri qui démarre, `fmt` / `clippy` / `test` verts | — |
| T1 ✅ | Adaptateurs de moteur | Lancer une session `claude` et `codex`, envoyer des tours, lire le flux normalisé, interrompre, reprendre par session-id. Faux CLI pour les tests | T0 |
| T2 ✅ | Arbre et registre | Modèle agent (id, niveau, parent, moteur, modèle, état, charte), persistance SQLite, réhydratation au démarrage | T1 |
| T3 ✅ | Plan de contrôle + API WS | stop/kill du sous-arbre, park/resume, inspect ; API WebSocket (commandes + flux d'événements) | T2 |
| T4 ✅ | Serveur MCP par niveau | Endpoint MCP, jetons par agent, outils L0 (gestion des Agents), L1 (Workers + board), L2 (compte rendu, escalade) | T3 |
| T5 | Contrats des agents | Prompts système par niveau : l'Assistant route des missions et résume ; l'Agent planifie avant d'exécuter ; le Worker fait une tâche et rend compte | T4 |
| T6 ✅ | Boards et missions | Mission → tâches, Worker lié à une tâche, repriorisation déterministe, remontée résumée | T4 |
| T7 ✅ | Mémoire | Dossier par agent (index, journal, notes, enfants), git local, digest chargé au démarrage, outil d'écriture mémoire, compaction avant saturation | T4 |
| T8 ✅ | Politique de modèles | Profils en TOML, résolution profil → moteur + modèle, repli sur quota épuisé, plafonds | T1 |
| T9 | GUI console | Connexion WS, arbre des agents, chat avec l'Assistant, fenêtre d'Agent en chat texte, panic stop | T3 |
| T10 | Recette | Le scénario de réussite de bout en bout, avec de vrais moteurs | tout |

T5 à T9 sont largement parallélisables une fois T4 posé (T9 dès T3).

## T1 — Ce qui est en place

`argos_daemon::engine::Session` : `spawn(SessionSpec)`, `send`, `interrupt`, `park`, `kill`, `next_event`. Événements normalisés : `AgentMessage`, `ToolActivity`, `TurnCompleted { outcome, usage }`, `Exited { code, stderr_tail }`.

- **Claude** : un process `claude -p` en stream-json par session. Argos génère l'id (`--session-id`), connu avant le premier tour ; il le repasse en `--resume`. L'interruption envoie un `control_request` `interrupt` sur stdin : le tour se termine en `error_during_execution`, traduit en `Interrupted`, et le process reste vivant.
- **Codex** : un process `codex app-server` par session, un thread. Une interruption demandée avant que le serveur ait donné l'id du tour est mise en attente, puis envoyée dès que l'id arrive : l'ordre n'est jamais perdu.
- **Groupe de process par session** : `kill` (SIGKILL sur le groupe) tue aussi les commandes lancées par l'agent.
- **Parking** : stdin fermé, le process sort de lui-même (tué au bout de 5 s sinon). Le code de sortie de `claude` reflète son dernier tour : 1 après un tour interrompu, sans que ce soit une panne.
- **Tests** : 30 tests sans tokens (14 unitaires sur des lignes de forme réelle, 16 d'intégration contre `tests/fixtures/fake-claude` et `fake-codex`). Deux tests sur les vrais moteurs, ignorés par défaut, couvrent mémoire, interruption, parking et reprise : `cargo test -p argos-daemon --test real_engines -- --ignored` (haiku, gpt-6-luna). Passés le 2026-10-25.
- **Pas encore fait** (à prévoir dans les tranches suivantes) : deltas de texte en streaming (pour le TTS, phase 3), détection des rate limits (T8), injection du serveur MCP d'Argos (T4).

## T2 — Ce qui est en place

- **Modèle** (`argos-core`) : `AgentRecord` (id, niveau, parent, moteur, modèle, répertoire de travail, charte, état, session-id, date de création), `AgentState` (`active` / `parked` / `retired`), `Charter` (nom, périmètre). Les énumérations ont une forme texte identique à leur forme JSON, utilisée en base.
- **Registre** (`argos_daemon::registry::Registry`, SQLite embarqué via `rusqlite` + `bundled`, base dans `$ARGOS_HOME/argos.db`, par défaut `~/.argos`). Règles appliquées :
  - un seul Assistant, jamais retiré ;
  - un Agent sous l'Assistant, un Worker sous un Agent, rien sous un Worker ;
  - charte obligatoire pour un Agent, interdite ailleurs ; nom unique parmi les Agents vivants, sans tenir compte de la casse (retirer un Agent libère son nom) ;
  - le retrait atteint tous les descendants et il est définitif ; les lignes restent pour l'historique.
  - Un nouvel agent commence `parked` sans session : il n'a encore jamais tourné.
- **`descendants`** rend le sous-arbre qu'un stop doit atteindre (T3).
- **Réhydratation** au démarrage du daemon : Assistant et Agents `active` passent `parked` (repris plus tard par leur session-id), Workers `active` passent `retired` et sont rendus comme perdus, pour que T6 marque leurs tâches `interrupted`.
- **Schéma versionné** (`PRAGMA user_version`) : une base d'une version plus récente est refusée au lieu d'être abîmée.
- `argos-daemon` ouvre le registre, réhydrate et affiche l'arbre. Il ne crée pas encore l'Assistant : moteur et modèle viendront de la configuration utilisateur (à poser avec T3).
- **Tests** : 11 tests du registre (dont redémarrage sur fichier et refus de schéma), 2 de plus dans `argos-core`.

## T3 — Ce qui est en place

- **Configuration** `$ARGOS_HOME/config.toml`, écrite commentée au premier démarrage : moteur et modèle de l'Assistant (défaut : `claude` + `claude-opus-5-5`), adresse de l'API (défaut `127.0.0.1:7878`). Le daemon crée l'Assistant au premier démarrage. Changer ensuite son moteur dans la config n'a pas encore d'effet.
- **Jeton d'accès** `$ARGOS_HOME/token` (créé en 0600), exigé dans l'URL : `ws://127.0.0.1:7878/?token=…`. L'API n'écoute qu'en local.
- **Superviseur** (`argos_daemon::supervisor`), seul à faire le lien entre registre et sessions. Toutes ses opérations sont déterministes (D14) :
  - `send` réveille un agent parké (sa session reprend par son session-id) puis lui transmet le message ;
  - `interrupt`, `park` (attend la sortie du process) ;
  - `stop` : arrêt dur du parent puis de tout son sous-arbre, rend la main une fois les process morts ;
  - `retire` : stop puis retrait ; `panic_stop` : stop de l'Assistant, donc de tout l'arbre ;
  - `shutdown` : parke tout à l'arrêt du daemon (Ctrl-C).
  - Un verrou par agent sérialise réveil, parking et stop d'un même agent sans bloquer les autres. L'état d'exécution (`running`, `busy`) se lit sans attendre ce verrou.
  - Une panne (process sorti de lui-même) remet l'agent en `parked` ; il se réveille au message suivant.
- **Protocole** (`argos_core::protocol`) : requêtes `tree`, `inspect`, `send`, `interrupt`, `park`, `stop`, `retire`, `panic_stop` ; réponses `tree` / `agent` / `done { affected }` / `error` ; événements `message`, `tool`, `turn_completed`, `exited`, `agent_updated`. Chaque connexion reçoit d'abord `hello { protocol }`.
- **Règle D2 appliquée par l'API** : l'utilisateur ne s'adresse pas à un Worker (send, interrupt, park, stop, retire refusés), il peut seulement lire son état. Le panic stop atteint les Workers par cascade.
- **Limites connues** :
  - un stop attend que le réveil en cours du même agent soit fini (jusqu'à 60 s pour une initialisation Codex) ;
  - un client trop lent perd des événements et doit se resynchroniser avec `tree`.
- **Tests** : 9 tests du superviseur et 6 de l'API, sur faux CLI. Essai réel le 2026-10-25 : daemon + client WebSocket Node, réveil de l'Assistant (haiku), réponse relayée en direct, parking, arrêt propre par Ctrl-C.
- `ARGOS_CLAUDE_BIN` / `ARGOS_CODEX_BIN` remplacent les moteurs du daemon (par exemple par les faux CLI) pour essayer la GUI sans tokens.

## T4 — Ce qui est en place

- **Serveur MCP** (`argos_daemon::mcp`, sur `axum`, `[mcp] listen`, défaut `127.0.0.1:7879`). Sous-ensemble du protocole HTTP « streamable » servi en JSON simple, sans session ni flux SSE : `initialize` (version du client acceptée), `ping`, `tools/list`, `tools/call` ; notifications acquittées en 202. Choisi plutôt que le SDK `rmcp` : quatre méthodes, stables d'une version du protocole à l'autre.
- **Un jeton par session** : au réveil, le superviseur émet un jeton et donne à l'agent l'URL `/mcp/<jeton>`. Le jeton identifie l'appelant et fixe ses outils ; il est révoqué quand la session s'arrête (un jeton inconnu reçoit 404).
- **Injection dans les moteurs** :
  - Claude : `--mcp-config` en JSON, plus `--allowedTools mcp__argos`, sinon le mode `-p` refuse l'outil.
  - Codex : la config du `CODEX_HOME` isolé est réécrite à chaque session avec `[mcp_servers.argos]` et `default_tools_approval_mode = "approve"`. Sans ce réglage, `approvalPolicy: never` fait refuser chaque appel faute d'approbation (constaté sur le vrai Codex).
- **Outils** (`argos_daemon::tools`) :
  - Assistant : `list_agents`, `create_agent`, `send_to_agent`, `inspect_agent`, `interrupt_agent`, `park_agent`, `stop_agent`, `retire_agent`.
  - Agent : `spawn_worker`, `list_workers`, `send_to_worker`, `inspect_worker`, `interrupt_worker`, `park_worker`, `stop_worker`, `retire_worker`, `report_to_assistant`, `escalate_to_user`.
  - Worker : `report_to_agent`, `escalate_to_agent`.
- **Autorité (D13)** vérifiée à chaque appel : la cible doit être un enfant direct et non retiré. Un Agent se désigne par son id ou son nom, un Worker par son id. Une erreur d'outil revient à l'agent comme résultat lisible (`isError`), pas comme erreur de protocole.
- **Moteur et modèle par défaut** d'un enfant : ceux du parent (D21), en attendant la politique de modèles (T8).
- **Comptes rendus et escalades** : ils arrivent au parent comme un nouveau message préfixé (`[Rapport de …]`, `[Escalade pour l'utilisateur de …]`, `[Question de …]`), qui le réveille au besoin. Aucun appel n'attend la fin du travail d'un enfant.
- **Stop protégé** : pendant un stop, ni l'agent ni ses enfants ne peuvent être réveillés. Le rapport tardif d'un Worker ne relance donc pas l'Agent qu'on arrête.
- **Nouveaux agents** : répertoire par défaut `$ARGOS_HOME/agents/<nom>-<suffixe>` ; un Worker travaille dans le répertoire de son Agent.
- **Tests** : 9 tests MCP sur faux CLI (jeton, niveaux, création, mission, Worker, rapport, autorité, révocation), 3 unitaires. Tests réels (ignorés par défaut) : un Assistant sur `claude` (haiku) puis sur `codex` (gpt-6-luna) appelle `list_agents` et répond le bon nombre d'Agents. Passés le 2026-10-25.

## T6 — Ce qui est en place

- **Missions et tâches en base** (schéma v2 ; les bases v1 sont migrées automatiquement, les migrations s'enchaînent selon `PRAGMA user_version`). Module `registry::board`. Types partagés dans `argos-core` : `Mission`, `Task`, `Board`, `MissionState` (`open` / `done` / `cancelled`), `TaskState` (`pending` / `in_progress` / `blocked` / `done` / `cancelled` / `interrupted`). Identifiants courts et lisibles : `m-1a2b3c4d`, `t-…`.
- **Seul un Agent vivant a un board** ; une tâche ne se rattache qu'à une mission ouverte de son propre board.
- **Ordre de priorité explicite** : missions et tâches ont un rang ; le board se lit missions ouvertes d'abord (dans leur ordre), puis leurs tâches, puis les tâches hors mission non terminées. Réprioriser une mission réordonne ses tâches avec elle.
- **Planifier avant d'exécuter, mécaniquement (D17)** : `spawn_worker` prend l'identifiant d'une tâche du board (à faire, bloquée ou interrompue), la passe `in_progress` et la lie au Worker. Le Worker reçoit son titre, son détail et le contexte de la mission. Sans tâche inscrite, pas de Worker.
- **Tenue déterministe du board** :
  - le compte rendu d'un Worker (`report_to_agent`, `outcome` `done` / `blocked` / `progress`) met à jour l'état et le résultat de sa tâche, puis prévient l'Agent ;
  - un Worker retiré en pleine tâche, ou perdu dans un crash (réhydratation), laisse sa tâche `interrupted`, prête à repartir avec un nouveau Worker (D10) ;
  - annuler une mission annule ses tâches non terminées et arrête leurs Workers.
- **Nouveaux outils** :
  - Assistant : `assign_mission` (inscrit la mission et réveille l'Agent), `prioritize_mission`, `cancel_mission` ; `inspect_agent` montre le board.
  - Agent : `board`, `add_task`, `update_task`, `reorder_task` ; `report_to_assistant` clôt une mission avec `mission` + `outcome: done`.
  - `send_to_agent` reste pour les simples messages (réponses aux questions).
- **Protocole** : commande `board { agent }`, réponse `board { board }` (lecture seule pour la GUI).
- **Pas fait** : la préemption automatique (parker le Worker d'une mission moins prioritaire quand une autre passe en tête), prévue en option par D14. La repriorisation ne fait que réordonner.
- **Tests** : 8 tests du board, 6 tests MCP de plus (plan obligatoire, rapport qui met à jour la tâche, mission sur le board, annulation qui arrête les Workers, ordre du board), 1 test d'API.

## T7 — Ce qui est en place

- **Une mémoire par Assistant et par Agent** (jamais pour un Worker), dans `$ARGOS_HOME/memory/<id>/`, hors du projet de l'utilisateur. Markdown compatible Obsidian. Module `argos_daemon::memory`.
  - **Tenus par l'agent** : `index.md` (l'essentiel, court), `journal.md` (chronologique), `notes/<nom>.md` (le détail).
  - **Tenu par le daemon** : `state.md`, réécrit à chaque réveil (rôle, périmètre, moteur, répertoire, enfants avec leur état ou leur tâche, board pour un Agent).
  - **Journal alimenté par le daemon** (deuxième étage de D23) : Agent créé, mission confiée / reçue / close / annulée, tâche terminée ou bloquée, retrait, compaction.
- **Git local** dans chaque dossier mémoire : chaque écriture est commitée (auteur « Argos », sans hooks ni signature). Il sert de journal d'annulation (D25). Sans git installé, la mémoire marche sans historique.
- **Digest au réveil (D24)** : le prompt système de la session (`--append-system-prompt` pour Claude, `developerInstructions` pour Codex) reçoit l'index, l'état, les 20 dernières lignes du journal et la liste des fichiers, borné à 12 000 caractères. Il est recalculé à chaque réveil, reprise comprise.
- **Outils** (Assistant et Agents) : `memory_read` (sans argument : liste), `memory_write` (`replace` ou `append`), `memory_search`. Seuls `index.md`, `journal.md` et `notes/<nom>.md` sont inscriptibles ; `state.md` est en lecture seule ; aucun chemin ne sort du dossier.
- **Compaction contrôlée (D24)** : quand un tour occupe 70 % de la fenêtre de contexte ou plus, le daemon demande à l'agent de mettre sa mémoire à jour, puis de répondre « prêt ». Ce tour fini, il parke l'agent et oublie sa session : le réveil suivant ouvre une session neuve, qui repart du digest.
  - La fenêtre vient des moteurs : `modelUsage.<modèle>.contextWindow` chez Claude, `modelContextWindow` chez Codex. Nouveau champ `Usage.context_window`, et `Usage::context_fill()`.
- **Mémoire automatique de Claude coupée** : `--settings '{"autoMemoryEnabled": false}'`. Vérifié : `memory_paths` disparaît de l'init.
- **Pas fait** : le miroir Obsidian synchronisé sur le Mac (phase 4).
- **Tests** : 7 tests du module, 5 de bout en bout (outils MCP, digest au réveil, Worker sans mémoire, double journal d'une mission, compaction complète jusqu'à la session neuve). Les 4 tests réels repassent avec la mémoire branchée (2026-10-25).

## T8 — Ce qui est en place

- **Politique de modèles** (`argos_daemon::policy`) dans la section `[models]` de `config.toml`, écrite commentée au premier démarrage. Un fichier sans cette section reçoit la politique par défaut. Une politique invalide (profil par défaut absent, chaîne vide) empêche le démarrage.
- **Profils par défaut**, chacun étant une chaîne : premier choix, puis replis.
  - `code` : Claude `claude-opus-5-5`, puis Codex `gpt-6.1-sol`.
  - `standard` (par défaut) : Claude `claude-sonnet-5-5`, puis Codex `gpt-6.1-sol`.
  - `léger` : Codex `gpt-6-luna`, puis Claude `claude-haiku-5-5`.
- **Les agents choisissent un profil, jamais un modèle** : `create_agent` et `spawn_worker` prennent `profile`. Le schéma MCP énumère les profils avec leur usage. Les anciens arguments `engine` / `model` ont disparu. Le profil retenu est mémorisé sur l'agent (`AgentRecord.profile`, schéma v3 par migration). L'Assistant, choisi par l'utilisateur, n'a pas de profil.
- **Repli sur quota épuisé** :
  - les moteurs le signalent par `EngineEvent::RateLimited` : `rate_limit_event` au statut `rejected` chez Claude, tour en échec avec `codexErrorInfo` à `usageLimitExceeded`, `rateLimitExceeded` ou `sessionBudgetExceeded` chez Codex ;
  - le superviseur parke l'agent, le fait passer au maillon suivant de sa chaîne, puis relance ce qui était en cours ;
  - en cas de changement de moteur, la session repart à neuf : un Agent repart de sa mémoire, un Worker de sa tâche (titre et détail renvoyés). Sinon, le dernier message est renvoyé.
  - Le changement est noté au journal de l'agent et signalé par l'événement `notice`. En bout de chaîne, ou sans profil, une notice prévient que l'agent attend.
- **Plafonds** (`[models.caps]`) :
  - `forbidden_for_workers` : un Worker reçoit le premier modèle permis de la chaîne, ou une erreur s'il n'y en a aucun ;
  - `max_parallel` (par défaut 4 sessions `claude-opus-5-5`) : un réveil de trop est refusé (`SupervisorError::Cap`) ; l'appelant le voit comme une erreur.
- **Protocole** : `AgentRecord.profile`, événement `notice { agent, text }`.
- **Tests** : 5 tests de la politique, 4 de bout en bout (repli d'un Worker vers Codex jusqu'au bout de chaîne, agent sans profil, plafond de sessions, modèle interdit), 1 test MCP du choix de profil, 2 unitaires de détection de quota.
