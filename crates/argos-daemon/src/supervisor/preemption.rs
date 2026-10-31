//! Préemption (D14, option `[board] preempt`) : quand une mission passe en tête du board
//! de son Agent, les Workers occupés à une tâche moins prioritaire sont parkés ; ils
//! reprennent d'eux-mêmes quand cette mission est close ou annulée. Tout est déterministe.

use super::{Result, Supervisor, locked};
use crate::registry::Position;
use argos_core::protocol::Origin;
use argos_core::{AgentId, AgentState, TaskState};

impl Supervisor {
    /// Déplace une mission sur le board de son Agent, puis applique la préemption si
    /// elle est active et que la mission est désormais en tête. Rend les Workers parkés.
    pub async fn prioritize_mission(
        &self,
        mission: &str,
        position: Position,
    ) -> Result<Vec<AgentId>> {
        let agent = self.registry().mission(mission)?.agent;
        self.registry().reposition_mission(mission, position)?;
        // Les consoles relisent le board de l'Agent à chaque mise à jour.
        self.publish_status(&agent);
        if !self.inner.settings.preempt {
            return Ok(vec![]);
        }
        let (on_top, tasks) = {
            let registry = self.registry();
            let board = registry.board(&agent)?;
            let on_top = board.missions.first().is_some_and(|m| m.id == mission);
            (on_top, registry.tasks_outside_mission(&agent, mission)?)
        };
        if !on_top {
            return Ok(vec![]);
        }
        let mut parked = vec![];
        let mut listed = vec![];
        for task in tasks {
            let Some(worker) = task.worker else { continue };
            // Un Worker déjà au repos l'a été par son Agent : on ne le réveillera pas.
            if !locked(&self.inner.running).contains_key(&worker) {
                continue;
            }
            self.park(&worker).await?;
            locked(&self.inner.preempted).insert(worker.clone(), mission.to_string());
            listed.push(format!("{worker} (tâche {})", task.id));
            parked.push(worker);
        }
        if parked.is_empty() {
            return Ok(vec![]);
        }
        let list = listed.join(", ");
        self.memory_log(
            &agent,
            format!("préemption : mission {mission} en tête, au repos : {list}"),
        );
        self.deliver(
            &agent,
            Origin::Argos,
            format!(
                "[Préemption] La mission {mission} passe en tête de ton board. Mis au repos : \
                 {list}. Ils reprendront d'eux-mêmes quand {mission} sera close ou annulée."
            ),
        )
        .await?;
        Ok(parked)
    }

    /// Une mission vient d'être close ou annulée : les Workers qu'elle avait mis au repos
    /// reprennent leur tâche, si elle est toujours en cours chez eux.
    pub async fn resume_preempted(&self, mission: &str) -> Result<Vec<AgentId>> {
        let workers: Vec<AgentId> = {
            let mut preempted = locked(&self.inner.preempted);
            let workers = preempted
                .iter()
                .filter(|(_, m)| m.as_str() == mission)
                .map(|(worker, _)| worker.clone())
                .collect();
            preempted.retain(|_, m| m.as_str() != mission);
            workers
        };
        let mut resumed = vec![];
        for worker in workers {
            let task = {
                let registry = self.registry();
                let alive = registry
                    .get(&worker)?
                    .is_some_and(|record| record.state != AgentState::Retired);
                let task = registry.task_of_worker(&worker)?;
                task.filter(|task| alive && task.state == TaskState::InProgress)
            };
            let Some(task) = task else { continue };
            self.deliver(
                &worker,
                Origin::Argos,
                format!(
                    "[Reprise] La mission prioritaire {mission} est terminée : reprends ta \
                     tâche {} là où tu l'avais laissée.",
                    task.id
                ),
            )
            .await?;
            self.memory_log(
                &task.agent,
                format!("préemption levée : {worker} reprend la tâche {}", task.id),
            );
            resumed.push(worker);
        }
        Ok(resumed)
    }
}
