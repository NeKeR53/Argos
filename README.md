# Argos

Argos est un assistant personnel qui organise le travail entre un assistant, des agents
durables et des workers temporaires. Il associe un daemon Rust à une application de
bureau Tauri et peut utiliser Claude Code ou Codex comme moteur d'agents.

Le projet est en **préversion** : les interfaces et les formats de configuration
peuvent évoluer. Les moteurs d'IA sont des logiciels distincts et nécessitent leur
propre installation et authentification ; leur utilisation peut entraîner des frais.

## Installer

Téléchargez une archive sur la page [Releases](https://github.com/NeKeR53/Argos/releases).

- **Windows x64** : extrayez entièrement le ZIP, puis lancez `Argos.cmd`. Le paquet
  inclut Whisper et Kokoro ainsi que leurs modèles pour la voix locale. WebView2 et le
  runtime Microsoft Visual C++ peuvent être nécessaires. Le paquet est non signé et
  n'a pas encore été validé sur une machine Windows réelle.
- **macOS Apple Silicon ou Intel** : extrayez le ZIP adapté à votre Mac et lancez
  `Lancer-Argos.command`. Le paquet est non signé et non notarié ; la voix nécessite
  une installation séparée de Whisper, Kokoro et de leurs modèles.

Dans tous les cas, installez et authentifiez séparément la CLI Claude Code ou Codex.
Consultez les instructions incluses dans chaque archive et le
[guide de compilation](docs/BUILD-BINARIES.md).

## Développer

Prérequis : Rust stable, Node.js, npm et les dépendances système de Tauri 2. Sur macOS,
installez aussi les outils de ligne de commande Xcode. Depuis la racine du dépôt :

```sh
cargo test --workspace
cargo run -p argos-daemon -- hub
```

Dans un autre terminal :

```sh
cd apps/gui
npm ci
npm run tauri dev
```

Le daemon et l'application utilisent `~/.argos` par défaut ou le dossier défini par
`ARGOS_HOME`. Utilisez un dossier distinct pour vos essais. La
[documentation de la GUI](apps/gui/README.md) décrit la configuration vocale et les
raccourcis ; [VISION](docs/VISION.md) et [GLOSSARY](docs/GLOSSARY.md) décrivent
l'architecture et le vocabulaire.

Les agents et workers utilisent par défaut les droits complets de l'utilisateur :
ils peuvent notamment exécuter des commandes et accéder au réseau. Choisissez le
réglage adapté dans la configuration et utilisez Argos dans un environnement de
confiance. Le code d'Argos, ses dépendances et les modèles distribués séparément
peuvent avoir des licences différentes.

## Participer

Les contributions sont bienvenues : consultez [CONTRIBUTING.md](CONTRIBUTING.md),
le [code de conduite](CODE_OF_CONDUCT.md) et la [politique de sécurité](SECURITY.md).
Argos est distribué sous [licence MIT](LICENSE).
