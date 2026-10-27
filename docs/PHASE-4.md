# Phase 4 — Multi-nœud

Objectif (D32, D33) : des Agents qui tournent sur d'autres machines (VPS, Raspberry Pi), pilotés depuis le hub comme s'ils étaient locaux.

**Contrainte du moment** : les vrais serveurs ne sont pas prêts. Tout est développé et vérifié avec des nœuds **simulés sur le Mac** (un vrai `argos-daemon node` par process, chacun avec son `ARGOS_HOME` et ses ports) et un faux `ssh`. Aucune connexion à un hôte réel n'a été faite.

## Architecture

- **Le hub garde tout l'état.** Registre, boards, surfaces, déclencheurs, mémoire et plan de contrôle restent sur le hub. Un nœud n'est qu'un exécuteur : il lance les sessions `claude` / `codex` qu'on lui demande et n'a pas de registre.
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

## Écarts avec la vision, à trancher

1. **Mémoire sur le hub, pas sur le nœud de l'agent** (D23 dit « sur le nœud de l'agent »).
   - Les outils de mémoire passent par MCP, donc par le hub, et le digest est construit au réveil, sur le hub.
   - Garder la mémoire au même endroit que le registre évite toute synchronisation, et le miroir Obsidian sur le Mac (D25) n'a qu'une source à copier.
   - Prix : un agent d'un nœud n'a plus accès à sa mémoire quand le hub est absent, ce qui le bloque de toute façon aujourd'hui (point suivant).
2. **Pas d'autonomie des nœuds sans le hub** (D32 la prévoit, avec les escalades en file).
   - Aujourd'hui un nœud sans hub parke ses sessions.
   - L'autonomie demande un vrai superviseur local sur le nœud (registre partiel, file d'attente des rapports et escalades, réconciliation au retour). C'est le plus gros morceau restant, et il vaut mieux le concevoir avec les vrais serveurs sous la main.
3. **Les déclencheurs tournent sur le hub** (D31 : « sur le nœud de ce qu'il observe »). Un watcher de fichier ne voit que les fichiers du hub ; il faudra le déporter sur le nœud.

## Reste à faire

- **Installation d'un nœud** (D33) : détection OS et architecture, envoi du binaire compilé pour la cible, enregistrement depuis la console. À faire avec un vrai hôte. Les fichiers de service sont déjà générés : `argos-daemon service systemd node` (unité systemd utilisateur) ou `argos-daemon service launchd node` (agent launchd, vérifié par `plutil`), qui lancent ce binaire avec ce `ARGOS_HOME` ; les poser reste un geste humain.
- **Conteneurs par nœud** (D36), politique réseau, plafonds.
- **Synchronisation de la mémoire vers le Mac** (miroir Obsidian en lecture seule, D25).
- **GUI** : afficher le nœud de chaque agent et l'état des nœuds (commande `nodes`).
- **Hub sur le VPS** : la GUI et la voix du Mac joignent alors l'API du hub par un tunnel SSH. Rien à coder côté daemon : l'API reste locale au VPS.
