//! Escalades vers l'utilisateur (D12, D35) : gardées en base jusqu'à leur réponse, pour
//! que la console les montre avec leurs choix et renvoie la réponse à l'Agent.

use super::{Registry, RegistryError, Result, now, parsed};
use argos_core::{AgentId, AgentState, Escalation, EscalationState, Level};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

pub(super) const SCHEMA: &str = "
CREATE TABLE escalations (
    id          TEXT PRIMARY KEY,
    agent       TEXT NOT NULL REFERENCES agents(id),
    question    TEXT NOT NULL,
    options     TEXT NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('open', 'answered', 'dismissed')),
    answer      TEXT,
    created_at  INTEGER NOT NULL,
    resolved_at INTEGER
);
CREATE INDEX escalations_by_state ON escalations(state);
";

const COLUMNS: &str = "id, agent, question, options, state, answer, created_at, resolved_at";

/// Choix proposés au plus, et leur longueur : ce sont des boutons.
pub const MAX_OPTIONS: usize = 4;
const MAX_OPTION_CHARS: usize = 40;
/// Escalades résolues gardées pour la console, les plus récentes.
const RESOLVED_KEPT: usize = 20;

impl Registry {
    /// Ouvre une escalade pour un Agent vivant.
    pub fn add_escalation(
        &self,
        agent: &AgentId,
        question: &str,
        options: &[String],
    ) -> Result<Escalation> {
        let record = self.require(agent)?;
        if record.level != Level::Agent {
            return Err(RegistryError::Escalation(
                "seuls les Agents escaladent vers l'utilisateur".into(),
            ));
        }
        if record.state == AgentState::Retired {
            return Err(RegistryError::Hierarchy(format!("{agent} est retiré")));
        }
        let question = question.trim();
        if question.is_empty() {
            return Err(RegistryError::Escalation(
                "une escalade a une question".into(),
            ));
        }
        let options: Vec<String> = options
            .iter()
            .map(|option| option.trim().to_string())
            .filter(|option| !option.is_empty())
            .collect();
        if options.len() > MAX_OPTIONS {
            return Err(RegistryError::Escalation(format!(
                "{MAX_OPTIONS} choix au plus"
            )));
        }
        if let Some(long) = options
            .iter()
            .find(|option| option.chars().count() > MAX_OPTION_CHARS)
        {
            return Err(RegistryError::Escalation(format!(
                "choix trop long ({MAX_OPTION_CHARS} caractères au plus) : « {long} »"
            )));
        }
        let escalation = Escalation {
            id: format!("e-{}", &Uuid::new_v4().simple().to_string()[..8]),
            agent: agent.clone(),
            question: question.into(),
            options,
            state: EscalationState::Open,
            answer: None,
            created_at: now(),
            resolved_at: None,
        };
        self.db.execute(
            &format!("INSERT INTO escalations ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, 'open', NULL, ?5, NULL)"),
            params![
                escalation.id,
                agent.as_str(),
                escalation.question,
                serde_json::to_string(&escalation.options).expect("liste sérialisable"),
                escalation.created_at as i64
            ],
        )?;
        Ok(escalation)
    }

    pub fn escalation(&self, id: &str) -> Result<Escalation> {
        self.db
            .query_row(
                &format!("SELECT {COLUMNS} FROM escalations WHERE id = ?1"),
                [id],
                escalation_row,
            )
            .optional()?
            .ok_or_else(|| RegistryError::EscalationNotFound(id.into()))
    }

    /// Les escalades ouvertes, plus anciennes d'abord, puis les dernières résolues.
    pub fn escalations(&self) -> Result<Vec<Escalation>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM escalations WHERE state = 'open' ORDER BY created_at, rowid"
        ))?;
        let mut all: Vec<Escalation> = statement
            .query_map([], escalation_row)?
            .collect::<rusqlite::Result<_>>()?;
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM escalations WHERE state != 'open'
             ORDER BY resolved_at DESC, rowid DESC LIMIT ?1"
        ))?;
        all.extend(
            statement
                .query_map([RESOLVED_KEPT as i64], escalation_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        Ok(all)
    }

    /// Résout une escalade ouverte : répondue (avec sa réponse) ou écartée.
    pub fn resolve_escalation(
        &self,
        id: &str,
        state: EscalationState,
        answer: Option<&str>,
    ) -> Result<Escalation> {
        let escalation = self.escalation(id)?;
        if escalation.state != EscalationState::Open {
            return Err(RegistryError::Escalation(format!(
                "l'escalade {id} est déjà {}",
                escalation.state
            )));
        }
        let answer = answer.map(str::trim).filter(|answer| !answer.is_empty());
        match (state, answer) {
            (EscalationState::Answered, None) => {
                return Err(RegistryError::Escalation("une réponse est vide".into()));
            }
            (EscalationState::Open, _) => {
                return Err(RegistryError::Escalation(
                    "une escalade résolue ne se rouvre pas".into(),
                ));
            }
            _ => {}
        }
        self.db.execute(
            "UPDATE escalations SET state = ?2, answer = ?3, resolved_at = ?4 WHERE id = ?1",
            params![id, state.as_str(), answer, now() as i64],
        )?;
        self.escalation(id)
    }
}

