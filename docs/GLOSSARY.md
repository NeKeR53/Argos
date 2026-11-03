# Glossaire Argos

Vocabulaire canonique du projet. Le code, les docs et les prompts des agents utilisent ces termes et pas d'autres.

## Hiérarchie

- **Assistant** — L0, unique. La session avec laquelle l'utilisateur parle à la voix. Vit sur un nœud, et peut changer de nœud (D48). Gère des Agents, jamais des Workers.
- **Agent** — L1. Agent durable et nommé, dédié à un projet ou un domaine. Possède une charte, un board, une mémoire, des surfaces et des triggers. Exécute ses tâches et délègue à des Workers seulement si cela économise temps et tokens.
- **Worker** — L2. Exécutant éphémère d'une seule tâche, sans mémoire persistante, sans interaction avec l'utilisateur.
- **Moteur** — le CLI qui fait tourner un agent : `claude` (Claude Code) ou `codex`.

## Travail

- **Mission** — demande de haut niveau adressée à un Agent (par l'Assistant). L'Agent la découpe en tâches.
- **Tâche** — unité de travail sur le board d'un Agent, exécutée par son Agent ou déléguée à un Worker. États : `pending`, `in-progress`, `blocked`, `done`, `cancelled` (et `interrupted` après un crash).
- **Board** — liste priorisée des tâches d'un Agent. Lui appartient.
- **Charte** — fiche d'identité d'un Agent : nom, périmètre, ressources possédées. Sert à décider réutiliser vs créer.
- **Registre** — liste des Agents connus de l'Assistant, avec leurs chartes et leur nœud.
- **Escalade** — un agent remonte au niveau supérieur un besoin qu'il ne peut pas traiter seul (jusqu'à l'utilisateur pour un Agent).
- **Dialogue latéral** — échange direct entre deux Agents (D40) : questions et informations, jamais des ordres. Adressé par nom, acheminé par le daemon quel que soit le nœud.
- **Pair** — un autre Agent, interlocuteur possible d'un dialogue latéral.
- **Fonction publiée** — action HTTP déterministe créée et maintenue par un Agent, persistée par le daemon. L'Assistant, les Agents et les surfaces de son propriétaire peuvent l'appeler sans réveiller sa session. La signature publique contient son nom, sa description et ses paramètres ; la requête et les secrets restent côté daemon.
- **Expéditeur** — origine d'un message reçu par un agent : l'utilisateur, un autre agent, ou Argos lui-même.

## Contrôle

- **Plan de contrôle** — opérations déterministes appliquées par le daemon sans LLM : stop, kill, park, resume, move, retire, inspect, réordonner un board.
- **Plan cognitif** — demandes livrées à un agent comme stimulus, qu'il interprète : missions, replanification.
- **Parking** — arrêt du process d'un agent inactif ; il est repris plus tard par son session-id.
- **Panic stop** — arrêt dur de tout l'arbre, depuis l'overlay.

## Modèles

- **Profil** — catégorie sémantique de travail (`code`, `léger`…) qui se résout en moteur + modèle + chaîne de repli.
- **Politique de modèles** — l'ensemble des profils, replis et plafonds, éditée par l'utilisateur.

## Mémoire

- **Mémoire** — dossier Markdown persistant d'un Assistant ou d'un Agent : index, journal, notes de domaine, registre des enfants.
- **Digest** — résumé compact de la mémoire chargé au démarrage d'une session.
- **Miroir** — copie en consultation seule de toutes les mémoires, synchronisée sur le Mac et ouvrable dans Obsidian.

## Surfaces

- **Surface** — page HTML/CSS/JS produite par un agent et hébergée par l'app.
- **Cockpit** — surface vivante et bidirectionnelle, mise à jour en direct.
- **Artifact** — surface snapshot, autonome.
- **SDK Argos** — script injecté dans chaque surface : composant chat standard + bridge `argos.send` / `argos.on`.

## Événements

- **Trigger** — source qui émet des événements : cron, interrupteur, watcher.
- **Événement** — stimulus routé vers un agent cible ; réveille l'agent s'il est parké.
- **Connecteur** — watcher déclaratif prêt à l'emploi (email, fichier, webhook, MQTT, calendrier).
- **Watcher custom** — script sandboxé qui émet des événements via `emit()`, pour ce qu'aucun connecteur ne couvre.

## Infrastructure

- **Nœud** — machine qui fait tourner un daemon (Mac, VPS, Raspberry Pi).
- **Daemon** — process Rust headless présent sur chaque nœud ; exécute, ne décide pas.
- **Hub** — *(abandonné par D48)* le nœud central qui hébergeait l'Assistant et l'état de contrôle. Il n'y a plus que des nœuds pairs.
- **Pair** — un autre nœud du maillage, relié par un lien qui sert dans les deux sens (D48).
- **Annuaire** — la liste des agents de tout le maillage, que chaque nœud tient à jour avec ce que lui annoncent ses pairs (D48).
- **Mesh** — l'ensemble des nœuds pairs, reliés entre eux par des tunnels SSH maillés (D48).
- **GUI** — l'app Tauri sur le Mac : orbe, console, fenêtres de surfaces.
- **Orbe** — la bulle vocale de l'Assistant, toujours au premier plan.
- **Console** — panneau invocable au raccourci : arbre des Agents, état global, escalades, panic stop.
- **Canal** — voie de discussion entre l'utilisateur et l'Assistant hors de la console et de l'orbe (D45), par exemple un bot Telegram. Instance configurée d'un type de canal, avec un seul interlocuteur. L'Assistant choisit sur quel canal il écrit.
- **Type de canal** — connecteur de messagerie codé dans le daemon (`telegram`…), qui transporte sans décider et déclare ses capacités (texte, vocal, boutons…).
- **Isolation** — mode d'exécution d'un agent : conteneur ou répertoire borné.

- **Workspace** — répertoire de projet sur le nœud de l’Agent, partagé avec ses Workers. Son `AGENTS.md` fournit les consignes du projet. Distinct de la mémoire Argos et du `CODEX_HOME` isolé.
- **Synchro de workspace** — lien continu et bidirectionnel entre le workspace d'un Agent sur son nœud et une copie locale sur le hub (D44). Ne transporte ni `.git/` ni les fichiers ignorés.
- **Copie locale** — le dossier du hub tenu à jour par une synchro de workspace, où l'on lance et débogue le projet. Distincte du miroir, qui ne concerne que les mémoires et reste en lecture seule.
- **État commun** — pour une synchro, l'index des fichiers sur lesquels les deux côtés étaient d'accord à la dernière passe. Un fichier qui s'en écarte des deux côtés est en conflit.
