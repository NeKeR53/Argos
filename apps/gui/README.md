# GUI Argos

App Tauri 2 : la console (fenêtre principale) et l'orbe (D38), plus la voix (D37). Le
cœur Rust est dans `src-tauri/`, le front React/TS dans `src/`.

```bash
npm run dev       # front seul dans un navigateur : ?daemon=<URL WebSocket encodée>
npx tauri dev     # app complète (daemon lancé à part, ARGOS_HOME partagé)
npm test          # vitest
```

## Réglages lus par la GUI

Dans le `config.toml` de `$ARGOS_HOME` (défaut `~/.argos`), à côté des sections du daemon :

```toml
[gui]
push_to_talk = "Alt+Space"          # maintenu pour parler à l'Assistant
console = "CmdOrCtrl+Shift+A"       # montre ou cache la console
panic = "CmdOrCtrl+Shift+X"         # panic stop : deux appuis en moins de 2 s
speak_typed_replies = false         # vrai : lire aussi les réponses aux demandes tapées

[voice]
whisper_model = "/chemin/vers/ggml-large-v3-turbo.bin"
whisper_server = "whisper-server"   # binaire whisper.cpp, cherché dans le PATH
language = "fr"
voice = "Thomas"                    # voix de `say` (`say -v '?'` pour la liste)
partial_interval_ms = 700
```

Les raccourcis se changent aussi depuis la console (⚙ Réglages → Raccourcis) : on clique
« Changer », on appuie la nouvelle combinaison, elle est active tout de suite et réécrite
dans `[gui]` sans toucher au reste du fichier. Une combinaison déjà prise par une autre app
est refusée et l'ancienne reste. Une lettre seule est refusée (elle volerait la frappe
partout) ; une touche F seule est permise, `F13` par exemple pour parler.

Le chemin du modèle n'est écrit nulle part dans le code : sans section `[voice]`, ou avec
un modèle introuvable, l'app tourne sans voix et dit pourquoi (pastille de l'orbe, avis et
badge de la console).

## Orbe

- Une pastille discrète (120×36 pt au repos), qui grandit seulement quand il se passe
  quelque chose : réflexion, écoute (ondes qui suivent la voix), parole (une bouche qui
  articule le texte que `say` prononce), confirmation du panic stop.
- Clic : console. Glisser : déplacer. Double-clic, ou clic droit puis « Remettre en
  place » : retour à la place par défaut (en bas au milieu de l'écran principal).
- La position est gardée dans `$ARGOS_HOME/gui-state.json`, et ramenée dans un écran si
  l'écran a changé.
- Sur macOS : ⚙ **Réglages → Globe** dans la console → **Globe animé dans le Dock**, ou clic
  droit sur l'orbe → **Placer dans le Dock**. Le globe animé remplace la pastille flottante
  et suit le repos, la réflexion, l'écoute et la parole, avec un badge pour les escalades
  et le panic stop. L'animation native reste active même quand la console est fermée.
  Un clic sur le globe du Dock ouvre la console ; **Globe → Globe flottant** rétablit
  la pastille à sa position gardée. Le choix est mémorisé dans `gui-state.json`.
- Démo sans daemon ni voix, dans un navigateur : `orb.html?demo=all` (ou `demo=listening`,
  `demo=speaking&t=1.2`…), servie par `npm run dev`.

## Voix

- Réservée à l'Assistant (D3) : la transcription d'un appui lui est envoyée comme un
  message tapé, et seules ses réponses sont lues.
- Par défaut, on lit la réponse à une demande faite à la voix ; une demande tapée n'est pas
  lue (sauf `speak_typed_replies = true`).
- Un appui pendant la lecture la coupe ; un appui pendant un tour de l'Assistant
  l'interrompt (barge-in).
- `whisper-server` est arrêté à la sortie de l'app, « Quitter Argos » compris.
- Essai sans micro, en dev seulement : le champ « Simuler la voix » de la console (commande
  `voice_simulate`, absente des builds de release) envoie une phrase comme si elle avait
  été dite.

## Permission micro (macOS)

- App empaquetée : `src-tauri/Info.plist` (fusionné par Tauri) porte
  `NSMicrophoneUsageDescription`, et `src-tauri/Entitlements.plist` le droit
  `com.apple.security.device.audio-input`, requis par le runtime renforcé à la signature.
  macOS demande l'accord au premier appui.
- En dev (`npx tauri dev`), le binaire n'est pas un bundle : macOS attribue l'accès au micro
  au process responsable, c'est-à-dire au terminal (ou à l'IDE) qui a lancé la commande.
  C'est lui qui doit avoir le droit dans Réglages Système → Confidentialité et sécurité →
  Micro. Sans ce droit, le premier appui échoue au bout de 5 s avec une erreur claire dans
  les avis de la console.

## Synthèse vocale locale Kokoro

Argos peut utiliser Kokoro-ONNX en français, sans service distant. Installer Python 3.12,
créer un environnement virtuel, puis installer `kokoro-onnx==0.6.1` et
`sounddevice==0.5.6`. Télécharger une fois les fichiers du modèle :

- [kokoro-v1.0.onnx](https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/kokoro-v1.0.onnx)
- [voices-v1.0.bin](https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/voices-v1.0.bin)

Dans `[voice]` du `config.toml`, ajouter les chemins locaux (adapter à la machine) :

```toml
tts_backend = "kokoro"
kokoro_python = "/chemin/vers/venv/bin/python"
kokoro_model = "/chemin/vers/kokoro-v1.0.onnx"
kokoro_voices = "/chemin/vers/voices-v1.0.bin"
kokoro_voice = "ff_siwis"
```

Sous Windows, le ZIP de release inclut les dépendances, les modèles et Whisper ; son
lanceur les configure au premier démarrage. Pour une installation manuelle, le Python
de l'environnement est `venv/Scripts/python.exe` ; utiliser des chemins absolus avec
`/` dans TOML. La version Windows reste à valider sur une machine réelle.
L'ancienne synthèse macOS reste disponible avec `tts_backend = "say"`. Redémarrer l'app
après modification. Le modèle est chargé à chaque phrase ; cela peut ajouter un délai.
