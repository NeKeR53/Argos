// Réglages et contexte de l'agent affiché, ouverts depuis le « ⋯ » de sa conversation :
// modèle, détails techniques, board, déclencheurs, skills, MCP et actions. Chaque section
// est repliée sur une ligne de résumé ; rien de tout ça n'encombre la conversation.

import { useEffect, useState, type ReactNode } from "react";
import type { AgentId, AgentStatus, Board, Capabilities, Command, ProfileInfo, Reply, Trigger } from "../protocol";
import { displayName, isAddressable, isRetirable } from "../state";
import { formatCost, levelLabel } from "../format";
import { modelLabel } from "../models";
import { BoardSection } from "./BoardSection";
import { ModelPicker } from "./ModelPicker";
import { TriggerPanel } from "./TriggerPanel";
import { WorkspacePicker } from "./WorkspacePicker";
import { Avatar } from "./Avatar";

interface Props {
  agent: AgentStatus;
  /** Agent d'un Worker, pour le rappeler dans les détails. */
  parent: AgentStatus | null;
  spent: number | undefined;
  board: Board | undefined;
  boardRevision: number;
  profiles: ProfileInfo[] | null;
  triggers: Trigger[] | undefined;
  revision: number;
  hooksBase: string | null;
  connected: boolean;
  send: (command: Command) => boolean;
  request: (command: Command) => Promise<Reply>;
  nameOf: (id: AgentId) => string;
  isLiving: (id: AgentId) => boolean;
  onSelect: (id: AgentId) => void;
  onRetire: (agent: AgentStatus) => void;
  onClose: () => void;
}

function skillName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  const last = parts[parts.length - 1];
  return (last?.toLowerCase() === "skill.md" ? parts[parts.length - 2] : last) || path;
}

/** Section repliable : un titre et un résumé d'une ligne, le détail au clic. */
function Section(props: { title: string; summary?: string; open?: boolean; children: ReactNode }) {
  return (
    <details className="section" open={props.open}>
      <summary>
        <span className="section-title">{props.title}</span>
        {props.summary && <span className="section-summary">{props.summary}</span>}
      </summary>
      <div className="section-body">{props.children}</div>
    </details>
  );
}

