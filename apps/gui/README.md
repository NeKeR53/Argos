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
whisper_model = "/Users/alexandre/.cache/short-editing/ggml-large-v3-turbo.bin"
whisper_server = "whisper-server"   # binaire whisper.cpp, cherché dans le PATH
language = "fr"
voice = "Thomas"                    # voix de `say` (`say -v '?'` pour la liste)
partial_interval_ms = 700
```

Les raccourcis se changent aussi depuis la console (bouton « Raccourcis ») : on clique
« Changer », on appuie la nouvelle combinaison, elle est active tout de suite et réécrite
dans `[gui]` sans toucher au reste du fichier. Une combinaison déjà prise par une autre app
est refusée et l'ancienne reste. Une lettre seule est refusée (elle volerait la frappe
partout) ; une touche F seule est permise, `F13` par exemple pour parler.

Le chemin du modèle n'est écrit nulle part dans le code : sans section `[voice]`, ou avec
un modèle introuvable, l'app tourne sans voix et dit pourquoi (pastille de l'orbe, avis et
badge de la console).

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
