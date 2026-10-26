# Phase 5 — Événements

Objectif (D29–D31) : des agents stimulés par des événements, même au repos. Réalisée avant la phase 4 : elle se vérifie entièrement en local, alors que le mesh demande des machines distantes.

## Ce qui est en place

- **Déclencheurs en base** (schéma v5, `registry::triggers`, type partagé `Trigger`). Chacun a un propriétaire, une cible, un nom, une source, un paramètre, une consigne, un état actif ou non, et la date de son dernier déclenchement.
  - Sources : `every` (intervalle, 60 s au moins), `cron` (expression à 5 champs, heure locale du nœud, crate `croner`), `watch` (dossier absolu existant), `webhook` (avec un secret propre), `switch` (interrupteur).
  - Seuls l'Assistant et les Agents en créent et en reçoivent. Retirer un agent supprime ses déclencheurs.
- **Moteur d'événements** (`argos_daemon::events`), lancé par le daemon. Une passe par seconde :
  - déclenche les périodiques échus ; la référence est le dernier déclenchement, donc après une absence un déclencheur ne part qu'une fois, sans rafale de rattrapage ;
  - ouvre et ferme les surveillances de dossiers selon la base (crate `notify`, FSEvents sur macOS) : un fichier créé ou déposé déclenche, et un même fichier n'est signalé qu'une fois sur 3 s.
- **Webhook** : `POST /hooks/<déclencheur>/<secret>` sur le serveur HTTP du daemon (celui du MCP). Le corps de l'appel, 20 000 caractères au plus, accompagne la consigne. Un déclencheur inconnu, suspendu ou appelé avec un mauvais secret reçoit 404. Cela couvre Home Assistant, dont les automatisations savent appeler un webhook.
- **Livraison** : la consigne est remise à la cible au nom d'Argos, préfixée `[Déclencheur « nom » (tr-…, source)]`, avec le contenu de l'événement (chemin du fichier, corps du webhook…). Un agent parké est réveillé (D31). La remise se fait dans sa propre tâche, pour qu'un réveil lent ne retarde pas les autres déclencheurs.
- **Outils** (Assistant et Agents) : `trigger_create` (`source`, `every_minutes`, `cron`, `path`, `target`), `trigger_list`, `trigger_set` (on/off), `trigger_delete`.
  - Un Agent ne crée de déclencheur que pour lui-même ; l'Assistant peut cibler l'un de ses Agents.
  - Seul le propriétaire suspend ou supprime.
  - Un webhook rend son URL complète.
- **API** pour la console : `triggers { agent }`, `fire_trigger`, `set_trigger`, `delete_trigger`. L'utilisateur peut tout actionner, suspendre ou supprimer.
- **Contrats** : quand créer un déclencheur, comment traiter sa consigne (brièvement, sans déranger l'utilisateur sans raison), ne pas créer plus fréquent que nécessaire.
- **Tests** :
  - 2 tests du stockage, 3 tests des échéances ;
  - 6 de bout en bout : intervalle (une seule fois), cron, déclencheur suspendu, vrai dossier surveillé, vrai webhook avec secret, création par MCP et propriété, interrupteur ;
  - recette réelle `the_assistant_sets_up_a_switch_and_reacts_when_it_is_flipped` : l'Assistant crée l'interrupteur « pause café » et, actionné, répond en une phrase. Passée le 2026-10-26.

## Pas encore fait

- **Connecteur email (IMAP / Gmail)** : demande de stocker un accès à ta boîte mail. C'est une décision de sécurité qui te revient : où vit le secret (trousseau macOS, fichier 0600), quel compte, lecture seule.
- **Script personnalisé** (D30, la soupape) : exécuter un script écrit par un agent demande un vrai bac à sable (`sandbox-exec` sur macOS, bubblewrap sous Linux) ; à décider avec l'isolation de la phase 4.
- **MQTT** : en attendant, le webhook couvre Home Assistant.
- **Console** : liste des déclencheurs d'un agent, interrupteurs, activation. L'API est prête, l'affichage reste à faire.
