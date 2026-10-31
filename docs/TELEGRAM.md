# Pont Telegram (D43)

Parler à l'Assistant depuis son téléphone, en texte ou en vocal, et y recevoir ses réponses et les escalades. Code : `crates/argos-daemon/src/telegram/`. Décision : [VISION.md](VISION.md), D43.

## Mise en route

1. Dans Telegram, écrire à **@BotFather** (compte officiel, coche bleue) : `/newbot`, un nom (« Argos »), un identifiant qui finit par `bot`. Il rend le jeton du bot.
2. Mettre ce jeton dans `$ARGOS_HOME/telegram-token`, lisible par soi seul :

   ```bash
   install -m 600 /dev/null ~/.argos/telegram-token && pbpaste > ~/.argos/telegram-token
   ```

   La variable `ARGOS_TELEGRAM_TOKEN` a priorité sur le fichier ; `token_file` dans `[telegram]` en change le chemin.
3. Ajouter `[telegram]` (vide) à `config.toml` et relancer le daemon : il affiche « pont Telegram actif ».
4. Écrire au bot. Tant que `chat_id` n'est pas réglé, il répond avec l'identifiant à reporter :

   ```toml
   [telegram]
   chat_id = 123456789
   voice_replies = true   # un vocal reçu appelle une réponse vocale
   escalations = true     # signaler les escalades, options en boutons
   ```

5. Relancer le daemon. Les autres interlocuteurs sont ignorés, sans réponse.

Les vocaux demandent `ffmpeg` et `whisper-cli` (`brew install ffmpeg whisper-cpp`) ; ils sont cherchés dans le `PATH` puis dans `/opt/homebrew/bin`. Les réglages `ffmpeg` et `whisper_cli` de `[telegram]` en donnent les chemins. Le modèle whisper, la langue et la synthèse viennent de la section `[voice]`, partagée avec la GUI : `whisper_model`, `language`, `tts_backend` (`say` avec `voice`, ou `kokoro` avec `kokoro_*`).

## Fonctionnement

- **Relevé** : long polling `getUpdates` (50 s), aucun port ouvert. La dernière nouveauté traitée est gardée dans `$ARGOS_HOME/telegram-offset` : un redémarrage ne rejoue rien. Un échec réseau est réessayé toutes les 5 s.
- **Texte** : remis à l'Assistant au nom de l'utilisateur, préfixé `[Depuis Telegram]`. `/start` reçoit un mot d'accueil.
- **Vocal** : téléchargé, converti en WAV 16 kHz par `ffmpeg`, transcrit par `whisper-cli` (environ 1 s avec large-v3-turbo), puis remis préfixé `[Depuis Telegram, message vocal transcrit]`. Un vocal vide ou illisible reçoit une réponse qui le dit.
- **Réponses** : chaque message de l'Assistant part sur Telegram tant que le dernier message de l'utilisateur en venait ; un message tapé dans la console ou dit à l'orbe les ramène à la console. Après un vocal, la réponse est synthétisée (`say -o`, ou Kokoro vers un WAV), encodée en Ogg/Opus et envoyée en vocal, le texte en légende. Au-delà de 1 024 caractères, le texte suit dans un message séparé. Une synthèse en échec retombe sur le texte.
- **Escalades** : chaque escalade ouverte est annoncée une fois, avec le nom de l'Agent et ses options en boutons. Un bouton répond avec l'option choisie ; citer le message de l'escalade y répond en texte libre ou en vocal.
- **Fichiers de travail** : `$ARGOS_HOME/telegram/`, un dossier par vocal, effacé après usage.
- **Jeton** : il fait partie de chaque URL de l'API ; les erreurs réseau sont rendues sans l'URL pour qu'il n'apparaisse dans aucun journal.

## Tests

- `cargo test -p argos-daemon --test telegram` : faux serveur Telegram (axum), faux `ffmpeg`, `whisper-cli` et `say`, faux Claude. Couvre le texte, le vocal, le retour à la console, les inconnus, l'identifiant donné quand `chat_id` manque, les boutons et la réponse en citant.
- `real_speech_round_trip` (ignoré) : vrais programmes, synthèse puis transcription. `ARGOS_WHISPER_MODEL=… cargo test -p argos-daemon --test telegram real_speech -- --ignored` ; `ARGOS_TTS=kokoro` avec `ARGOS_KOKORO_PYTHON`, `ARGOS_KOKORO_MODEL` et `ARGOS_KOKORO_VOICES` essaie Kokoro. Vérifié le 2026-10-31 avec `say` et Kokoro.

## Limites et suites

- Les conversations avec un bot ne sont pas chiffrées de bout en bout : Telegram peut les lire.
- Ni photo ni document : le bot répond qu'il ne lit que le texte et les vocaux.
- Une escalade répondue ailleurs garde ses boutons sur Telegram ; un bouton périmé reçoit le refus du daemon.
- Le pont n'existe que sur le hub et suppose macOS pour `say`.
- Pas encore essayé avec un vrai bot : à faire à la première mise en route.
