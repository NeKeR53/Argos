# Argos

Argos est un assistant personnel qui organise le travail entre un **Assistant**, des
**Agents** durables et des **Workers** temporaires. Chacun est une vraie session
[Claude Code](https://docs.claude.com/claude-code) ou [Codex](https://github.com/openai/codex).
Un daemon Rust tient la hiérarchie, la mémoire et les événements ; une application de
bureau Tauri en est la console, avec une orbe vocale toujours à portée.

```
Vous ──▶ Assistant ──▶ Agent « Domotique » ──▶ Workers (une tâche chacun)
              │    └──▶ Agent « Veille »    ──▶ Workers
              └── déclencheurs, canaux (Telegram), surfaces, fonctions publiées
```

- **L'Assistant** est votre unique interlocuteur. Il ne fait pas le travail lui-même :
  il crée des Agents, leur confie des missions et vous rend compte.
- **Un Agent** est durable : il a un domaine, un répertoire, une mémoire, un board de
  missions. Il dort quand il n'a rien à faire et se réveille sur un message ou un
  déclencheur.
- **Un Worker** exécute une seule tâche pour son Agent, puis disparaît.

Le vocabulaire exact est dans [GLOSSARY](docs/GLOSSARY.md), les décisions d'architecture
dans [VISION](docs/VISION.md).

> **Préversion.** Les interfaces et les formats de configuration peuvent évoluer. Les
> moteurs d'IA sont des logiciels distincts, à installer et authentifier séparément ;
> leur utilisation peut entraîner des frais. Par défaut, les Agents et les Workers ont
> les droits complets de l'utilisateur (commandes, réseau) : utilisez Argos dans un
> environnement de confiance.

## Sommaire

- [Installation](#installation)
- [Premier lancement et configuration](#premier-lancement-et-configuration)
- [Déclencheurs](#déclencheurs)
- [Déclencheurs personnalisés](#déclencheurs-personnalisés)
- [Autres fonctions](#autres-fonctions)
- [Développer](#développer)
- [Documentation](#documentation)

## Installation

### 1. Installer un moteur

Argos pilote des sessions `claude` ou `codex` : au moins l'une des deux CLI doit être
installée, authentifiée et accessible dans le `PATH`.

```bash
npm install -g @anthropic-ai/claude-code && claude    # puis se connecter
npm install -g @openai/codex && codex login
```

Vérifiez qu'une session démarre dans un terminal avant de lancer Argos. Le moteur et le
modèle de l'Assistant se choisissent dans `config.toml` (voir plus bas).

### 2. Télécharger Argos

Les archives sont sur la page [Releases](https://github.com/NeKeR53/Argos/releases) :

| Système | Archive | Contenu |
| --- | --- | --- |
| macOS Apple Silicon | `Argos-macOS-arm64.zip` | `Argos.app`, `argos-daemon`, `Lancer-Argos.command` |
| macOS Intel | `Argos-macOS-x86_64.zip` | idem |
| Windows x64 | `Argos-Windows-x64.zip` | `Argos.exe`, `argos-daemon.exe`, `Argos.cmd`, voix locale complète |
| Linux x86_64 | `Argos-Linux-x86_64.tar.gz` | `Argos.AppImage`, `argos-daemon`, `lancer-argos.sh` |

Chaque archive est accompagnée d'un fichier `SHA256SUMS-…txt`. Pour vérifier :

```bash
shasum -a 256 -c SHA256SUMS-macos-arm64.txt      # macOS
sha256sum -c SHA256SUMS-linux-x86_64.txt         # Linux
```

```powershell
Get-FileHash -Algorithm SHA256 .\Argos-Windows-x64.zip   # Windows, à comparer au fichier
```

Les archives ne sont **pas signées** (ni notariées sur macOS).

### 3. Lancer

**macOS**

1. Décompressez le ZIP dans un dossier conservé (par exemple `~/Applications/Argos`).
2. L'archive n'étant pas notariée, retirez la quarantaine une fois :

   ```bash
   xattr -dr com.apple.quarantine ~/Applications/Argos
   ```

3. Double-cliquez sur `Lancer-Argos.command` : il démarre le daemon en arrière-plan s'il
   ne tourne pas déjà (journal dans `$TMPDIR/argos-daemon.log`), puis ouvre `Argos.app`.
4. Au premier appui sur le raccourci vocal, macOS demande l'accès au micro.

La voix locale (Whisper, et Kokoro ou `say`) s'installe à part, par exemple
`brew install whisper-cpp` et un modèle `ggml-*.bin` : voir
[la documentation de la GUI](apps/gui/README.md#réglages-lus-par-la-gui).

**Windows**

1. Extrayez **entièrement** le ZIP dans un dossier conservé.
2. Double-cliquez sur `Argos.cmd` : il démarre le daemon puis l'application, et configure
   la voix locale au premier lancement.
3. Python, Whisper, Kokoro et leurs modèles français sont inclus. WebView2 et le
   [redistribuable Visual C++ x64](https://learn.microsoft.com/fr-fr/cpp/windows/latest-supported-vc-redist)
   peuvent être nécessaires.

Les données vont dans `%LOCALAPPDATA%\Argos\data`. Pour arrêter le daemon, terminez
`argos-daemon.exe` dans le Gestionnaire des tâches. Windows n'a pas encore été validé
sur une machine réelle.

**Linux**

1. Installez FUSE 2 pour l'AppImage (`sudo apt install libfuse2`, ou `libfuse2t64` sur
   Ubuntu 24.04) et WebKitGTK si votre distribution ne l'a pas.
2. Extrayez et lancez :

   ```bash
   tar -xzf Argos-Linux-x86_64.tar.gz
   ./Argos/lancer-argos.sh
   ```

   Sans FUSE : `./Argos/Argos.AppImage --appimage-extract-and-run` après avoir démarré
   `./Argos/argos-daemon hub`.

La voix locale s'installe à part comme sur macOS ; le globe dans le Dock et la synthèse
`say` sont propres à macOS. Linux n'a pas encore été validé sur une machine réelle.

### 4. Faire tourner le daemon en service (facultatif)

Le daemon peut tourner en permanence, indépendamment de l'application — c'est utile pour
que les déclencheurs et Telegram fonctionnent fenêtre fermée. Il sait écrire son fichier
de service, sans l'installer :

```bash
./argos-daemon service launchd hub > ~/Library/LaunchAgents/argos-hub.plist
launchctl load ~/Library/LaunchAgents/argos-hub.plist             # macOS
```

```bash
mkdir -p ~/.config/systemd/user
./argos-daemon service systemd hub > ~/.config/systemd/user/argos-hub.service
systemctl --user enable --now argos-hub                                # Linux
```

Le fichier pointe vers le binaire à son emplacement actuel : gardez-le à cet endroit.

## Premier lancement et configuration

Au premier démarrage, le daemon crée `~/.argos` (ou le dossier de `ARGOS_HOME`) :

| Fichier | Rôle |
| --- | --- |
| `config.toml` | configuration commentée, à éditer puis relancer le daemon |
| `argos.db` | registre : agents, missions, déclencheurs, mémoire, journal |
| `assistant/`, `agents/` | répertoires de travail des sessions |
| `gui-state.json` | position de l'orbe et préférences de la console |

Les réglages principaux de `config.toml` :

```toml
[assistant]
engine = "claude"            # ou "codex"
model = "claude-opus-5-5"

[api]
listen = "127.0.0.1:7878"    # API WebSocket de la console

[mcp]
listen = "127.0.0.1:7879"    # outils des agents et webhooks des déclencheurs

[workers]
access = "full"              # ou "workspace" : écriture dans leur répertoire, sans réseau

[board]
preempt = false              # une mission passée en tête met au repos les Workers moins prioritaires
```

S'y ajoutent `[gui]` et `[voice]` (raccourcis, voix, modèle Whisper, voir
[apps/gui/README.md](apps/gui/README.md)), `[channels.*]` (canaux de discussion, voir
[TELEGRAM](docs/TELEGRAM.md)) et `[nodes.*]` (autres machines, voir
[PHASE-4](docs/PHASE-4.md)). Pour un essai, utilisez un dossier jetable :
`ARGOS_HOME=/tmp/argos-essai`.

Ensuite, parlez à l'Assistant — par écrit dans la console, à la voix en maintenant
`Alt+Espace`, ou depuis Telegram — et demandez-lui ce que vous voulez déléguer.

## Déclencheurs

Un **déclencheur** réveille un agent sans que vous lui parliez : à chaque événement, sa
**consigne** est remise à l'agent cible, même s'il était au repos. C'est ce qui rend un
Agent proactif : un point du matin, une surveillance, une réaction à la maison connectée.

### Les sources disponibles

| Source | Paramètre | Se déclenche quand… | Exemple |
| --- | --- | --- | --- |
| `every` | `every_minutes` (1 au moins) | l'intervalle est écoulé depuis le dernier déclenchement | « toutes les 30 min, regarde si le build de nuit est vert » |
| `cron` | expression à 5 champs, heure locale | l'heure correspond | `0 8 * * 1-5` : chaque jour ouvré à 8 h |
| `watch` | dossier absolu existant | un fichier y est créé ou déposé | factures déposées dans `~/Documents/Factures` |
| `webhook` | aucun (un secret est généré) | un `POST` arrive sur son URL | automatisation Home Assistant, script, service externe |
| `switch` | aucun | vous l'actionnez depuis la console | « pause café », « je pars du bureau » |

Règles communes :

- Seuls l'Assistant et les Agents créent et reçoivent des déclencheurs ; un Agent n'en
  crée que pour lui-même, l'Assistant peut en créer pour l'un de ses Agents.
- Après une absence (Mac en veille, daemon arrêté), un déclencheur périodique ne part
  **qu'une fois**, sans rafale de rattrapage.
- Réactiver un déclencheur suspendu le réarme : il attend sa prochaine échéance.
- Un même fichier dans un dossier surveillé n'est signalé qu'une fois sur 3 s.
- Retirer un agent supprime ses déclencheurs.
- Chaque réveil est un tour de modèle, donc des tokens : préférez la fréquence la plus
  basse qui suffit.

### Créer un déclencheur

Il suffit de le demander en langage naturel ; l'Assistant ou l'Agent concerné appelle
l'outil `trigger_create` :

> « Chaque lundi à 9 h, fais-moi un résumé des issues GitHub ouvertes la semaine passée. »
>
> « Crée un interrupteur "je pars" : quand je l'actionne, vérifie que les lumières et le
> chauffage sont éteints. »
>
> « Donne-moi un webhook pour que Home Assistant te prévienne quand la porte d'entrée
> s'ouvre la nuit. »

Les outils exposés aux agents :

| Outil | Arguments |
| --- | --- |
| `trigger_create` | `name`, `source`, `prompt`, puis selon la source `every_minutes`, `cron` ou `path` ; `target` (Assistant seulement) |
| `trigger_list` | — : les déclencheurs dont l'agent est propriétaire ou cible |
| `trigger_set` | `trigger`, `status` (`on` / `off`) |
| `trigger_delete` | `trigger` |

Pour un webhook, la réponse contient l'URL complète à transmettre au service appelant.

### Ce que reçoit l'agent

La consigne arrive au nom d'Argos, préfixée par le nom, l'identifiant et la source du
déclencheur, suivie du contenu de l'événement :

```
[Déclencheur « Porte d'entrée » (tr-3f9a…, webhook)]
Si la porte s'ouvre entre 23 h et 6 h, préviens-moi sur Telegram.

Reçu :
{"entity":"binary_sensor.porte_entree","state":"on","time":"02:14"}
```

Pour `watch`, le contenu est `Nouveau fichier : /chemin/du/fichier`. Les agents sont
instruits de traiter la consigne brièvement et de ne vous déranger que si elle le justifie.

### Dans la console

Le panneau **Déclencheurs** d'un agent liste ceux qu'il a créés et ceux qui le visent,
avec leur source, leur paramètre, la date du dernier déclenchement et la consigne. Vous
pouvez **Actionner** (remettre la consigne tout de suite : c'est ainsi qu'on bascule un
interrupteur), **Suspendre / Activer**, **Supprimer**, et copier l'URL d'un webhook.

### Webhooks en détail

```
POST http://127.0.0.1:7879/hooks/<id du déclencheur>/<secret>
```

- Le corps, quel qu'il soit (JSON, texte), accompagne la consigne, tronqué à 20 000
  caractères. Un appel sans corps est remis comme « (appel sans contenu) ».
- Réponse `202 Accepted` quand l'événement est pris en compte ; `404` pour un
  déclencheur inconnu, suspendu, d'une autre source ou un mauvais secret.
- L'adresse est celle de `[mcp] listen`, **locale par défaut**. Le secret fait partie de
  l'URL : traitez-la comme un mot de passe.

```bash
curl -X POST --data 'Sauvegarde terminée : 12 Go, 0 erreur' \
  http://127.0.0.1:7879/hooks/tr-3f9a.../<secret>
```

## Déclencheurs personnalisés

Argos n'exécute pas encore de script de surveillance écrit par un agent (la « soupape »
de D30 attend un vrai bac à sable). Mais le **webhook** et le **dossier surveillé** font
de n'importe quel programme une source d'événements : la condition vit dans votre script,
qui ne coûte rien tant qu'il ne se passe rien, et l'agent n'est réveillé que lorsqu'il y
a vraiment quelque chose à traiter. C'est souvent mieux qu'un `every` qui réveille l'agent
pour constater que tout va bien.

Dans tous les cas : demandez d'abord à l'agent un déclencheur `webhook` (ou `watch`) avec
la consigne qui dit quoi faire de l'événement, puis branchez votre source dessus.

### Un script local

Exemple : prévenir l'Agent « Système » seulement quand le disque dépasse 90 %.

```bash
#!/usr/bin/env bash
# ~/bin/argos-disque.sh — lancé toutes les 10 min par cron, launchd ou un timer systemd
hook="http://127.0.0.1:7879/hooks/tr-xxxx/SECRET"
use=$(df -P / | awk 'NR==2 {gsub("%","",$5); print $5}')
if [ "$use" -ge 90 ]; then
  curl -fsS -X POST -H 'Content-Type: application/json' \
    --data "{\"disque\":\"/\",\"utilisation\":$use}" "$hook"
fi
```

```cron
*/10 * * * * ~/bin/argos-disque.sh
```

Le même schéma marche pour un prix qui franchit un seuil, une page web qui change, un
service qui ne répond plus, la fin d'un long calcul (`make && curl … || curl …`), un
hook Git (`post-receive`), etc. Stockez l'URL hors du dépôt (fichier `0600`, variable
d'environnement).

### Home Assistant

Déclarez un `rest_command` qui appelle le webhook, puis utilisez-le dans une automatisation :

```yaml
# configuration.yaml
rest_command:
  argos_porte:
    url: !secret argos_hook_porte        # http://127.0.0.1:7879/hooks/tr-xxxx/SECRET
    method: POST
    content_type: "application/json"
    payload: '{"entity":"{{ entity }}","state":"{{ state }}"}'

# automations.yaml
- alias: Porte ouverte la nuit → Argos
  trigger:
    - platform: state
      entity_id: binary_sensor.porte_entree
      to: "on"
  condition:
    - condition: time
      after: "23:00:00"
      before: "06:00:00"
  action:
    - service: rest_command.argos_porte
      data:
        entity: binary_sensor.porte_entree
        state: "on"
```

Pour l'action inverse — qu'un Agent allume une lumière sans tour de modèle — voir les
[fonctions publiées](docs/FUNCTIONS.md).

### Une source sur une autre machine

Le daemon n'écoute qu'en local. Plutôt que d'exposer `[mcp] listen` (qui sert aussi les
outils des agents), ouvrez un tunnel SSH inverse depuis le hub vers la machine qui
appelle : elle verra le webhook sur son propre `127.0.0.1:7879`.

```bash
ssh -N -R 7879:127.0.0.1:7879 homeassistant
```

Pour un service Internet (GitHub, un formulaire, IFTTT…), placez un reverse proxy ou un
tunnel (Tailscale Funnel, Cloudflare Tunnel) qui ne transmet **que** le chemin
`/hooks/`, en HTTPS.

### Un dossier comme boîte aux lettres

Un déclencheur `watch` transforme un dossier en file d'entrée : tout ce qui y dépose un
fichier réveille l'agent avec son chemin. Un scanner qui enregistre ses PDF, un
téléchargement, un export nocturne, ou un script qui écrit un rapport :

```bash
date > ~/Argos-inbox/rapport-$(date +%F-%H%M).txt
```

La consigne dit quoi faire du fichier (« classe-le », « résume-le », « extrais le
montant »).

### Un bouton dans une surface

Une [surface](docs/PHASE-2.md) (page HTML hébergée par l'app) peut envoyer un événement
à son agent avec `argos.emit('nom', données)` : un tableau de bord avec ses propres
boutons et formulaires, sans passer par un déclencheur.

### Pas encore disponible

Connecteur email (IMAP/Gmail), MQTT natif, calendrier et script de surveillance
sandboxé écrit par l'agent. Un déclencheur tourne aujourd'hui sur le hub, même pour un
Agent placé sur un autre nœud. Le détail est dans [PHASE-5](docs/PHASE-5.md).

## Autres fonctions

- **Voix** : maintenir `Alt+Espace` pour parler à l'Assistant ; transcription Whisper
  locale, réponse lue par `say` ou Kokoro, interruption possible. Mot de déclenchement
  pour l'écoute mains libres. Voir [apps/gui/README.md](apps/gui/README.md) et
  [PHASE-3](docs/PHASE-3.md).
- **Orbe** : pastille flottante (ou globe animé dans le Dock sur macOS) qui montre si
  l'Assistant réfléchit, écoute ou parle.
- **Canaux** : parler à l'Assistant depuis Telegram, en texte ou en vocal ; il répond là
  où on lui a parlé et y signale les escalades. Voir [TELEGRAM](docs/TELEGRAM.md).
- **Surfaces** : pages HTML créées par les agents, cockpits vivants ou résultats figés.
  Elles s'ouvrent aussi dans un navigateur : <http://127.0.0.1:7890/> sur le Mac, ou
  depuis un autre appareil du réseau local, associé une fois en scannant le QR code de
  cette page (ou en tapant son code) ; il ouvre ensuite les liens que donnent les agents,
  et s'installe sur l'écran d'accueil. Chaque appareil se révoque depuis cette page (voir
  `[surfaces]` dans `config.toml`).
- **Fonctions publiées** : actions HTTP déterministes qu'un Agent publie et que les
  autres appellent sans tour de modèle. Voir [FUNCTIONS](docs/FUNCTIONS.md).
- **Escalades** : quand un agent a besoin de vous, il pose une question à options,
  à laquelle on répond depuis la console ou Telegram.
- **Plusieurs machines** : un hub et des nœuds reliés par SSH, avec synchronisation des
  répertoires de travail. Voir [PHASE-4](docs/PHASE-4.md).
- **Panic stop** : `Cmd/Ctrl+Shift+X` deux fois arrête tout.

## Développer

Prérequis : Rust stable, Node.js, npm et les
[dépendances système de Tauri 2](https://v2.tauri.app/start/prerequisites/) (outils de
ligne de commande Xcode sur macOS, WebKitGTK sur Linux).

```bash
cargo test --workspace
ARGOS_HOME=/tmp/argos-essai cargo run -p argos-daemon -- hub
```

Dans un autre terminal :

```bash
cd apps/gui
npm ci
npm run tauri dev
```

Les tests ne consomment pas de tokens : les moteurs sont remplacés par de faux CLI qui
rejouent des sorties enregistrées (`ARGOS_CLAUDE_BIN`, `ARGOS_CODEX_BIN`). Les commandes
complètes sont dans [CLAUDE.md](CLAUDE.md) ; la construction des archives et le processus
de release dans [BUILD-BINARIES](docs/BUILD-BINARIES.md).

| Dossier | Rôle |
| --- | --- |
| `crates/argos-core` | types partagés daemon/GUI |
| `crates/argos-daemon` | daemon : sessions, outils MCP, mémoire, déclencheurs, canaux, API |
| `crates/argos-voice` | moteur vocal local |
| `apps/gui` | application Tauri 2 (Rust + React/TypeScript) |

## Documentation

- [VISION](docs/VISION.md) — décisions d'architecture numérotées
- [GLOSSARY](docs/GLOSSARY.md) — vocabulaire canonique
- [PHASE-1](docs/PHASE-1.md) à [PHASE-5](docs/PHASE-5.md) — ce qui est fait et ce qui reste
- [TELEGRAM](docs/TELEGRAM.md), [FUNCTIONS](docs/FUNCTIONS.md), [BUILD-BINARIES](docs/BUILD-BINARIES.md)

## Participer

Les contributions sont bienvenues : consultez [CONTRIBUTING.md](CONTRIBUTING.md), le
[code de conduite](CODE_OF_CONDUCT.md) et la [politique de sécurité](SECURITY.md).
Argos est distribué sous [licence MIT](LICENSE). Ses dépendances et les modèles
distribués séparément peuvent avoir des licences différentes.
