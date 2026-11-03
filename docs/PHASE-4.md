# Phase 4 — Multi-nœud

Objectif (D32, D33) : des Agents qui tournent sur d'autres machines (VPS, Raspberry Pi), pilotés depuis le hub comme s'ils étaient locaux.

**Contrainte du moment** : les vrais serveurs ne sont pas prêts. Tout est développé et vérifié avec des nœuds **simulés sur le Mac** (un vrai `argos-daemon node` par process, chacun avec son `ARGOS_HOME` et ses ports) et un faux `ssh`. Premier essai sur un hôte réel le 2026-11-03 : le Raspberry Pi, joint par l'hôte SSH `hermes-app` (voir « Essai sur un vrai hôte »). Les hôtes `hermes` et `homeassistant` restent hors limites.

## Architecture

- **Le hub garde l'état de contrôle.** Registre, boards, surfaces, déclencheurs et plan de contrôle restent sur le hub. Un nœud exécute : il lance les sessions `claude` / `codex` qu'on lui demande et garde la mémoire de ses agents (voir plus bas), sans registre.
- **Une seule connexion, ouverte par le hub.** Le hub se connecte en WebSocket à chaque nœud, directement ou dans un tunnel SSH qu'il ouvre lui-même (`ssh -N -L`). Cette connexion porte :
  - les ordres (`spawn`, `send`, `interrupt`, `park`, `kill`) ;
  - les événements des sessions (le même `EngineEvent` qu'en local, sérialisé) ;
  - les requêtes MCP des agents du nœud.
  Le nœud n'ouvre jamais de connexion vers le hub et n'écoute que sur 127.0.0.1.
- **Mandataire MCP sur le nœud.** Les agents d'un nœud reçoivent l'URL `http://127.0.0.1:<port du nœud>/mcp/<jeton>`. Le nœud relaie chaque requête au hub par la connexion. Le hub y répond avec le même code que pour ses agents locaux (`mcp::answer`), donc le jeton, les outils par niveau et les vérifications sont identiques.
- **Une session distante est une `Session` comme une autre.** Le superviseur ne fait la différence qu'au réveil : `record.node` vide, la session est lancée sur place ; sinon `Nodes::spawn`. Interruption, parking, stop en cascade, repli de modèle, compaction et dialogue latéral fonctionnent sans changement.

## Ce qui est en place

- **Registre** (schéma v7) : colonne `node` sur les agents. Un Worker hérite toujours du nœud de son Agent. Sur un nœud, un répertoire de travail relatif part du dossier de données du nœud (`agents/<nom>-xxxx` par défaut).
- **Configuration du hub** : une section par nœud, ajoutée à la main (D33 : l'ajout d'un nœud reste un geste humain).
  ```toml
  [nodes.pi]
  ssh = "pi"                  # hôte de ~/.ssh/config ; ou url = "ws://127.0.0.1:7880"
  remote = "127.0.0.1:7880"   # écoute du nœud, vue depuis cet hôte
  token_file = "/chemin/vers/le/jeton"
  ```
- **Mode nœud** : `argos-daemon node` lit `$ARGOS_HOME/node.toml` (`name`, `listen`, `mcp_listen`) et crée son jeton dans `$ARGOS_HOME/node-token`, lisible par l'utilisateur seul.
- **Connexion** :
  - le nœud se présente (nom et version de protocole) ; un nœud qui donne un autre nom est refusé ;
  - nouvelles tentatives de 0,5 s à 30 s ;
  - un nœud n'accepte qu'un hub à la fois, et une nouvelle connexion remplace l'ancienne.
- **Tunnel SSH** : `ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -L 127.0.0.1:<port libre>:<remote> <hôte>`.
  - Jamais de question interactive : un tunnel qui ne s'ouvre pas échoue.
  - Le binaire se remplace par `ARGOS_SSH_BIN` (tests).
- **Perte d'un nœud** :
  - côté hub, chaque session du nœud reçoit `Exited` et son agent est parké ;
  - côté nœud, les sessions de la connexion perdue sont parkées, puis tuées 10 s plus tard ;
  - un message à un agent d'un nœud injoignable est refusé avec « nœud … injoignable » ;
  - au retour du nœud, l'agent reprend sa session par son session-id.
- **Outils de l'Assistant** :
  - `list_nodes` : nœuds déclarés et s'ils sont joignables ;
  - `create_agent` accepte `node`, refusé s'il n'est pas déclaré.
  - Le contrat de l'Assistant lui demande de placer un Agent sur le nœud de ce qu'il gère.
  - `list_agents` et l'état structuré de la mémoire indiquent le nœud.
- **API** : commande `nodes` → `{ kind: "nodes", nodes: [{ name, connected, error }] }` ; `node` apparaît dans chaque agent.

## Tests (sans tokens, sans réseau)

Dans `tests/nodes.rs`, chaque nœud est un vrai process `argos-daemon node` sur faux CLI :
- un Agent placé sur un nœud y tourne (dossier du nœud, mandataire MCP du nœud, sans droit d'écrire ni de lancer de commande, comme tout Agent) et appelle les outils du hub par le nœud (`tools/list`, `add_task`) ; un jeton inconnu est refusé ;
- un Worker lancé par cet Agent tourne sur le même nœud, avec un `CODEX_HOME` du nœud ;
- un nœud tué : l'agent est parké, un message est refusé ; le nœud relancé, l'agent reprend sa session (`--resume`) ;
- l'Assistant ne place un Agent que sur un nœud déclaré ;
- un nœud qui se présente sous un autre nom est refusé ;
- un nœud `ssh` est atteint par le tunnel du faux `ssh` (`tests/fixtures/fake-ssh`, un relais TCP local qui ne contacte aucun hôte).

S'y ajoutent des tests unitaires : configuration des nœuds et du mode nœud, arguments du tunnel.

## Mémoire sur le nœud de l'agent (D23, D25)

- La mémoire d'un Agent placé sur un nœud vit **sur ce nœud** (`$ARGOS_HOME/memory/<id>` du nœud), avec son git d'annulation.
- Le hub y envoie chaque opération par la connexion : `ensure`, `write_state`, `log`, `digest`, `list`, `read`, `write`, `search` (`MemoryOp`, protocole v2). Les outils `memory_*`, l'état structuré et le digest du réveil passent tous par `Supervisor::memory_op`.
- Le hub garde un **miroir** sans historique dans son propre `memory/<id>`. Le nœud lui envoie une copie complète de la mémoire après chaque écriture, et de toutes ses mémoires à chaque connexion : ce qui a changé pendant une coupure revient ainsi au hub. Ce miroir est la source du futur miroir Obsidian du Mac.
- Nœud injoignable : les lectures se font sur le miroir, les écritures sont refusées (« mémoire non modifiée : nœud … injoignable »). Les faits que le daemon ajoute au journal sont alors perdus, sans bloquer l'action qu'ils décrivent.
- Test : `an_agents_memory_lives_on_its_node_and_is_mirrored_on_the_hub`.

## Déplacement d'un Agent (D47)

L'Assistant déplace l'un de ses Agents par `move_agent(agent, node, workdir?)` : vers un nœud déclaré, ou vers le hub (`node = "hub"`). Code : `supervisor/relocation.rs`.

**Déroulé.** Rien ne change dans le registre tant que mémoire et workspace ne sont pas arrivés.
1. Vérifications : Agent vivant et inactif, sans Worker ni synchro, nœuds de départ et d'arrivée déclarés et joignables. Un workspace qui est un dossier de projet exige `workdir` (le chemin du projet à l'arrivée).
2. Sa session est arrêtée ; la conversation du moteur reste sur l'ancienne machine.
3. Mémoire exportée (`MemoryOp::Export`) : dernier commit, puis `git bundle --all` en base64, plus les fichiers pour une arrivée sans git.
4. Dossier dédié copié, par les requêtes de la synchro ouvertes en mode copie (`SyncRequest::Open { copy: true }`) : arbre complet, `.git` et fichiers ignorés compris, dossier créé s'il manque, sans watcher. Même nom de dossier à l'arrivée, ou un nom neuf s'il existe déjà (retour sur une machine déjà occupée). D'un nœud à un autre, la copie passe par `$ARGOS_HOME/moves/<id>` du hub, effacé ensuite. Fichiers de plus de 100 Mo non copiés, signalés.
5. Mémoire importée (`MemoryOp::Import`) : clone du bundle ; une mémoire déjà là (le miroir du hub) est mise de côté.
6. Registre : nouveau nœud, nouveau workspace, session oubliée.
7. Ancienne mémoire mise de côté (`MemoryOp::Archive` → `memory-moved/<id>-<date>`), jamais effacée. L'ancien workspace reste en place.

**Miroir.** Le hub n'accepte plus la copie de mémoire d'un nœud que si l'agent y est placé : une mémoire restée sur un ancien nœud n'écrase jamais celle de l'agent. Un nœud n'envoie pas de copie après une mise de côté.

**Protocole** nœud v7 : `MemoryOp::Export`, `Import`, `Archive` ; champ `copy` de `SyncRequest::Open`. Un nœud v6 est refusé : le mettre à jour avec le hub.

**Test** : `an_agent_moves_between_machines_with_its_memory_and_workspace` (hub → pi → vps → hub, mémoire et historique, `.git` et fichier ignoré copiés, session neuve sur le nœud, refus pour un nœud inconnu et pour un dossier de projet sans `workdir`) ; tests unitaires de `memory`.

## Synchro de workspace (D44)

Cas d'usage : un Agent développe un projet sur le Pi ou le VPS ; on lance ce projet en debug sur le Mac, et les deux copies restent à jour sans commit ni push. Code : `src/sync/` (`tree`, `plan`, `hub`), `registry/syncs.rs`, côté nœud dans `node/runner.rs`.

**Modèle.**
- Une synchro relie un Agent placé sur un nœud et un dossier absolu du hub, vide ou absent à l'ouverture. Elle est déclarée dans le registre (table `syncs`, schéma v11) et relancée au démarrage du hub.
- Chaque côté tient un index `chemin → (empreinte blake3, taille, mtime, exécutable, lien)`. L'**état commun** est le dernier index sur lequel les deux côtés se sont accordés ; le hub l'écrit après chaque passe dans `$ARGOS_HOME/sync/<id>/base.json`.
- Une modification se détecte par écart d'empreinte avec l'état commun, pas par l'horodatage : un `touch` ne voyage pas, et l'écho d'une écriture de la synchro se réduit à un accord.
- Ne voyagent pas : `.git/`, les `.gitignore` de l'arbre (crate `ignore`), les exclusions de la synchro, les temporaires `.argos-sync-*`. Un fichier connu qui devient ignoré n'est pas une suppression : il est « tenu », ni copié ni supprimé. Pareil pour un fichier de plus de 100 Mo, signalé dans l'état.

**Passes.**
- À l'ouverture et à chaque reconnexion, le nœud parcourt le workspace et rend son index complet ; ensuite son watcher (`notify`, regroupé jusqu'à 300 ms de calme, 2 s au plus) envoie les écarts. Le hub regarde sa copie locale de la même façon.
- Rapprochement à trois états (`plan::reconcile`) : un seul côté a changé → copie ou suppression de l'autre côté ; deux changements identiques → accord ; une modification contre une suppression → la modification gagne ; deux modifications → conflit.
- Conflit : la version du hub est renommée `<nom>.conflit-hub` (ou `-2`…), celle du nœud prend le chemin, puis la version mise de côté part au nœud à la passe suivante. Une escalade est ouverte au nom de l'Agent, avec « Fusionne les deux versions » / « Garde la version du nœud ».
- Toute écriture porte l'empreinte attendue du chemin ; si le fichier a changé entre-temps, elle est refusée et la passe suivante reprend. Écriture dans un temporaire, vérification de l'empreinte, puis `rename`.
- Garde-fou : une passe qui supprimerait d'un coup plus de la moitié des fichiers suivis (20 au moins) est suspendue et signalée en erreur.
- Nœud injoignable : état `offline`, la copie locale reste modifiable ; rapprochement au retour.