export function AgentContextPanel(props: Props) {
  const { agent, parent, spent, board, triggers, revision, connected, send, request } = props;
  const addressable = isAddressable(agent);
  const [boardOpen, setBoardOpen] = useState(false);
  const [triggersOpen, setTriggersOpen] = useState(false);
  const [capabilities, setCapabilities] = useState<{ config: Capabilities; argosEnabled: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!connected) return;
    let cancelled = false;
    setCapabilities(null);
    setError(null);
    request({ command: "capabilities", agent: agent.id })
      .then((reply) => {
        if (cancelled) return;
        if (reply.kind === "capabilities") setCapabilities({ config: reply.config, argosEnabled: reply.argos_enabled });
        else setError(reply.kind === "error" ? reply.message : "Réponse indisponible.");
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(String(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [agent.id, connected, request]);

  const { onClose } = props;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const mcpCount = capabilities ? capabilities.config.mcp.length + (capabilities.argosEnabled ? 1 : 0) : null;
  const status = agent.running ? (agent.busy ? "au travail" : "prêt") : "en pause";

  const rows: [string, string][] = [
    ["Moteur", agent.engine],
    ["Modèle", agent.model ?? "par défaut"],
    ...(agent.profile ? ([["Profil", agent.profile]] as [string, string][]) : []),
    ...(agent.charter?.scope ? ([["Périmètre", agent.charter.scope]] as [string, string][]) : []),
    ...(parent && agent.level === "worker" ? ([["Agent", displayName(parent)]] as [string, string][]) : []),
    ["Nœud", agent.node ?? "ce Mac"],
    ["État", status],
    ["Coût", spent != null ? `${formatCost(spent)} depuis l'ouverture` : "rien depuis l'ouverture"],
    ["Identifiant", agent.id],
  ];

  return (
    <div className="sheet-scrim" onMouseDown={props.onClose}>
    <aside
      className="sheet"
      role="dialog"
      aria-modal="true"
      aria-label={`Fiche de ${displayName(agent)}`}
      onMouseDown={(event) => event.stopPropagation()}
    >
      <header className="sheet-head">
        <Avatar agent={agent} size="lg" />
        <div className="sheet-who">
          <h2>{displayName(agent)}</h2>
          <p className="sheet-role">
            {agent.level === "assistant" ? "Reçoit tes demandes et les confie aux bons agents." : (agent.charter?.scope ?? "")}
          </p>
          <p className="sheet-now">
            <span className="sheet-dot" data-busy={agent.busy || undefined} aria-hidden="true" />
            {agent.busy ? "Au travail" : agent.running ? "Disponible" : "Au repos"}
          </p>
        </div>
        <button type="button" className="icon-button sheet-close" aria-label="Fermer la fiche" onClick={props.onClose}>
          ×
        </button>
      </header>
      <div className="context-body">
        {addressable && (
          <Section title="Modèle" summary={agent.model ? modelLabel(agent.model) : "par défaut"}>
            {connected ? (
              <ModelPicker agent={agent} profiles={props.profiles} connected={connected} send={send} />
            ) : (
              <p className="muted">Hors connexion</p>
            )}
          </Section>
        )}
        <Section title="Détails" summary={`${levelLabel[agent.level]} · ${status}`}>
          <dl className="facts">
            {rows.map(([label, value]) => (
              <div key={label} className="fact">
                <dt>{label}</dt>
                <dd>{value}</dd>
              </div>
            ))}
          </dl>
          {agent.level !== "worker" && <WorkspacePicker agent={agent} connected={connected} request={request} />}
        </Section>
        {agent.level === "agent" && (
          <BoardSection
            agent={agent}
            board={board}
            revision={props.boardRevision}
            connected={connected}
            open={boardOpen}
            onToggle={setBoardOpen}
            send={send}
            isLiving={props.isLiving}
            onSelect={(id) => {
              props.onClose();
              props.onSelect(id);
            }}
          />
        )}
        {agent.level !== "worker" && (
          <TriggerPanel
            agent={agent}
            triggers={triggers}
            revision={revision}
            hooksBase={props.hooksBase}
            connected={connected}
            open={triggersOpen}
            onToggle={setTriggersOpen}
            send={send}
            nameOf={props.nameOf}
          />
        )}
        <Section
          title="Skills"
          summary={capabilities ? `${capabilities.config.skills.length}` : error ? "indisponible" : connected ? "…" : "hors connexion"}
        >
          {error ? (
            <p role="status">Impossible de charger les capacités : {error}</p>
          ) : !capabilities ? (
            <p className="muted">{connected ? "Chargement…" : "Hors connexion"}</p>
          ) : capabilities.config.skills.length ? (
            <ul className="context-tags">
              {capabilities.config.skills.map((skill) => (
                <li key={skill} title={skill}>
                  {skillName(skill)}
                </li>
              ))}
            </ul>
          ) : (
            <p className="muted">Aucun skill configuré.</p>
          )}
        </Section>
        <Section title="MCP" summary={mcpCount != null ? `${mcpCount}` : error ? "indisponible" : connected ? "…" : "hors connexion"}>
          {error ? (
            <p>Impossible de charger les capacités.</p>
          ) : !capabilities ? (
            <p className="muted">{connected ? "Chargement…" : "Hors connexion"}</p>
          ) : (
            <>
              {mcpCount ? (
                <ul className="context-tags">
                  {capabilities.argosEnabled && <li>Argos</li>}
                  {capabilities.config.mcp.map((server) => (
                    <li key={server.name}>{server.name}</li>
                  ))}
                </ul>
              ) : (
                <p className="muted">Aucun serveur MCP configuré.</p>
              )}
              <p className="context-hint">Serveurs configurés au démarrage.</p>
            </>
          )}
        </Section>
        {addressable ? (
          <div className="context-actions">
            <button
              type="button"
              className="button"
              disabled={!connected || !agent.running}
              onClick={() => send({ command: "park", agent: agent.id })}
              title="Arrêt propre du process ; l'agent reprend sa session au prochain message"
            >
              Mettre en pause
            </button>
            <button
              type="button"
              className="button"
              disabled={!connected}
              onClick={() => send({ command: "stop", agent: agent.id })}
              title="Arrêt dur de l'agent et de tout son sous-arbre"
            >
              Arrêter
            </button>
            {isRetirable(agent) && (
              <button type="button" className="button button-danger-quiet" disabled={!connected} onClick={() => props.onRetire(agent)}>
                Retirer…
              </button>
            )}
          </div>
        ) : (
          <p className="context-hint">Un Worker exécute une tâche pour son Agent ; on ne lui parle pas directement.</p>
        )}
      </div>
    </aside>
    </div>
  );
}
