# Fonctions publiées

Un Agent peut publier une action HTTP déterministe avec `function_publish`. La définition est persistée dans SQLite ; l'Assistant, les Agents et les surfaces du propriétaire peuvent l'appeler sans réveiller sa session. Le daemon exécute sur le nœud du propriétaire. Un nœud absent fait échouer l'appel, sans exécution sur le hub ni relance automatique.

## Publication par l'Agent

Exemple pour une API HTTP acceptant un nom de lampe et un booléen. L'URL est à adapter au service réel :

```json
{
  "name": "set_light",
  "description": "Change l'état d'une lumière du salon ou du bureau.",
  "parameters": {
    "type": "object",
    "properties": {
      "name": {"type": "string", "enum": ["salon", "bureau"]},
      "state": {"type": "boolean"}
    },
    "required": ["name", "state"],
    "additionalProperties": false
  },
  "request": {
    "method": "POST",
    "url": "http://service-maison.local/lights/set",
    "headers": {"Authorization": "Bearer {{env:HOME_ASSISTANT_TOKEN}}"},
    "body": {"name": "{{name}}", "state": "{{state}}"},
    "timeout_ms": 10000
  }
}
```

`parameters` est un sous-ensemble de JSON Schema : objet avec propriétés scalaires `string`, `boolean`, `integer`, `number`, `enum` facultatif et `required`. `additionalProperties: false` est obligatoire ; les contraintes non prises en charge sont refusées. Pour une fonction sans argument : `properties: {}`, `required: []`.

Une chaîne JSON entière `"{{state}}"` conserve le type de l'argument : le corps ci-dessus contient un booléen. Les substitutions intégrées à une chaîne produisent du texte. Les valeurs ne sont jamais réinterprétées comme du code ou d'autres modèles.

L'URL accepte des substitutions dans son chemin et sa query, encodées comme données ; protocole, hôte et port restent fixes. Par exemple, une action Home Assistant peut utiliser `/api/services/light/turn_{{state}}`, avec `state` limité par `enum` à `"on"` et `"off"`, et un corps `{"entity_id":"light.{{name}}"}`. La signature reflète les paramètres du service réel ; elle n'impose pas un booléen à toutes les APIs.

Les secrets `{{env:NOM}}` sont autorisés uniquement dans les valeurs d'en-têtes et sont résolus dans l'environnement du daemon qui exécute la fonction. Ils ne sont pas inclus dans le catalogue public. Ne pas inscrire de secret en clair dans une définition ou une surface. Les valeurs de secrets résolues sont caviardées si le service les renvoie dans une réponse JSON.

Republier le même nom remplace la définition pour les prochains appels. `function_delete({"function":"set_light"})` retire sa propre fonction. Les appels déjà commencés gardent leur définition. Seul un Agent vivant peut publier ou retirer ses fonctions ; les Workers n'ont pas ces outils.

## Appel entre Agents

```json
{"agent":"Domotique"}
```

Passé à `function_list`, cet argument renvoie les noms, descriptions et schémas de paramètres, sans les requêtes ni leurs en-têtes. L'Agent se désigne par nom ou id. Sans argument, un Agent liste ses propres fonctions ; l'Assistant découvre toutes les fonctions des Agents vivants.

Puis `function_call` :

```json
{"agent":"Domotique","function":"set_light","arguments":{"name":"salon","state":true}}
```

Le résultat est du JSON ; une réponse HTTP vide vaut `null`. Une erreur HTTP, un timeout, un JSON invalide ou un argument incorrect est renvoyé comme erreur de l'outil. Aucun tour LLM n'est lancé pour réaliser l'action.

## Appel depuis une surface

```html
<button id="salon">Allumer le salon</button>
<p id="status" role="status" aria-live="polite"></p>
<script>
const button = document.getElementById('salon');
const status = document.getElementById('status');
button.onclick = async () => {
  button.disabled = true;
  status.textContent = 'Action en cours…';
  try {
    await argos.call('set_light', {name: 'salon', state: true});
    status.textContent = 'Salon allumé';
  } catch (error) {
    status.textContent = error.message;
  } finally {
    button.disabled = false;
  }
};
</script>
```

Le bridge identifie la surface ; celle-ci ne fournit aucun Agent cible et n'appelle que les fonctions de son propriétaire. Le SDK renvoie une Promise corrélée à l'appel, sans réseau direct depuis l'iframe. Une déconnexion rejette les appels en attente sans les rejouer. `argos.send` et `argos.emit` restent disponibles pour les demandes qui nécessitent la réflexion de l'Agent.

## Bornes et arrêt

- Définition, arguments et requête développée : 64 Kio au plus ; réponse JSON : 64 Kio au plus.
- Délai HTTP : 100 à 30 000 ms, 10 000 ms par défaut ; pas de redirection ni de nouvelle tentative automatique.
- 32 appels simultanés au plus sur le hub, par nœud et par SDK de surface.
- Stop, retrait et panic stop annulent les appels en cours du propriétaire ou de l'appelant. Cela ne peut pas annuler une action déjà appliquée par le service.
- Les appels sont journalisés sans leurs arguments, requêtes ni résultats.
- Le protocole hub/nœud passe en version 4 : les binaires des deux côtés doivent être à jour.
