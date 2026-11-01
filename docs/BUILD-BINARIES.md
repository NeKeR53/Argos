# Construire et publier Argos

Le workflow GitHub Actions **Build and publish release** se déclenche à la réception
d'un tag tel que `v0.1.0`. La version du tag doit correspondre aux versions dans
`Cargo.toml`, `apps/gui/package.json` et `apps/gui/src-tauri/tauri.conf.json`.
Il crée une release en brouillon, construit les archives sur macOS Apple Silicon,
macOS Intel, Windows x64 et Linux x86_64, puis publie la release uniquement lorsque
les quatre archives et leurs sommes SHA-256 sont disponibles. Les versions `0.x` et les tags
avec suffixe sont publiés comme préversions. En cas d'échec, le brouillon reste
disponible et l'exécution peut être relancée depuis GitHub Actions.
Si seules les étapes finales ont échoué et que les huit fichiers sont déjà présents,
le workflow manuel **Publish completed release draft** vérifie ces fichiers et publie
le brouillon sans reconstruire les archives.

```sh
git tag -a v0.1.0 -m 'Argos v0.1.0'
git push origin v0.1.0
```

Les archives ne sont pas signées ; celles de macOS ne sont pas notariées. Windows
et Linux doivent encore être validés sur une machine réelle. Claude ou Codex doit être installé
et authentifié séparément sur chaque plateforme.

Le workflow manuel **Build binaries** permet également de construire des artefacts
sans publier de release (`macos`, `windows`, `linux` ou `all`).

## macOS Apple Silicon et Intel

Avec Xcode Command Line Tools, Rust, Node.js et npm :

```sh
./scripts/package-macos.sh
```

Le dossier `target/distribution/macos-arm64` ou `macos-x86_64` contient le ZIP
adapté à l'architecture et sa somme SHA-256. Extraire le ZIP et lancer
`Lancer-Argos.command` pour démarrer le daemon puis l'application. La voix locale,
Python et ses modèles ne sont pas inclus dans l'archive macOS ; voir
[la configuration de la GUI](../apps/gui/README.md).

## Windows x64

Avec Visual Studio C++ Build Tools, WebView2, Rust, Node.js, npm et Python 3.12 x64 :

```powershell
./scripts/package-windows.ps1
```

Le dossier `target/distribution/windows-x64` contient `Argos-Windows-x64.zip`
et sa somme SHA-256. Extraire entièrement l'archive puis double-cliquer sur
`Argos.cmd` : il démarre le daemon et l'application et configure la voix locale
embarquée au premier lancement. Le ZIP contient Python, Whisper, Kokoro et leurs
modèles français. Les données sont conservées dans `%LOCALAPPDATA%\Argos\data`.
WebView2 et le [redistribuable Microsoft Visual C++ x64](https://learn.microsoft.com/fr-fr/cpp/windows/latest-supported-vc-redist)
peuvent être nécessaires.

Le daemon et Whisper utilisent des jobs Windows pour arrêter les processus descendants ;
le jeton est créé avec une ACL limitée à son propriétaire. Codex utilise un lien
matériel vers `auth.json` quand ce fichier existe : le dossier utilisateur et les
données Argos doivent alors se trouver sur le même volume NTFS.

## Linux x86_64

Sur Ubuntu 22.04 ou plus récent, avec Rust, Node.js, npm et les dépendances de Tauri 2 :

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libasound2-dev libssl-dev libfuse2 file
./scripts/package-linux.sh
```

Le dossier `target/distribution/linux-x86_64` contient `Argos-Linux-x86_64.tar.gz`
et sa somme SHA-256. L'archive contient un dossier `Argos/` avec l'application en
AppImage, le daemon, `lancer-argos.sh` et un `LISEZ-MOI.txt`. Le lanceur démarre le
daemon s'il n'écoute pas déjà sur `127.0.0.1:7878`, puis ouvre l'AppImage. Comme sur
macOS, la voix locale n'est pas incluse ; le globe dans le Dock et la synthèse `say`
(canal Telegram en vocal) sont propres à macOS.
