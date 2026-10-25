// Pastille d'exécution : process arrêté (creuse), en marche (pleine), tour en cours (pulsée).

import type { AgentStatus } from "../protocol";

export function StatusDot({ agent }: { agent: AgentStatus }) {
  const status = agent.busy ? "busy" : agent.running ? "running" : "idle";
  const label = {
    busy: "process en marche, tour en cours",
    running: "process en marche",
    idle: "pas de process",
  }[status];
  return <span className="dot" data-status={status} role="img" aria-label={label} title={label} />;
}
