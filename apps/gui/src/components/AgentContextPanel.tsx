import { useEffect, useState } from "react";
import type { AgentId, AgentStatus, Capabilities, Command, ProfileInfo, Reply, Trigger } from "../protocol";
import { displayName } from "../state";
import { engineLabel } from "../format";
import { ModelPicker } from "./ModelPicker";
import { TriggerPanel, triggerKindLabel } from "./TriggerPanel";

interface Props {
  profiles: ProfileInfo[] | null; agent: AgentStatus; triggers: Trigger[] | undefined; revision: number; hooksBase: string | null;
  connected: boolean; send: (command: Command) => boolean; request: (command: Command) => Promise<Reply>;
  nameOf: (id: AgentId) => string;
}
function skillName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  const last = parts[parts.length - 1];
  return (last?.toLowerCase() === "skill.md" ? parts[parts.length - 2] : last) || path;
}

export function AgentContextPanel(props: Props) {
  const { agent, triggers, revision, connected, send, request } = props;
  const [selectedTrigger, setSelectedTrigger] = useState<string | null>(null);
  const [modelOpen, setModelOpen] = useState(false);
  const [capabilities, setCapabilities] = useState<{ config: Capabilities; argosEnabled: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const selected = triggers?.find((trigger) => trigger.id === selectedTrigger);
  useEffect(() => {
    if (!connected || agent.level === "worker") return;
    const timer = setTimeout(() => send({ command: "triggers", agent: agent.id }), 250);
    return () => clearTimeout(timer);
  }, [agent.id, agent.level, revision, connected, send]);
  useEffect(() => {
    if (!connected) return;
    let cancelled = false;
    setCapabilities(null); setError(null);
    request({ command: "capabilities", agent: agent.id }).then((reply) => {
      if (cancelled) return;
      if (reply.kind === "capabilities") setCapabilities({ config: reply.config, argosEnabled: reply.argos_enabled });
      else setError(reply.kind === "error" ? reply.message : "Réponse indisponible.");
    }).catch((reason: unknown) => { if (!cancelled) setError(String(reason)); });
    return () => { cancelled = true; };
  }, [agent.id, connected, request]);
  useEffect(() => { if (selectedTrigger && triggers && !selected) setSelectedTrigger(null); }, [selectedTrigger, selected, triggers]);
  return <aside className="context-area" aria-label={`Informations de ${displayName(agent)}`}>
    <div className="context-overview">
      <header className="context-title">{displayName(agent)}</header>
      <section className="context-section"><h2>Mission</h2><p>{agent.charter?.scope || (agent.level === "assistant" ? "Coordonner les agents et vous accompagner dans vos demandes." : agent.level === "worker" ? "Ce Worker intervient pour son agent." : "Aucun rôle renseigné.")}</p></section>
      {agent.level !== "worker" && <section className="context-section"><h2>Déclencheurs <span>{triggers?.length ?? ""}</span></h2>
        {!triggers ? <p>{connected ? "Chargement…" : "Hors connexion"}</p> : triggers.length === 0 ? <p>Aucun déclencheur.</p> : <ul className="context-triggers">{triggers.map((trigger) => <li key={trigger.id}><button type="button" aria-pressed={selectedTrigger === trigger.id} onClick={() => { setModelOpen(false); setSelectedTrigger(trigger.id); }}><span><strong>{trigger.name}</strong><small>{triggerKindLabel[trigger.kind]} · {trigger.enabled ? "actif" : "suspendu"}</small></span><span aria-hidden="true">›</span></button></li>)}</ul>}
      </section>}
      <section className="context-section context-model"><h2>Modèle</h2>{agent.level !== "worker" ? <button type="button" className="context-model-open" aria-expanded={modelOpen} onClick={() => { setSelectedTrigger(null); setModelOpen(true); }}><span>{engineLabel(agent)}</span><span aria-hidden="true">›</span></button> : <p>{engineLabel(agent)}</p>}</section>
      <section className="context-section"><h2>Skills</h2>
        {!connected && !capabilities ? <p>Hors connexion</p> : error ? <p role="status">Impossible de charger les capacités : {error}</p> : !capabilities ? <p>Chargement…</p> : capabilities.config.skills.length ? <ul className="context-tags">{capabilities.config.skills.map((skill) => <li key={skill} title={skill}>{skillName(skill)}</li>)}</ul> : <p>Aucun skill configuré.</p>}
      </section>
      <section className="context-section"><h2>MCP</h2>
        {!connected && !capabilities ? <p>Hors connexion</p> : error ? <p>Impossible de charger les capacités.</p> : !capabilities ? <p>Chargement…</p> : <>{capabilities.argosEnabled || capabilities.config.mcp.length ? <ul className="context-tags">{capabilities.argosEnabled && <li>Argos</li>}{capabilities.config.mcp.map((server) => <li key={server.name}>{server.name}</li>)}</ul> : <p>Aucun serveur MCP configuré.</p>}<p className="context-hint">Serveurs configurés au démarrage.</p></>}
      </section>
    </div>
    {modelOpen && <div className="context-detail context-model-detail" aria-label="Choix du modèle"><header><div><span>MODÈLE</span><h2>Changer de modèle</h2></div><button type="button" aria-label="Fermer le choix du modèle" onClick={() => setModelOpen(false)}>×</button></header>
      {!connected && <p className="context-model-offline">Hors connexion</p>}
      {connected && <ModelPicker inline agent={agent} profiles={props.profiles} connected={connected} send={send} onChoose={() => setModelOpen(false)} />}
    </div>}
    {selected && <div className="context-detail" aria-label={`Détail du déclencheur ${selected.name}`}><header><div><span>DÉCLENCHEUR</span><h2>{selected.name}</h2></div><button type="button" aria-label="Fermer le détail du déclencheur" onClick={() => setSelectedTrigger(null)}>×</button></header>
      <TriggerPanel {...props} triggers={[selected]} open={true} onToggle={() => {}} />
    </div>}
  </aside>;
}