**Protocole** (nœud v6, v7 depuis D47) : `ToNode::Sync { call, sync, request }` avec `open` (workdir, exclusions, chemins connus), `read` (morceaux de 1 Mo, 4 Mo par aller-retour), `write` (`put` par morceaux, `link`, `delete`) ou `close` ; réponses `FromNode::SyncReply`, écarts spontanés `FromNode::SyncChanged`. Le hub attend chaque réponse avant la demande suivante. Le nœud n'écrit que sous le workspace de l'Agent : chemin absolu, `..` ou passage par un lien symbolique refusés.

**Surfaces.**
- Assistant : `open_sync(agent, local, excludes?)`, `close_sync(sync)`, `list_syncs` ; son contrat ne les utilise qu'à la demande de l'utilisateur.
- API : `syncs`, `open_sync`, `close_sync` ; événement `sync_updated` (états `idle`, `syncing`, `offline`, `error`, fichiers suivis, conflits, fichiers tenus).
- GUI : section « Synchro » dans le panneau d'un Agent placé sur un nœud (ouvrir, état, conflits, fermer).
- Tant qu'une synchro est ouverte, le workspace de l'Agent ne change pas. La fermer laisse les deux copies en place.

**Tests** : unitaires (`sync::tree`, `sync::plan`, `sync::hub`, `registry::syncs`) et dans `tests/nodes.rs`, sur nœuds simulés : synchro dans les deux sens avec `.git`, `.gitignore` et exclusions respectés, sans écho, écriture hors workspace refusée, workspace figé ; coupure du nœud avec conflit, modification contre suppression et ajout hors ligne ; refus d'ouverture. Essayé aussi dans la console, avec un hub et un nœud jetables.

