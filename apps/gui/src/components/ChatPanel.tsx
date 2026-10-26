// Panneau de l'agent sélectionné : en-tête et actions de contrôle, board d'un Agent,
// fil des messages, saisie. Un Worker s'affiche en lecture seule (D2).

import { useEffect, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import type { AgentId, AgentStatus, Board, Command, Origin } from "../protocol";
import { displayName, isAddressable, isRetirable, shortId, type ConsoleSend, type ThreadEntry } from "../state";
import { engineLabel, formatCost, formatUsage, levelLabel, outcomeLabel } from "../format";
import { classifyInput } from "../inputs";
import { AgentFlags } from "./AgentTree";
import { BoardSection } from "./BoardSection";
import { StatusDot } from "./StatusDot";

interface Props {
  agent: AgentStatus;
  /** Agent parent d'un Worker, pour y renvoyer l'utilisateur. */
  parent: AgentStatus | null;
  thread: ThreadEntry[];
  spent: number | undefined;
  /** Board d'un Agent (absent pour les autres niveaux ou avant la première lecture). */
  board: Board | undefined;
  boardRevision: number;
  connected: boolean;
  send: (command: Command) => boolean;
  isLiving: (agent: AgentId) => boolean;
  /** Nom affichable d'un agent, pour dire qui a envoyé un message. */
  nameOf: (agent: AgentId) => string;
  onRetire: (agent: AgentStatus) => void;
  onSelect: (agent: AgentId) => void;
}

export function ChatPanel(props: Props) {
  const { agent, parent, thread, spent, connected, send, onRetire, onSelect, nameOf } = props;
  const addressable = isAddressable(agent);
  // Board ouvert ou replié : le choix suit l'utilisateur d'un Agent à l'autre.
  const [boardOpen, setBoardOpen] = useState(true);
  return (
    <section className="chat" aria-label={`Fil de ${displayName(agent)}`}>
      <header className="chat-header">
        <div className="chat-title">
          <StatusDot agent={agent} />
          <h1>{displayName(agent)}</h1>
          <span className="level-tag" data-level={agent.level}>
            {levelLabel[agent.level]}
          </span>
          <AgentFlags agent={agent} />
        </div>
        <div className="chat-meta">
          <span>{engineLabel(agent)}</span>
          {/* Un Worker porte déjà son id court en titre. */}
          {agent.level !== "worker" && (
            <span className="mono" title={agent.id}>
              {shortId(agent.id)}
            </span>
          )}
          {agent.charter && <span title="Périmètre de la charte">{agent.charter.scope}</span>}
          {spent != null && <span title="Coût cumulé depuis la connexion de la console">{formatCost(spent)}</span>}
        </div>
        {addressable && (
          <div className="chat-actions">
            <button
              type="button"
              className="button"
              disabled={!connected || !agent.running}
              onClick={() => send({ command: "park", agent: agent.id })}
              title="Arrêt propre du process ; l'agent reprendra sa session au prochain message"
            >
              Parker
            </button>
            <button
              type="button"
              className="button"
              disabled={!connected}
              onClick={() => send({ command: "stop", agent: agent.id })}
              title="Arrêt dur de l'agent et de tout son sous-arbre"
            >
              Stopper
            </button>
            {isRetirable(agent) && (
              <button
                type="button"
                className="button button-danger-quiet"
                disabled={!connected}
                onClick={() => onRetire(agent)}
                title="Retrait définitif de l'Agent et de ses Workers"
              >
                Retirer…
              </button>
            )}
          </div>
        )}
      </header>

      {!addressable && (
        <p className="readonly-banner" role="note">
          Worker en lecture seule : l'utilisateur ne s'adresse jamais à un Worker, il passe par son Agent.
          {parent && (
            <>
              {" "}
              <button type="button" className="link" onClick={() => onSelect(parent.id)}>
                Ouvrir {displayName(parent)}
              </button>
            </>
          )}
        </p>
      )}

      {agent.level === "agent" && (
        <BoardSection
          agent={agent}
          board={props.board}
          revision={props.boardRevision}
          connected={connected}
          open={boardOpen}
          onToggle={setBoardOpen}
          send={send}
          isLiving={props.isLiving}
          onSelect={onSelect}
        />
      )}

      {/* Clés par agent : défilement et brouillon ne passent pas d'un agent à l'autre. */}
      <Thread key={`thread-${agent.id}`} entries={thread} nameOf={nameOf} />

      {addressable && <Composer key={`composer-${agent.id}`} agent={agent} connected={connected} send={send} />}
    </section>
  );
}

function Thread({ entries, nameOf }: { entries: ThreadEntry[]; nameOf: (agent: AgentId) => string }) {
  const list = useRef<HTMLDivElement>(null);
  // Suit le bas du fil tant que l'utilisateur n'est pas remonté le relire.
  const pinned = useRef(true);

  useLayoutEffect(() => {
    const element = list.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [entries]);

  if (entries.length === 0) {
    return (
      <div className="thread thread-empty">
        <p>Aucun message depuis la connexion.</p>
        <p className="muted">Le daemon ne garde pas d'historique : le fil commence à l'ouverture de la console.</p>
      </div>
    );
  }
  return (
    <div
      ref={list}
      className="thread"
      role="log"
      aria-live="polite"
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 40;
      }}
    >
      {entries.map((entry) => (
        <Entry key={entry.seq} entry={entry} nameOf={nameOf} />
      ))}
    </div>
  );
}

