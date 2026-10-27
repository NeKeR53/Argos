//! Politique de modèles (D22) : des profils sémantiques (« code », « léger »…) choisis
//! par les agents, chacun résolu en une chaîne moteur + modèle — le premier choix puis
//! les replis quand un quota est épuisé — avec des plafonds globaux. Éditée par
//! l'utilisateur dans la section `[models]` de `config.toml`.

use argos_core::{EngineKind, Level};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Écrite telle quelle dans `config.toml` au premier démarrage, et politique par défaut
/// d'un fichier qui n'a pas de section `[models]`.
pub const DEFAULT_MODELS: &str = r#"
[models]
# Profil pris quand l'agent n'en précise pas : le moins cher.
default_profile = "léger"

# Chaque profil est une chaîne : le premier choix, puis les replis quand son quota est
# épuisé. Les agents choisissent un profil, jamais un modèle en dur.
[models.profiles.code]
description = "Écrire, modifier ou relire du code."
chain = [
  { engine = "claude", model = "claude-opus-5-5" },
  { engine = "codex", model = "gpt-6.1-sol" },
]

[models.profiles.standard]
description = "Jugement ou rédaction soignée : analyse croisée de plusieurs sources, texte destiné à quelqu'un, décision nuancée."
chain = [
  { engine = "claude", model = "claude-sonnet-5-5" },
  { engine = "codex", model = "gpt-6.1-sol" },
]

[models.profiles."léger"]
description = "Par défaut : lire, trier, résumer, extraire, vérifier, convertir, suivre une procédure claire."
chain = [
  { engine = "claude", model = "claude-haiku-5-5" },
  { engine = "codex", model = "gpt-6-luna" },
]