**Limites connues.** Fichier entier à chaque changement (pas de diff par blocs). Sur Linux, le watcher récursif pose un watch inotify par dossier, ignorés compris : un très gros `node_modules/` peut atteindre la limite du système. La synchro entre deux nœuds dont aucun n'est le hub reste à concevoir avec les vrais serveurs.

## Essai sur un vrai hôte (2026-11-03)

Hôte : Raspberry Pi (`aarch64`, Debian 13, glibc 2.41, 4 Go), joint par l'hôte SSH `hermes-app` (tunnel launchd local vers le Pi). Hub d'essai sur le Mac, `ARGOS_HOME` jetable et ports 17878/17879/17890, sur faux CLI ; le hub de tous les jours n'a pas été touché.

**Installation à la main, faute d'outil (D33 reste à faire).**
1. Binaire : aucune release Linux `aarch64`, et ni `rustup` ni cible croisée sur le Mac (Rust de Homebrew). Compilé sur le Pi : `rustup` en profil minimal dans le compte, sources par `git archive HEAD`, `cargo build --release -p argos-daemon -j3` : 16 min la première fois, 7 min après une modification du daemon.
2. Binaire posé dans `~/.local/share/argos/argos-daemon` (même place que `install.sh` sous Linux) ; `~/.argos/node.toml` avec `name = "hermes"`.
3. Service : `argos-daemon service systemd node > ~/.config/systemd/user/argos-node.service`, puis `systemctl --user enable --now argos-node`. Le compte a le *linger* : le service tourne sans session ouverte.
4. Hub : jeton du nœud copié dans un fichier local, puis
   ```toml
   [nodes.hermes]
   ssh = "hermes-app"
   remote = "127.0.0.1:7880"
   token_file = "/chemin/du/jeton"
   ```

