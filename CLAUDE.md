# Argos

Assistant IA personnel qui délègue à une hiérarchie Assistant → Agents → Workers, chacun étant une vraie session `claude` ou `codex`.

## À lire avant de toucher au code

- [docs/VISION.md](docs/VISION.md) — décisions numérotées (D1…D40). Une décision remise en cause se modifie là, pas en douce dans le code.
- [docs/GLOSSARY.md](docs/GLOSSARY.md) — vocabulaire canonique. Code, docs et prompts d'agents utilisent ces termes.
- [docs/PHASE-1.md](docs/PHASE-1.md) — tranches en cours, choix d'implémentation et faits vérifiés sur les CLI.
- [docs/PHASE-2.md](docs/PHASE-2.md), [docs/PHASE-4.md](docs/PHASE-4.md), [docs/PHASE-5.md](docs/PHASE-5.md) — surfaces, multi-nœud, événements : ce qui est fait, les écarts avec la vision, ce qui reste.

## Structure

- `crates/argos-core` — types partagés daemon/GUI.
- `crates/argos-daemon` — binaire headless (sessions, contrôle, MCP, mémoire, API WS). Données dans `$ARGOS_HOME` (défaut `~/.argos`) ; utiliser un `ARGOS_HOME` jetable pour les essais. `argos-daemon node` le lance en nœud d'exécution (phase 4).
- `crates/argos-voice` — moteur vocal local (whisper-server, `say`, micro).
- `apps/gui` — app Tauri 2 : `src-tauri/` (Rust, membre du workspace) et front React/TS (Vite).

## Commandes

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p argos-daemon --test real_engines -- --ignored   # vrais moteurs, consomme des tokens
ARGOS_HOME=/tmp/argos-essai ARGOS_CLAUDE_BIN=$PWD/crates/argos-daemon/tests/fixtures/fake-claude \
  ARGOS_CODEX_BIN=$PWD/crates/argos-daemon/tests/fixtures/fake-codex cargo run -p argos-daemon   # daemon sur faux CLI
cd apps/gui && npm run build      # front seul
cd apps/gui && npx tauri dev      # lance la GUI
```

## Conventions

- Identifiants en anglais, commentaires et docs en français.
- Les tests ne consomment pas de tokens : les moteurs sont remplacés par un faux CLI qui rejoue des sorties enregistrées.
- L'infrastructure exécute, elle ne décide pas : aucune logique d'orchestration en dur dans le daemon ou la GUI.