[models.caps]
# Modèles jamais donnés à un Worker (coût) : le premier modèle permis de la chaîne est pris.
forbidden_for_workers = []
# Sessions simultanées au plus, par modèle.
max_parallel = { "claude-opus-5-5" = 4 }
"#;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModelPolicy {
    pub default_profile: String,
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub caps: Caps,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Profile {
    pub description: String,
    pub chain: Vec<ModelChoice>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModelChoice {
    pub engine: EngineKind,
    pub model: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Caps {
    #[serde(default)]
    pub forbidden_for_workers: Vec<String>,
    #[serde(default)]
    pub max_parallel: BTreeMap<String, usize>,
}

impl Default for ModelPolicy {
    fn default() -> Self {
        #[derive(Deserialize)]
        struct Section {
            models: ModelPolicy,
        }
        toml::from_str::<Section>(DEFAULT_MODELS)
            .expect("politique par défaut valide")
            .models
    }
}

impl ModelPolicy {
    /// Une politique utilisable : le profil par défaut existe et aucune chaîne n'est vide.
    pub fn check(&self) -> Result<(), String> {
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(format!(
                "profil par défaut « {} » absent des profils",
                self.default_profile
            ));
        }
        if let Some((name, _)) = self.profiles.iter().find(|(_, p)| p.chain.is_empty()) {
            return Err(format!("le profil « {name} » n'a aucun modèle"));
        }
        Ok(())
    }

    /// Moteur et modèle pour un nouvel agent de ce niveau, d'après le profil demandé
    /// (ou celui par défaut). Rend aussi le nom du profil retenu.
    pub fn resolve(
        &self,
        profile: Option<&str>,
        level: Level,
    ) -> Result<(String, ModelChoice), String> {
        let name = profile.unwrap_or(&self.default_profile);
        let chosen = self
            .profiles
            .get(name)
            .ok_or_else(|| format!("profil inconnu : « {name} » (profils : {})", self.names()))?;
        let choice = chosen
            .chain
            .iter()
            .find(|choice| self.allowed(choice, level))
            .ok_or_else(|| {
                format!("aucun modèle du profil « {name} » n'est permis à un {level}")
            })?;
        Ok((name.to_string(), choice.clone()))
    }

    /// Le repli après `engine` + `model` dans la chaîne du profil : le maillon permis
    /// suivant, ou `None` en bout de chaîne.
    pub fn fallback(
        &self,
        profile: &str,
        level: Level,
        engine: EngineKind,
        model: Option<&str>,
    ) -> Option<ModelChoice> {
        let chain = &self.profiles.get(profile)?.chain;
        let current = chain
            .iter()
            .position(|choice| choice.engine == engine && Some(choice.model.as_str()) == model)?;
        chain[current + 1..]
            .iter()
            .find(|choice| self.allowed(choice, level))
            .cloned()
    }

    /// Plafond de sessions simultanées pour ce modèle, s'il y en a un.
    pub fn max_parallel(&self, model: &str) -> Option<usize> {
        self.caps.max_parallel.get(model).copied()
    }

    /// Les profils et leur usage, pour les décrire aux agents.
    pub fn catalogue(&self) -> String {
        self.profiles
            .iter()
            .map(|(name, profile)| format!("« {name} » : {}", profile.description))
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    pub fn names(&self) -> String {
        self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
    }

    fn allowed(&self, choice: &ModelChoice, level: Level) -> bool {
        level != Level::Worker || !self.caps.forbidden_for_workers.contains(&choice.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_policy_is_valid() {
        let policy = ModelPolicy::default();
        policy.check().unwrap();
        assert_eq!(policy.default_profile, "léger");
        let (name, choice) = policy.resolve(Some("code"), Level::Worker).unwrap();
        assert_eq!(name, "code");
        assert_eq!(
            choice,
            ModelChoice {
                engine: EngineKind::Claude,
                model: "claude-opus-5-5".into()
            }
        );
        assert_eq!(
            policy.resolve(Some("léger"), Level::Agent).unwrap().1.model,
            "claude-haiku-5-5"
        );
        assert_eq!(policy.max_parallel("claude-opus-5-5"), Some(4));
    }

    #[test]
    fn without_a_profile_the_default_one_is_taken() {
        let policy = ModelPolicy::default();
        assert_eq!(policy.resolve(None, Level::Agent).unwrap().0, "léger");
        assert!(policy.resolve(Some("inconnu"), Level::Agent).is_err());
    }

    #[test]
    fn falling_back_walks_the_chain_to_its_end() {
        let policy = ModelPolicy::default();
        let next = policy
            .fallback(
                "code",
                Level::Worker,
                EngineKind::Claude,
                Some("claude-opus-5-5"),
            )
            .unwrap();
        assert_eq!(
            (next.engine, next.model.as_str()),
            (EngineKind::Codex, "gpt-6.1-sol")
        );
        assert_eq!(
            policy.fallback(
                "code",
                Level::Worker,
                EngineKind::Codex,
                Some("gpt-6.1-sol")
            ),
            None
        );
        // Un modèle hors chaîne n'a pas de repli défini.
        assert_eq!(
            policy.fallback("code", Level::Worker, EngineKind::Claude, Some("autre")),
            None
        );
    }

    #[test]
    fn workers_never_get_a_forbidden_model() {
        let mut policy = ModelPolicy::default();
        policy.caps.forbidden_for_workers = vec!["claude-opus-5-5".into()];
        assert_eq!(
            policy.resolve(Some("code"), Level::Worker).unwrap().1.model,
            "gpt-6.1-sol"
        );
        assert_eq!(
            policy.resolve(Some("code"), Level::Agent).unwrap().1.model,
            "claude-opus-5-5"
        );

        policy.caps.forbidden_for_workers.push("gpt-6.1-sol".into());
        assert!(policy.resolve(Some("code"), Level::Worker).is_err());
    }

    #[test]
    fn a_broken_policy_is_reported() {
        let policy = ModelPolicy {
            default_profile: "absent".into(),
            ..ModelPolicy::default()
        };
        assert!(policy.check().is_err());
        let mut policy = ModelPolicy::default();
        policy.profiles.get_mut("code").unwrap().chain.clear();
        assert!(policy.check().is_err());
    }
}
