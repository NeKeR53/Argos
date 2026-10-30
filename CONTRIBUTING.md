# Contribuer à Argos

Les rapports de bugs, améliorations de documentation et propositions de code sont
les bienvenus. La langue du projet est le français ; les identifiants du code sont en
anglais. Pour un changement important, ouvrez d'abord une issue pour en discuter.
Pour une vulnérabilité, suivez plutôt [SECURITY.md](SECURITY.md).

1. Forkez le dépôt et créez une branche depuis `main`.
2. Décrivez le problème et apportez un changement ciblé, avec des tests adaptés.
3. Vérifiez vos changements localement, puis ouvrez une pull request vers `main`.
   Indiquez son objectif, la manière de la tester et les limites connues.

Commandes utiles depuis la racine :

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps/gui && npm ci && npm test && npm run build
```

Les tests ordinaires utilisent des moteurs simulés. Certains tests marqués `ignored`
utilisent de vrais services, du matériel audio ou des crédits : ne les activez que
volontairement. Pour les builds natifs, consultez [docs/BUILD-BINARIES.md](docs/BUILD-BINARIES.md).

Ne joignez pas de secrets, jetons, conversations privées ou données personnelles aux
issues, pull requests et journaux. En soumettant une contribution, vous acceptez sa
distribution sous la [licence MIT](LICENSE) et confirmez pouvoir la proposer sous
cette licence. Respectez le [code de conduite](CODE_OF_CONDUCT.md).
