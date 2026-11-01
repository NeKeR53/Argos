# Canal Telegram (D43, D45)

Parler à l'Assistant depuis son téléphone, en texte ou en vocal, et y recevoir ce qu'il choisit d'y envoyer ainsi que les escalades. Telegram est le premier type de canal de discussion (D45). Code : `crates/argos-daemon/src/telegram/` pour le type, `crates/argos-daemon/src/channels.rs` pour les canaux. Décisions : [VISION.md](VISION.md), D43 et D45.

## Mise en route

1. Dans Telegram, écrire à **@BotFather** (compte officiel, coche bleue) : `/newbot`, un nom (« Argos »), un identifiant qui finit par `bot`. Il rend le jeton du bot.
2. Choisir l'identifiant du canal, ici `telegram`, et mettre le jeton dans `$ARGOS_HOME/<id>-token`, lisible par soi seul :

   ```bash
   install -m 600 /dev/null ~/.argos/telegram-token && pbpaste > ~/.argos/telegram-token
   ```

   La variable `ARGOS_<ID>_TOKEN` (`ARGOS_TELEGRAM_TOKEN` ici) a priorité sur le fichier ; `token_file` en change le chemin.
3. Ajouter le canal à `config.toml` et relancer le daemon : il affiche « canal telegram actif (telegram) ».

   ```toml
   [channels.telegram]
   type = "telegram"
   ```

4. Écrire au bot. Tant que `chat_id` n'est pas réglé, il répond avec l'identifiant à reporter :

   ```toml
   [channels.telegram]
   type = "telegram"
   chat_id = 123456789
   escalations = true     # signaler les escalades, options en boutons
   surfaces = true        # envoyer le lien de chaque nouvelle surface
   max_per_hour = 30      # envois de l'Assistant au-delà desquels le daemon refuse
   ```

5. Relancer le daemon. Les autres interlocuteurs sont ignorés, sans réponse.

Plusieurs bots font plusieurs canaux, chacun sa section. L'ancienne section `[telegram]` est encore lue comme `[channels.telegram]`, avec un avertissement au démarrage. Les réglages de la console montrent chaque canal : actif, en attente (`chat_id` manquant) ou désactivé (jeton introuvable), ses capacités et ses envois de l'heure.

Les vocaux demandent `ffmpeg` et `whisper-cli` (`brew install ffmpeg whisper-cpp`) ; ils sont cherchés dans le `PATH` puis dans `/opt/homebrew/bin`. Les réglages `ffmpeg` et `whisper_cli` du canal en donnent les chemins. Le modèle whisper, la langue et la synthèse viennent de la section `[voice]`, partagée avec la GUI : `whisper_model`, `language`, `tts_backend` (`say` avec `voice`, ou `kokoro` avec `kokoro_*`).

## Fonctionnement

- **Relevé** : long polling `getUpdates` (50 s), aucun port ouvert. La dernière nouveauté traitée est gardée dans `$ARGOS_HOME/<id>-offset` : un redémarrage ne rejoue rien. Un échec réseau est réessayé toutes les 5 s.
- **Texte** : remis à l'Assistant au nom de l'utilisateur, préfixé `[Canal <id>]`. `/start` reçoit un mot d'accueil.
- **Vocal** : téléchargé, converti en WAV 16 kHz par `ffmpeg`, transcrit par `whisper-cli` (environ 1 s avec large-v3-turbo), puis remis préfixé `[Canal <id>, message vocal transcrit]`. Un vocal vide ou illisible reçoit une réponse qui le dit.
- **Envois** : la réponse finale d'un tour ouvert depuis Telegram y revient d'elle-même, en vocal pour un vocal, sauf si l'Assistant a appelé `channel_send` pendant ce tour. Il s'en sert pour répondre ailleurs ou écrire de lui-même, et voit les canaux avec `channel_list`. Les outils d'Argos sont chargés d'emblée (`alwaysLoad`) : différés par Claude Code, l'Assistant n'en voyait que le nom et ne les appelait pas. En vocal, le texte est synthétisé (`say -o`, ou Kokoro vers un WAV), encodé en Ogg/Opus et envoyé, le texte en légende ; au-delà de 1 024 caractères, il suit dans un message séparé. Une synthèse en échec retombe sur le texte, et l'Assistant en est averti. Au-delà de `max_per_hour` envois dans l'heure glissante, le daemon refuse.
- **Orbe du Mac** : elle ne lit pas à voix haute la réponse à un message venu de Telegram, ni celles qui suivent les rapports d'Agents, tant que l'utilisateur ne revient pas à la console ou à l'orbe.
- **Photos et documents** : la plus grande taille d'une photo, ou le document, est téléchargé (10 Mo au plus, sinon refusé sans téléchargement) et remis à l'Assistant en pièce jointe, légende comprise, comme depuis la console (`argos_daemon::attachments`) : une image part en entrée native du moteur, un petit fichier texte est recopié, un autre (PDF…) est enregistré dans `$ARGOS_HOME/attachments/<assistant>/` et l'Assistant en reçoit le chemin.
- **Surfaces** : le lien de chaque nouvelle surface (version 1) est envoyé une fois, sans clé : seul un appareil associé l'ouvre (voir [PHASE-2](PHASE-2.md), « Dans un navigateur »). Rien n'est envoyé quand les surfaces n'écoutent que la boucle locale. `surfaces = false` coupe ces annonces.
- **Escalades** : chaque escalade ouverte est annoncée une fois, avec le nom de l'Agent et ses options en boutons. Un bouton répond avec l'option choisie ; citer le message de l'escalade y répond en texte libre ou en vocal.
- **Fichiers de travail** : `$ARGOS_HOME/<id>/`, un dossier par vocal, effacé après usage.
- **Jeton** : il fait partie de chaque URL de l'API ; les erreurs réseau sont rendues sans l'URL pour qu'il n'apparaisse dans aucun journal.

## Tests

- `cargo test -p argos-daemon --test telegram` : faux serveur Telegram (axum), faux `ffmpeg`, `whisper-cli` et `say`, faux Claude. Couvre le texte, le vocal, les photos et documents (trop lourds compris), l'annonce des nouvelles surfaces, le renvoi de la réponse, les envois de l'Assistant par `channel_send`, les inconnus, l'identifiant donné quand `chat_id` manque, les boutons et la réponse en citant.
- `real_speech_round_trip` (ignoré) : vrais programmes, synthèse puis transcription. `ARGOS_WHISPER_MODEL=… cargo test -p argos-daemon --test telegram real_speech -- --ignored` ; `ARGOS_TTS=kokoro` avec `ARGOS_KOKORO_PYTHON`, `ARGOS_KOKORO_MODEL` et `ARGOS_KOKORO_VOICES` essaie Kokoro. Vérifié le 2026-10-31 avec `say` et Kokoro.

## Limites et suites

- Les conversations avec un bot ne sont pas chiffrées de bout en bout : Telegram peut les lire.
- Ni vidéo, ni sticker, ni position : le bot répond qu'il ne lit pas encore ce type de message. Une photo ou un document vers un Assistant placé sur un autre nœud : seuls les images et le texte passent.
- Une escalade répondue ailleurs garde ses boutons sur Telegram ; un bouton périmé reçoit le refus du daemon.
- Le pont n'existe que sur le hub et suppose macOS pour `say`.
- Essayé le 2026-10-31 avec un vrai bot : identifiant donné à `/start`, puis aller-retour texte avec l'Assistant. Le vocal réel depuis le téléphone reste à essayer.
