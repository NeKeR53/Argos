//! Fonctions publiées : aucun réveil de session, exécution sur le nœud du propriétaire.

use super::{Supervisor, locked};
use argos_core::protocol::DaemonEvent;
use argos_core::{AgentId, AgentState, Level};
use serde_json::Value;
use std::sync::atomic::Ordering;
use tokio::sync::oneshot;

pub(super) struct FunctionCall {
    owner: AgentId,
    caller: AgentId,
    cancel: oneshot::Sender<()>,
}

struct Pending {
    supervisor: Supervisor,
    id: u64,
}

impl Drop for Pending {
    fn drop(&mut self) {
        locked(&self.supervisor.inner.function_calls).remove(&self.id);
    }
}

impl Supervisor {
    /// Appelle uniquement une fonction explicitement publiée par un Agent vivant.
    /// Une surface passe son propriétaire comme appelant : elle n'adresse aucun pair.
    pub async fn call_function(
        &self,
        caller: &AgentId,
        owner: &AgentId,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let caller_record = self.record(caller).map_err(|e| e.to_string())?;
        let owner_record = self.record(owner).map_err(|e| e.to_string())?;
        if caller_record.level == Level::Worker {
            return Err("un Worker ne peut pas appeler de fonction publiée".into());
        }
        if owner_record.level != Level::Agent {
            return Err("seul un Agent publie des fonctions".into());
        }
        if [caller_record.state, owner_record.state].contains(&AgentState::Retired) {
            return Err("un agent de cet appel est retiré".into());
        }
        let definition = self
            .registry()
            .function(owner, name)
            .map_err(|e| e.to_string())?;
        crate::functions::validate_args(&definition.parameters, &arguments)?;
        let (cancel, cancelled) = oneshot::channel();
        let id = self.inner.generations.fetch_add(1, Ordering::SeqCst);
        {
            // Même ordre que stop : l'appel est soit enregistré, soit refusé avant l'arrêt.
            let halted = locked(&self.inner.halted);
            if halted.contains(caller) || halted.contains(owner) {
                return Err("un agent de cet appel est en cours d'arrêt".into());
            }
            // Le retrait prend aussi halted : pas d'inscription à partir d'un état périmé.
            if self.record(caller).map_err(|e| e.to_string())?.state == AgentState::Retired
                || self.record(owner).map_err(|e| e.to_string())?.state == AgentState::Retired
            {
                return Err("un agent de cet appel est retiré".into());
            }
            let mut calls = locked(&self.inner.function_calls);
            if calls.len() >= 32 {
                return Err("trop d'appels de fonctions en cours (32 au plus)".into());
            }
            calls.insert(
                id,
                FunctionCall {
                    owner: owner.clone(),
                    caller: caller.clone(),
                    cancel,
                },
            );
        }
        let _pending = Pending {
            supervisor: self.clone(),
            id,
        };
        let _ = self.inner.events.send(DaemonEvent::Tool {
            agent: owner.clone(),
            name: format!("function_call:{name}"),
        });
        let execute = async {
            match &owner_record.node {
                Some(node) => {
                    self.nodes()
                        .function(node, owner, definition.request, arguments)
                        .await
                }
                None => crate::functions::execute(&definition.request, &arguments).await,
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancelled => Err("appel de fonction annulé par l'arrêt".into()),
            result = execute => result,
        };
        let outcome = if result.is_ok() {
            "réussi"
        } else {
            "échoué ou annulé"
        };
        let fact = format!("fonction {name} appelée par {caller} : {outcome}");
        self.memory_log(owner, fact.clone());
        if caller != owner {
            self.memory_log(caller, fact);
        }
        result
    }

    pub(super) fn cancel_functions(&self, targets: &[AgentId]) {
        let mut calls = locked(&self.inner.function_calls);
        let ids: Vec<_> = calls
            .iter()
            .filter(|(_, call)| targets.contains(&call.owner) || targets.contains(&call.caller))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(call) = calls.remove(&id) {
                let _ = call.cancel.send(());
            }
        }
    }
}