**Vérifié.**
- Le hub ouvre le tunnel, le nœud répond (`nodes` → `connected: true`). Après `systemctl --user restart argos-node`, le hub se reconnecte seul.
- Avec un second nœud jetable sur le Pi (faux CLI, ports 17880/17881) : un Agent créé par `create_agent` y est lancé dans `agents/<nom>` du nœud, avec l'URL MCP du mandataire du nœud. Depuis le Pi, `tools/list` et `add_task` passent par le nœud, puis le tunnel, jusqu'au registre du hub ; un jeton inconnu reçoit 404.
- Sur le nœud de service, un Agent sans `claude` trouvable est refusé avec « nœud hermes : impossible de lancer claude ».

**Corrigé pendant l'essai.**
- *PATH des services.* systemd utilisateur ne donne que `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin`. Le `claude` du Pi, installé dans `~/.local/bin`, restait introuvable. Les fichiers générés par `argos-daemon service` portent maintenant le `PATH` de la commande qui les écrit, complété par `~/.local/bin` (absent d'un `ssh hôte commande`).
- *SIGTERM.* Le hub et le nœud n'attendaient que Ctrl-C. Un `kill`, `systemctl stop` ou `launchctl kickstart -k` les tuait sans parker les sessions, et les tunnels `ssh` du hub restaient orphelins. SIGTERM suit maintenant le même chemin que Ctrl-C ; vérifié : sessions parkées, plus aucun tunnel après l'arrêt.

**Trouvé, pas encore corrigé.**
- *Reprise d'une session jamais enregistrée.* `claude` lancé avec `--session-id`, puis tué avant d'avoir écrit sa conversation : l'id reste dans le registre. Chaque réveil suivant relance `--resume <id>` et échoue (« No conversation found with session ID »). Rien ne corrige ce cas, sur le hub comme sur un nœud. Il faudrait oublier la session sur cette erreur (`clear_session`) et relancer à neuf, comme une compaction.
- *Pas de chemin d'installation d'un nœud* : `install.sh` n'installe qu'un hub, ne connaît pas Linux `aarch64`, et la release n'a pas d'archive `linux-arm64`. Il faudrait une cible `aarch64-unknown-linux-gnu` dans `release.yml` (runner `ubuntu-24.04-arm`), puis un `install.sh --node <nom>` : `node.toml`, unité `argos-node`, pas d'app ni d'AppImage, et le jeton affiché pour le hub.
- *Pas de mise à jour du nœud* : `argos-daemon update` lit `install.toml`, que l'installation à la main n'écrit pas. Sans ce fichier, il suppose le service `argos-hub`.

## Écarts avec la vision, à trancher

1. **Pas d'autonomie des nœuds sans le hub** (D32 la prévoit, avec les escalades en file).
   - Aujourd'hui un nœud sans hub parke ses sessions.
   - L'autonomie demande un vrai superviseur local sur le nœud (registre partiel, file d'attente des rapports et escalades, réconciliation au retour). C'est le plus gros morceau restant, et il vaut mieux le concevoir avec les vrais serveurs sous la main.
