// Carte des machines (D48) : ce Mac, puis chaque nœud pair relié par son lien, avec les
// Agents qui y tournent. Un Worker tourne sur le nœud de son Agent : il est compté sous
// lui, pas affiché à part.

import type { AgentId, AgentStatus, NodeStatus } from "../protocol";
import { displayName } from "../state";
import { Avatar } from "./Avatar";

/** Une machine de la carte et ce qui y tourne. */
export interface Machine {
  /** `null` pour le nœud de cette app. */
  name: string | null;
  /** Le nœud de l'app est toujours joignable ; un nœud absent de la config ne l'est pas. */
  connected: boolean;
  error: string | null;
  /** Déclaré dans `[nodes.<nom>]` ; faux pour un nœud qu'un agent cite encore. */
  declared: boolean;
  agents: { agent: AgentStatus; workers: number }[];
}

/** Range l'Assistant et les Agents par machine : ce nœud d'abord, puis les autres par nom. */
export function machinesOf(agents: Record<AgentId, AgentStatus>, nodes: NodeStatus[]): Machine[] {
  const hub: Machine = { name: null, connected: true, error: null, declared: true, agents: [] };
  const byName = new Map<string, Machine>();
  for (const node of nodes) {
    byName.set(node.name, { name: node.name, connected: node.connected, error: node.error, declared: true, agents: [] });
  }
  const workers = new Map<AgentId, number>();
  for (const agent of Object.values(agents)) {
    if (agent.level === "worker" && agent.parent) workers.set(agent.parent, (workers.get(agent.parent) ?? 0) + 1);
  }
  const placed = Object.values(agents)
    .filter((agent) => agent.level !== "worker")
    .sort((a, b) => (a.level === "assistant" ? -1 : b.level === "assistant" ? 1 : displayName(a).localeCompare(displayName(b))));
  for (const agent of placed) {
    let machine = hub;
    if (agent.node) {
      machine = byName.get(agent.node) ?? { name: agent.node, connected: false, error: null, declared: false, agents: [] };
      byName.set(agent.node, machine);
    }
    machine.agents.push({ agent, workers: workers.get(agent.id) ?? 0 });
  }
  return [hub, ...[...byName.values()].sort((a, b) => a.name!.localeCompare(b.name!))];
}

export function NodeMap({ agents, nodes, onSelect }: {
  agents: Record<AgentId, AgentStatus>;
  nodes: NodeStatus[];
  onSelect: (agent: AgentId) => void;
}) {
  const [hub, ...remote] = machinesOf(agents, nodes);
  return (
    <div className="node-map">
      <MachineCard machine={hub} onSelect={onSelect} />
      {remote.length > 0 && (
        <ul className="node-map-links">
          {remote.map((machine) => (
            <li key={machine.name} className="node-map-branch" data-connected={machine.connected}>
              <span className="node-map-link" aria-hidden />
              <span className="node-map-link-label">{linkLabel(machine)}</span>
              <MachineCard machine={machine} onSelect={onSelect} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function linkLabel(machine: Machine): string {
  if (!machine.declared) return "non déclaré";
  if (machine.connected) return "relié";
  return "injoignable";
}

function MachineCard({ machine, onSelect }: { machine: Machine; onSelect: (agent: AgentId) => void }) {
  const hub = machine.name === null;
  return (
    <section className="machine" data-connected={machine.connected} data-hub={hub || undefined} aria-label={hub ? "Ce nœud" : `Nœud ${machine.name}`}>
      <header className="machine-head">
        <span className="node-dot" aria-hidden />
        <span className="machine-name">{hub ? "Ce Mac" : machine.name}</span>
        <span className="machine-role">{hub ? "ici" : "nœud"}</span>
        <span className="machine-count">{agentCount(machine.agents.length)}</span>
      </header>
      {machine.error && <p className="machine-error">{machine.error}</p>}
      {machine.agents.length === 0 ? (
        <p className="machine-empty">Aucun agent ici.</p>
      ) : (
        <ul className="machine-agents">
          {machine.agents.map(({ agent, workers }) => (
            <li key={agent.id}>
              <button type="button" className="machine-agent" onClick={() => onSelect(agent.id)} title={agent.workdir}>
                <Avatar agent={agent} size="sm" />
                <span className="machine-agent-name">{displayName(agent)}</span>
                {workers > 0 && <span className="machine-agent-workers">{workers === 1 ? "1 worker" : `${workers} workers`}</span>}
                <span className="machine-agent-state" data-state={agent.busy ? "busy" : agent.running ? "running" : "parked"}>
                  {agent.busy ? "au travail" : agent.running ? "actif" : "parké"}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function agentCount(count: number): string {
  return count === 0 ? "vide" : count === 1 ? "1 agent" : `${count} agents`;
}
