//! Escalades vers l'utilisateur (D12, D35) : ouvertes par un Agent, résolues depuis la
//! console ou par l'Assistant. La réponse part directement à l'Agent qui attend.

use super::{Result, Supervisor};
use argos_core::protocol::{DaemonEvent, Origin};
use argos_core::{AgentId, Escalation, EscalationState};

impl Supervisor {
    /// Ouvre une escalade et la signale aux consoles.
    pub fn open_escalation(
        &self,
        agent: &AgentId,
        question: &str,
        options: &[String],
    ) -> Result<Escalation> {
        let escalation = self.registry().add_escalation(agent, question, options)?;
        self.memory_log(
            agent,
            format!(
                "escalade {} ouverte : {}",
                escalation.id, escalation.question
            ),
        );
        self.publish(DaemonEvent::EscalationUpdated {
            escalation: escalation.clone(),
        });
        Ok(escalation)
    }

    /// Répond à une escalade ouverte : la réponse est remise à l'Agent, au nom de
    /// l'utilisateur, avec la question pour mémoire.
    pub async fn answer_escalation(&self, id: &str, answer: &str) -> Result<Escalation> {
        let escalation =
            self.registry()
                .resolve_escalation(id, EscalationState::Answered, Some(answer))?;
        let answer = escalation.answer.as_deref().unwrap_or_default();
        self.settle_escalation(
            &escalation,
            format!("répondue : {answer}"),
            format!(
                "[Réponse de l'utilisateur à ton escalade {}]\nQuestion : {}\nRéponse : {answer}",
                escalation.id, escalation.question
            ),
        )
        .await?;
        Ok(escalation)
    }

    /// Écarte une escalade sans y répondre ; l'Agent cesse de l'attendre.
    pub async fn dismiss_escalation(&self, id: &str) -> Result<Escalation> {
        let escalation =
            self.registry()
                .resolve_escalation(id, EscalationState::Dismissed, None)?;
        self.settle_escalation(
            &escalation,
            "écartée par l'utilisateur".into(),
            format!(
                "[Escalade {} écartée par l'utilisateur]\nQuestion : {}\nN'attends pas de réponse : \
                 continue sans, ou rends compte à l'Assistant si tu ne peux pas avancer.",
                escalation.id, escalation.question
            ),
        )
        .await?;
        Ok(escalation)
    }

    async fn settle_escalation(
        &self,
        escalation: &Escalation,
        fact: String,
        message: String,
    ) -> Result<()> {
        self.publish(DaemonEvent::EscalationUpdated {
            escalation: escalation.clone(),
        });
        let fact = format!("escalade {} {fact}", escalation.id);
        self.memory_log(&escalation.agent, fact.clone());
        // L'Assistant l'a relayée : il sait qu'elle est close et ne la reposera pas.
        let assistant = self.registry().assistant();
        if let Ok(Some(assistant)) = assistant {
            self.memory_log(&assistant.id, fact);
        }
        self.deliver(&escalation.agent, Origin::User, message).await
    }
}
