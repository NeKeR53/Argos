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
- **Agents isolés de la config globale de l'utilisateur.** Un `claude` lancé tel quel hérite des MCP, hooks, plugins et CLAUDE.md globaux (vu au test : ses outils incluaient ceux de l'utilisateur). Le daemon lance donc chaque session avec `--strict-mcp-config` et sa propre config, et Codex avec `--ignore-user-config`. Le détail (répertoire de config dédié, hooks) se règle en T1.

## Faits vérifiés sur les CLI (2026-10-25)

- **claude 2.1.310** : multi-tours dans un process via `--input-format stream-json` ; session-id dans le champ `session_id` ; fin de tour = ligne `type: result` (`result`, `total_cost_usd`, `usage`) ; événement `rate_limit_event` exploitable pour le repli ; transcripts dans `~/.claude/projects/<cwd encodé>/<session-id>.jsonl`. Flags utiles : `--resume`, `--session-id`, `--model`, `--mcp-config`, `--strict-mcp-config`, `--permission-mode`, `--append-system-prompt`, `--max-budget-usd`, `--include-partial-messages`.
- **codex-cli 0.164.0** : `exec --json` émet `thread.started` (`thread_id`), `turn.started`, `item.completed` (`item.text`), `turn.completed` (`usage`) ; `exec resume <thread_id>` reprend ; `-m`, `-s read-only|workspace-write|danger-full-access`, `-C`, `-c key=value`, `--skip-git-repo-check`, `--ignore-user-config`. `app-server` écoute en `stdio://`, `unix://` ou `ws://` et peut générer son schéma (`generate-json-schema`) : à étudier en T1 pour le mode persistant. Modèles vus : `gpt-6.1-sol` (défaut de la config), `gpt-6-luna`.
- **Toolchain** : rustc/cargo 1.96.1 via Homebrew (pas de rustup : il faudra l'installer en phase 4 pour la cross-compilation ARM/Linux), Node 26 + npm 11, Xcode présent. Pas de Docker (requis seulement sur les serveurs, phase 4).

## Tranches

Chaque tranche se termine par quelque chose qui tourne et se teste.

| # | Tranche | Contenu | Dépend de |
|---|---------|---------|-----------|
| T0 | Squelette | Workspace Cargo, crates vides, app Tauri qui démarre, `fmt` / `clippy` / `test` verts | — |
| T1 | Adaptateurs de moteur | Lancer une session `claude` et `codex`, envoyer des tours, lire le flux normalisé, interrompre, reprendre par session-id. Faux CLI pour les tests | T0 |
| T2 | Arbre et registre | Modèle agent (id, niveau, parent, moteur, modèle, état, charte), persistance SQLite, réhydratation au démarrage | T1 |
| T3 | Plan de contrôle + API WS | stop/kill du sous-arbre, park/resume, inspect ; API WebSocket (commandes + flux d'événements) | T2 |
| T4 | Serveur MCP par niveau | Endpoint MCP, jetons par agent, outils L0 (gestion des Agents), L1 (Workers + board), L2 (compte rendu, escalade) | T3 |
| T5 | Contrats des agents | Prompts système par niveau : l'Assistant route des missions et résume ; l'Agent planifie avant d'exécuter ; le Worker fait une tâche et rend compte | T4 |
| T6 | Boards et missions | Mission → tâches, Worker lié à une tâche, repriorisation déterministe, remontée résumée | T4 |
| T7 | Mémoire | Dossier par agent (index, journal, notes, enfants), git local, digest chargé au démarrage, outil d'écriture mémoire, compaction avant saturation | T4 |
| T8 | Politique de modèles | Profils en TOML, résolution profil → moteur + modèle, repli sur quota épuisé, plafonds | T1 |
| T9 | GUI console | Connexion WS, arbre des agents, chat avec l'Assistant, fenêtre d'Agent en chat texte, panic stop | T3 |
| T10 | Recette | Le scénario de réussite de bout en bout, avec de vrais moteurs | tout |

T5 à T9 sont largement parallélisables une fois T4 posé (T9 dès T3).
