# Phase 3 — Voix et overlay

Objectif (D37, D38, D16) : parler à l'Assistant en maintenant une touche et l'entendre répondre, en local et en streaming. Une orbe toujours présente montre son état, et un raccourci arrête tout.

Réglages et permission micro : voir [apps/gui/README.md](../apps/gui/README.md).

## Moteur vocal (`crates/argos-voice`)

- **STT** : `whisper-server` (whisper.cpp 1.9.4, Homebrew) lancé par la crate avec le modèle `large-v3-turbo` gardé en mémoire.
  - Pendant l'appui, des transcriptions partielles toutes les 700 ms ; la finale au relâchement.
  - Le micro passe par `cpal`. Une erreur claire arrive après 5 s si macOS refuse le micro.
- **TTS** : `say -v Thomas`, phrase par phrase. La première phrase complète est dite pendant que la suite arrive encore.
- **Barge-in** : un nouvel appui coupe la lecture.
- **Garde contre les orphelins** : `whisper-server` tourne sous une petite garde `sh`, dans un groupe de process à part. La garde le termine dès que l'app disparaît, même tuée par SIGKILL (macOS n'a pas `PR_SET_PDEATHSIG`) ; le groupe s'arrête d'un coup à la sortie normale. Ce problème a été trouvé pendant la vérification : chaque relance de `tauri dev` laissait un serveur.
- **Mesures réelles** (2026-10-27) : finale 334 ms après le relâchement ; premier `say` lancé 132 ms après le premier fragment ; `say` coupé 20 ms après l'arrêt.

## App (`apps/gui`)

- **Orbe** : fenêtre à part, toujours au premier plan, aux états suivants :
  - écoute, avec le niveau du micro ;
  - parole, avec un halo animé (`say` ne donne pas de niveau) ;
  - sinon, l'état de l'Assistant.
- **Raccourcis globaux** : push-to-talk `Alt+Space` maintenu, console `Cmd+Shift+A`, panic stop `Cmd+Shift+X` (deux appuis en moins de 2 s).
- **Garde des navigations** : une surface ne peut pas faire naviguer la fenêtre vers une URL externe.
- **Boucle vocale** :
  1. appui : `press()`, et interruption de l'Assistant s'il est en plein tour ;
  2. relâchement : `release()`, puis la transcription part vers l'Assistant seul (D3), comme un message tapé, marquée « à la voix » dans son fil ;
  3. ses `message_delta` sont lus au fil de l'eau, et `turn_completed` termine la lecture.
  - Seules les réponses aux demandes faites à la voix sont lues, sauf `speak_typed_replies = true`.
- **Sans voix** : sans section `[voice]`, ou avec un modèle introuvable, l'app tourne sans voix et dit pourquoi.
- **Sortie** : « Quitter Argos » et SIGTERM, SIGHUP ou SIGINT arrêtent la voix.
- **Essai sans micro**, en debug seulement : commande `voice_simulate`, champ « Simuler la voix » de la console, ou `ARGOS_VOICE_SIMULATE="phrase"` au lancement.

## Vérifié

- `npx tauri dev` contre un vrai daemon, Assistant sur Haiku, phrase simulée « Dis-moi bonjour en une phrase ». Les fragments de la réponse partent à la lecture et `say -v Thomas -- Bonjour, je suis là si vous avez besoin de quelque chose.` a été observé pendant 2 s. `whisper-server` a disparu après l'arrêt de l'app.
- Tests : 84 vitest (lecture voix ou texte, états de l'orbe, protocole) et des tests Rust (configuration absente ou invalide, garde des orphelins).

## Pas vérifié

- Le vrai micro, le raccourci maintenu, l'écoute réelle et ses niveaux, le barge-in par un vrai appui : le terminal n'a pas la permission micro. **À essayer de ton côté** : donner le micro au terminal, lancer le daemon puis `npx tauri dev`, maintenir `Alt+Space`.
- Sur l'un des trois essais, la réponse n'a pas été dite : les fragments sont partis, mais `finish_speech` n'est jamais arrivé. C'était probablement pendant une relance de l'app par `tauri dev`. Ce n'est pas reproduit et la cause n'est pas établie.
- Le rendu visuel de l'orbe et de la console dans l'app Tauri : pas de permission de capture d'écran.
