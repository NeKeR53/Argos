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
| T4 | Serveur MCP par niveau | Endpoint MCP, jetons par agent, outils L0 (gestion des Agents), L1 (Workers + board), L2 (compte rendu, escalade) | T3 |
| T5 | Contrats des agents | Prompts système par niveau : l'Assistant route des missions et résume ; l'Agent planifie avant d'exécuter ; le Worker fait une tâche et rend compte | T4 |
| T6 | Boards et missions | Mission → tâches, Worker lié à une tâche, repriorisation déterministe, remontée résumée | T4 |
| T7 | Mémoire | Dossier par agent (index, journal, notes, enfants), git local, digest chargé au démarrage, outil d'écriture mémoire, compaction avant saturation | T4 |
| T8 | Politique de modèles | Profils en TOML, résolution profil → moteur + modèle, repli sur quota épuisé, plafonds | T1 |
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