2. **Les déclencheurs tournent sur le hub** (D31 : « sur le nœud de ce qu'il observe »). Un watcher de fichier ne voit que les fichiers du hub ; il faudra le déporter sur le nœud.

## Reste à faire

- **Installation d'un nœud** (D33) : détection OS et architecture, envoi du binaire compilé pour la cible, enregistrement depuis la console. Faite une fois à la main sur le Pi (voir « Essai sur un vrai hôte »), sans outil pour l'instant. Les fichiers de service sont déjà générés : `argos-daemon service systemd node` (unité systemd utilisateur) ou `argos-daemon service launchd node` (agent launchd, vérifié par `plutil`), qui lancent ce binaire avec ce `ARGOS_HOME` ; les poser reste un geste humain.
- **Conteneurs par nœud** (D36), politique réseau, plafonds.
- **Synchronisation de la mémoire vers le Mac** (miroir Obsidian en lecture seule, D25).
- **GUI** : fait pour la lecture. Réglages → Nœuds montre une carte : ce Mac (hub), puis chaque nœud relié par son tunnel (vert joignable, rouge pointillé sinon), avec l'Assistant et les Agents qui y tournent et le nombre de Workers sous chacun ; un clic ouvre l'agent. Reste : rafraîchir sans attendre les 15 s de relecture, et placer un agent sur un nœud depuis la console.
- **Hub sur le VPS** : la GUI et la voix du Mac joignent alors l'API du hub par un tunnel SSH. Rien à coder côté daemon : l'API reste locale au VPS.
