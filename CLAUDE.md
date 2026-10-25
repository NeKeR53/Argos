# Argos

Assistant IA personnel qui délègue à une hiérarchie Assistant → Agents → Workers, chacun étant une vraie session `claude` ou `codex`.

## À lire avant de toucher au code

- [docs/VISION.md](docs/VISION.md) — décisions numérotées (D1…D39). Une décision remise en cause se modifie là, pas en douce dans le code.
- [docs/GLOSSARY.md](docs/GLOSSARY.md) — vocabulaire canonique. Code, docs et prompts d'agents utilisent ces termes.
- [docs/PHASE-1.md](docs/PHASE-1.md) — tranches en cours, choix d'implémentation et faits vérifiés sur les CLI.

## Structure

- `crates/argos-core` — types partagés daemon/GUI.
- `crates/argos-daemon` — binaire headless (sessions, contrôle, MCP, mémoire, API WS). Données dans `$ARGOS_HOME` (défaut `~/.argos`) ; utiliser un `ARGOS_HOME` jetable pour les essais.
- `apps/gui` — app Tauri 2 : `src-tauri/` (Rust, membre du workspace) et front React/TS (Vite).

## Commandes

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p argos-daemon --test real_engines -- --ignored   # vrais moteurs, consomme des tokens
cd apps/gui && npm run build      # front seul
cd apps/gui && npx tauri dev      # lance la GUI
```

## Conventions

- Identifiants en anglais, commentaires et docs en français.
- Les tests ne consomment pas de tokens : les moteurs sont remplacés par un faux CLI qui rejoue des sorties enregistrées.
- L'infrastructure exécute, elle ne décide pas : aucune logique d'orchestration en dur dans le daemon ou la GUI.
