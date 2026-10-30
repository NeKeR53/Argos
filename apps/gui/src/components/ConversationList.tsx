// Liste des conversations, façon messagerie : l'Assistant épinglé, puis les Agents, chacun
// avec son avatar, l'aperçu de son dernier message et une pastille quand il a besoin de
// l'utilisateur. Les Workers n'y sont pas : on ne leur parle jamais (D2).

import { useState } from "react";
import type { AgentId, AgentStatus } from "../protocol";
import { displayName, type ThreadEntry } from "../state";
import { Avatar } from "./Avatar";

interface Props {
  agents: AgentStatus[];
  threads: Record<AgentId, ThreadEntry[]>;
  selected: AgentId | null;
  /** Escalades non vues, par agent (voir `attention`). */
  alerts: Record<AgentId, number>;
  onSelect: (agent: AgentId) => void;
}

/** Aperçu d'une conversation : ce qui s'y est dit en dernier, sur une ligne. */
export function preview(agent: AgentStatus, thread: ThreadEntry[] | undefined): string {
  for (let index = (thread?.length ?? 0) - 1; index >= 0; index -= 1) {
    const entry = thread![index];
    if (agent.busy && entry.kind === "tool") return `${entry.name}…`;
    if (entry.kind === "escalation") return entry.text;
    if (entry.kind === "agent" || entry.kind === "streaming") return entry.text;
    if (entry.kind === "input") return `Vous : ${entry.text}`;
  }
  if (agent.busy) return "Au travail…";
  return agent.charter?.scope ?? (agent.level === "assistant" ? "Reçoit vos demandes et les confie aux bons agents." : "");
}

export function ConversationList({ agents, threads, selected, alerts, onSelect }: Props) {
  const [query, setQuery] = useState("");
  const wanted = query.trim().toLowerCase();
  const matches = (agent: AgentStatus) =>
    !wanted || displayName(agent).toLowerCase().includes(wanted) || (agent.charter?.scope ?? "").toLowerCase().includes(wanted);
  const assistant = agents.find((agent) => agent.level === "assistant");
  const others = agents.filter((agent) => agent.level === "agent" && matches(agent));

  const row = (agent: AgentStatus) => {
    const alert = alerts[agent.id] ?? 0;
    const text = preview(agent, threads[agent.id]);
    return (
      <button
        key={agent.id}
        type="button"
        className="conv"
        data-needs={alert > 0 || undefined}
        aria-current={agent.id === selected || undefined}
        onClick={() => onSelect(agent.id)}
      >
        <Avatar agent={agent} />
        <span className="conv-meta">
          <span className="conv-name">{displayName(agent)}</span>
          <span className="conv-preview">
            {agent.busy && <span className="spin" aria-label="au travail" />}
            <span className="conv-text">{text}</span>
          </span>
        </span>
        {alert > 0 && (
          <span className="conv-badge" aria-label={`${alert} message${alert > 1 ? "s" : ""} en attente de vous`}>
            {alert}
          </span>
        )}
      </button>
    );
  };

  return (
    <nav className="conversations" aria-label="Conversations">
      <label className="conv-search">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="11" cy="11" r="6.5" />
          <path d="m16 16 4 4" />
        </svg>
        <span className="sr-only">Rechercher</span>
        <input type="search" placeholder="Rechercher" value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>
      <div className="conv-list">
        {assistant && matches(assistant) && row(assistant)}
        {others.length > 0 && <h2 className="conv-section">Agents</h2>}
        {others.map(row)}
        {agents.length === 0 && <p className="conv-empty">En attente du daemon…</p>}
      </div>
    </nav>
  );
}
