import { useEffect, useState } from "react";
import type { AgentStatus, Capabilities, Command, Reply } from "../protocol";

export function CapabilitiesView({ agent, connected, request }: {
  agent: AgentStatus;
  connected: boolean;
  request: (command: Command) => Promise<Reply>;
}) {
  const [config, setConfig] = useState<Capabilities | null>(null);
  const [argosEnabled, setArgosEnabled] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!connected) return;
    let cancelled = false;
    request({ command: "capabilities", agent: agent.id }).then((reply) => {
      if (cancelled) return;
      if (reply.kind === "capabilities") { setError(null); setConfig(reply.config); setArgosEnabled(reply.argos_enabled); }
      else if (reply.kind === "error") setError(reply.message);
    }).catch((cause: unknown) => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; };
  }, [agent, connected, request]);
  return <section aria-label="Skills et MCP">
    <h3>Skills et MCP</h3>
    <p className="muted">{agent.level === "worker" ? "Configuration héritée de l’Agent." : "Configuration transmise à l’Agent et à ses Workers au démarrage."}</p>
    {error && <p role="alert">{error}</p>}
    {!config && !error && <p>Chargement…</p>}
    {config && <>
      <h4>Skills</h4>
      {config.skills.length ? <ul>{config.skills.map((path) => <li key={path} className="mono">{path}</li>)}</ul> : <p>Aucun skill configuré.</p>}
      <h4>Serveurs MCP</h4>
      {argosEnabled || config.mcp.length ? <ul>{argosEnabled && <li>Argos</li>}{config.mcp.map((server) => <li key={server.name}>{server.name} · {server.type}</li>)}</ul> : <p>Aucun serveur MCP configuré.</p>}
      <p className="muted">La présence ici indique la configuration, pas l’état de connexion des serveurs.</p>
    </>}
  </section>;
}