/// Un Agent retiré n'attend plus rien : ses escalades ouvertes sont écartées.
pub(super) fn dismiss_escalations_of(db: &Connection, agents: &[AgentId]) -> Result<()> {
    for agent in agents {
        db.execute(
            "UPDATE escalations SET state = 'dismissed', resolved_at = ?2
             WHERE agent = ?1 AND state = 'open'",
            params![agent.as_str(), now() as i64],
        )?;
    }
    Ok(())
}

fn escalation_row(row: &Row) -> rusqlite::Result<Escalation> {
    let options: String = row.get(3)?;
    Ok(Escalation {
        id: row.get(0)?,
        agent: AgentId::new(row.get::<_, String>(1)?),
        question: row.get(2)?,
        options: serde_json::from_str(&options).unwrap_or_default(),
        state: parsed(row, 4)?,
        answer: row.get(5)?,
        created_at: row.get::<_, i64>(6)? as u64,
        resolved_at: row.get::<_, Option<i64>>(7)?.map(|at| at as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::super::NewAgent;
    use super::*;
    use argos_core::{Charter, EngineKind};

    fn registry_with_agent() -> (Registry, AgentId, AgentId) {
        let registry = Registry::open_in_memory().unwrap();
        let assistant = registry
            .ensure_assistant(EngineKind::Claude, None, "/tmp/a".into())
            .unwrap();
        let agent = registry
            .create(NewAgent {
                parent: assistant.id.clone(),
                level: Level::Agent,
                engine: EngineKind::Claude,
                model: None,
                profile: None,
                node: None,
                workdir: "/tmp/b".into(),
                charter: Some(Charter {
                    name: "Compta".into(),
                    scope: "les factures".into(),
                }),
            })
            .unwrap();
        (registry, assistant.id, agent.id)
    }

    #[test]
    fn an_escalation_is_kept_until_answered_once() {
        let (registry, _, agent) = registry_with_agent();
        let options = vec!["Approuver".to_string(), " Refuser ".to_string()];
        let escalation = registry
            .add_escalation(&agent, "Payer la facture de 120 € ?", &options)
            .unwrap();
        assert_eq!(escalation.options, ["Approuver", "Refuser"]);
        assert_eq!(registry.escalations().unwrap(), vec![escalation.clone()]);

        let answered = registry
            .resolve_escalation(&escalation.id, EscalationState::Answered, Some("Approuver"))
            .unwrap();
        assert_eq!(answered.state, EscalationState::Answered);
        assert_eq!(answered.answer.as_deref(), Some("Approuver"));
        assert!(answered.resolved_at.is_some());
        // Une seule réponse par escalade.
        assert!(
            registry
                .resolve_escalation(&escalation.id, EscalationState::Dismissed, None)
                .is_err()
        );
    }

    #[test]
    fn only_living_agents_escalate_with_few_short_options() {
        let (registry, assistant, agent) = registry_with_agent();
        assert!(registry.add_escalation(&assistant, "?", &[]).is_err());
        assert!(registry.add_escalation(&agent, "  ", &[]).is_err());
        let many: Vec<String> = (0..5).map(|n| n.to_string()).collect();
        assert!(registry.add_escalation(&agent, "?", &many).is_err());
        assert!(
            registry
                .add_escalation(&agent, "?", &["x".repeat(41)])
                .is_err()
        );
        let open = registry
            .add_escalation(&agent, "Quel compte ?", &[])
            .unwrap();
        assert!(
            registry
                .resolve_escalation(&open.id, EscalationState::Answered, Some(" "))
                .is_err()
        );

        registry.retire(&agent).unwrap();
        assert_eq!(
            registry.escalation(&open.id).unwrap().state,
            EscalationState::Dismissed
        );
        assert!(registry.add_escalation(&agent, "encore ?", &[]).is_err());
    }
}