const deliveryLabel = {
  pending: "envoi…",
  delivered: null,
  failed: "non remis",
  lost: "connexion perdue : remise incertaine",
} as const;

/** Qui a envoyé un message reçu sans préfixe et sans venir de cette console. */
function senderLabel(from: Origin | null, nameOf: (agent: AgentId) => string): string {
  switch (from?.kind) {
    case "user":
      return "utilisateur (autre console)";
    case "agent":
      return nameOf(from.agent);
    case "argos":
      return "Argos";
    default:
      return "expéditeur inconnu";
  }
}

function Entry({ entry, nameOf }: { entry: ThreadEntry; nameOf: (agent: AgentId) => string }) {
  switch (entry.kind) {
    case "input":
      return <InputEntry text={entry.text} from={entry.from} sent={entry.console} nameOf={nameOf} />;
    case "agent":
      return (
        <div className="entry entry-agent">
          <div className="bubble">{entry.text}</div>
        </div>
      );
    case "tool":
      return (
        <div className="entry entry-tool">
          <span className="muted">outil</span> <span className="mono">{entry.name}</span>
        </div>
      );
    case "turn": {
      const usage = formatUsage(entry.usage);
      return (
        <div className="entry entry-turn" data-outcome={entry.outcome.status}>
          <span>{outcomeLabel(entry.outcome)}</span>
          {usage && <span className="muted">{usage}</span>}
        </div>
      );
    }
    case "exited":
      return (
        <div className="entry entry-turn" data-outcome="exited">
          <span>Process terminé{entry.code != null ? ` (code ${entry.code})` : ""}</span>
        </div>
      );
  }
}

/**
 * Ce que l'agent reçoit. Un message préfixé par un outil (mission, rapport, escalade…)
 * se distingue d'un message de l'utilisateur par son étiquette et son cadre.
 */
function InputEntry({
  text,
  from,
  sent,
  nameOf,
}: {
  text: string;
  from: Origin | null;
  sent: ConsoleSend | null;
  nameOf: (agent: AgentId) => string;
}) {
  const { category, tag, body } = classifyInput(text);
  const note = sent ? deliveryLabel[sent.delivery] : null;
  const label = category ? tag : sent ? null : `reçu · ${senderLabel(from, nameOf)}`;
  return (
    <div
      className="entry entry-input"
      data-category={category ?? (sent ? "user" : "free")}
      data-tagged={category ? "" : undefined}
      data-delivery={sent?.delivery}
    >
      <div className="bubble">
        {label && <div className="input-tag">{label}</div>}
        {body}
      </div>
      {note && <div className="entry-note">{note}</div>}
    </div>
  );
}

function Composer({ agent, connected, send }: Pick<Props, "agent" | "connected" | "send">) {
  const [text, setText] = useState("");
  const field = useRef<HTMLTextAreaElement>(null);

  // Nouvel agent affiché : la saisie prend le focus.
  useEffect(() => field.current?.focus(), []);

  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    const message = text.trim();
    if (!message || !connected) return;
    if (send({ command: "send", agent: agent.id, text: message })) setText("");
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    // Entrée envoie, Maj+Entrée va à la ligne (sauf composition IME en cours).
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) submit(event);
  };

  return (
    <form className="composer" onSubmit={submit}>
      <textarea
        ref={field}
        value={text}
        rows={2}
        placeholder={connected ? `Message à ${displayName(agent)}…` : "Hors connexion"}
        aria-label={`Message à ${displayName(agent)}`}
        disabled={!connected}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={onKeyDown}
      />
      <div className="composer-actions">
        {agent.busy && (
          <button
            type="button"
            className="button"
            disabled={!connected}
            onClick={() => send({ command: "interrupt", agent: agent.id })}
          >
            Interrompre
          </button>
        )}
        <button type="submit" className="button button-primary" disabled={!connected || !text.trim()}>
          Envoyer
        </button>
      </div>
    </form>
  );
}
