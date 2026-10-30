// Panneau de l'agent sélectionné : en-tête et actions de contrôle, onglets « Chat » et
// surfaces, board d'un Agent, fil des messages, saisie. Un Worker s'affiche en lecture
// seule (D2) et n'a pas de surface.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import type { AgentId, AgentStatus, Command, Origin, Reply, SurfaceDoc, SurfaceMeta } from "../protocol";
import {
  displayName,
  isAddressable,
  type ConsoleSend,
  type FeedItem,
  type ThreadEntry,
} from "../state";
import { formatUsage, outcomeLabel } from "../format";
import { classifyInput } from "../inputs";
import { threadItems, toolsSummary, type ToolEntry } from "../threadView";
import { Avatar } from "./Avatar";
import { SurfaceView } from "./SurfaceView";

interface Props {
  agent: AgentStatus;
  /** Agent parent d'un Worker, pour y renvoyer l'utilisateur. */
  parent: AgentStatus | null;
  thread: ThreadEntry[];
  /** Surfaces de l'agent (jamais pour un Worker), leur code et leur flux. */
  surfaces: SurfaceMeta[];
  surfaceDocs: Record<string, SurfaceDoc>;
  surfaceFeeds: Record<string, FeedItem[]>;
  surfaceRevision: number;
  /** Transcription partielle du push-to-talk, montrée dans le fil de l'Assistant. */
  dictation: string | null;
  connected: boolean;
  send: (command: Command) => boolean;
  request: (command: Command) => Promise<Reply>;
  /** Nom affichable d'un agent, pour dire qui a envoyé un message. */
  nameOf: (agent: AgentId) => string;
  onSelect: (agent: AgentId) => void;
  /** Panneau des réglages de l'agent (bouton « ⋯ »). */
  panelOpen: boolean;
  onTogglePanel: () => void;
  onOpenSettings: () => void;
}

/** Sous-titre de la conversation : ce que fait l'agent, sinon son rôle. */
function subtitle(agent: AgentStatus, thread: ThreadEntry[]): string {
  if (agent.busy) {
    const tool = [...thread].reverse().find((entry) => entry.kind === "tool");
    return tool && tool.kind === "tool" ? `En ce moment : ${tool.name}…` : "En train de répondre…";
  }
  if (agent.level === "assistant") return "Reçoit tes demandes et les confie aux bons agents.";
  return agent.charter?.scope ?? "";
}

export function ChatPanel(props: Props) {
  const { agent, parent, thread, connected, send, onSelect, nameOf } = props;
  const addressable = isAddressable(agent);
  // Incrémenté par « Répondre » sur une escalade : la saisie reprend le focus.
  const [replyTick, setReplyTick] = useState(0);
  // Onglet affiché, par agent : on retrouve la surface qu'on regardait.
  const [tabs, setTabs] = useState<Record<AgentId, string>>({});
  // Une surface supprimée laisse place au chat.
  const surface = props.surfaces.find((candidate) => candidate.id === tabs[agent.id]) ?? null;

  // Liste des surfaces : lue à la sélection, relue après chaque changement de l'agent
  // (rafales regroupées), tenue à jour entre-temps par `surface_updated`.
  const { surfaceRevision } = props;
  useEffect(() => {
    if (!connected || !addressable) return;
    const timer = setTimeout(() => send({ command: "surfaces", agent: agent.id }), 250);
    return () => clearTimeout(timer);
  }, [agent.id, addressable, surfaceRevision, connected, send]);

  return (
    <section className="chat" aria-label={`Fil de ${displayName(agent)}`}>
      <header className="chat-header">
        <Avatar agent={agent} size="sm" />
        <button
          type="button"
          className="chat-who"
          aria-expanded={props.panelOpen}
          title="Fiche de l'agent : rôle, modèle, historique"
          onClick={props.onTogglePanel}
        >
          <span className="chat-name">
            {displayName(agent)}
            <svg className="chevron" viewBox="0 0 24 24" aria-hidden="true">
              <path d="m7 10 5 5 5-5" />
            </svg>
          </span>
          <span className="chat-sub">{subtitle(agent, thread)}</span>
        </button>
        <span className="spacer" />
        <button type="button" className="icon-button" aria-label="Réglages" title="Réglages" onClick={props.onOpenSettings}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M10.3 3.5h3.4l.5 2.1a6.8 6.8 0 0 1 1.7 1l2.1-.7 1.7 2.9-1.6 1.5a6.8 6.8 0 0 1 0 2l1.6 1.5-1.7 2.9-2.1-.7a6.8 6.8 0 0 1-1.7 1l-.5 2.1h-3.4l-.5-2.1a6.8 6.8 0 0 1-1.7-1l-2.1.7-1.7-2.9 1.6-1.5a6.8 6.8 0 0 1 0-2L4.4 8.8l1.7-2.9 2.1.7a6.8 6.8 0 0 1 1.7-1z" />
            <circle cx="12" cy="12" r="2.6" />
          </svg>
        </button>
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

      {addressable && (
        <SurfaceTabs
          surfaces={props.surfaces}
          active={surface?.id ?? null}
          onChange={(tab) => setTabs((current) => ({ ...current, [agent.id]: tab ?? CHAT_TAB }))}
        />
      )}

      {surface ? (
        <SurfaceView
          key={surface.id}
          meta={surface}
          doc={props.surfaceDocs[surface.id]}
          feed={props.surfaceFeeds[surface.id] ?? EMPTY_FEED}
          busy={agent.busy}
          connected={connected}
          send={send}
          request={props.request}
        />
      ) : (
        <>
          {/* Clés par agent : défilement et brouillon ne passent pas d'un agent à l'autre. */}
          <Thread
            key={`thread-${agent.id}`}
            entries={thread}
            nameOf={nameOf}
            dictation={agent.level === "assistant" ? props.dictation : null}
            remote={!!agent.node}
            busy={agent.busy}
            onReply={() => setReplyTick((tick) => tick + 1)}
            canReply={addressable}
          />

          {addressable && (
            <Composer key={`composer-${agent.id}`} agent={agent} connected={connected} send={send} focusTick={replyTick} />
          )}
        </>
      )}
    </section>
  );
}

const CHAT_TAB = "chat";
const EMPTY_FEED: FeedItem[] = [];

/** « Chat » puis un onglet par surface, avec sa nature (cockpit ou artifact). */
function SurfaceTabs(props: {
  surfaces: SurfaceMeta[];
  active: string | null;
  onChange: (surface: string | null) => void;
}) {
  const { surfaces, active, onChange } = props;
  if (surfaces.length === 0) return null;
  return (
    <div className="tabs" role="tablist" aria-label="Chat et surfaces">
      <button type="button" role="tab" className="tab" aria-selected={active === null} onClick={() => onChange(null)}>
        Chat
      </button>
      {surfaces.map((surface) => (
        <button
          key={surface.id}
          type="button"
          role="tab"
          className="tab"
          aria-selected={active === surface.id}
          title={`${surface.title} (${surface.id}, v${surface.version})`}
          onClick={() => onChange(surface.id)}
        >
          <span className="tab-title">{surface.title}</span>
          <span className="surface-kind" data-kind={surface.kind}>
            {surface.kind}
          </span>
        </button>
      ))}
    </div>
  );
}

function Thread({
  entries,
  nameOf,
  dictation,
  remote,
  busy,
  onReply,
  canReply,
}: {
  remote: boolean;
  busy: boolean;
  /** « Répondre » sur une escalade (agents auxquels on peut parler seulement). */
  onReply: () => void;
  canReply: boolean;
  entries: ThreadEntry[];
  nameOf: (agent: AgentId) => string;
  /** Ce que l'utilisateur est en train de dire (fil de l'Assistant seulement). */
  dictation: string | null;
}) {
  const list = useRef<HTMLDivElement>(null);
  // Suit le bas du fil tant que l'utilisateur n'est pas remonté le relire.
  const pinned = useRef(true);

  useLayoutEffect(() => {
    const element = list.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [entries, dictation]);

  const items = threadItems(entries, busy);
  if (entries.length === 0 && dictation === null) {
    return (
      <div className="thread thread-empty">
        <p>Aucun message depuis la connexion.</p>
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
      {items.map((item) =>
        item.kind === "tools" ? (
          <ToolGroup key={item.key} tools={item.tools} live={item.live} />
        ) : (
          <Entry key={item.key} entry={item.entry} nameOf={nameOf} remote={remote} onReply={canReply ? onReply : undefined} />
        ),
      )}
      {dictation !== null && (
        <div className="entry entry-input entry-dictation" aria-live="polite">
          <div className="bubble">
            <div className="input-tag">dictée en cours</div>
            {dictation || "…"}
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Les outils d'un tour, sur une seule ligne repliée (« 3 outils utilisés ▸ ») ; en cours
 * de tour, la ligne montre le dernier outil appelé.
 */
function ToolGroup({ tools, live }: { tools: ToolEntry[]; live: boolean }) {
  const last = tools[tools.length - 1];
  return <details className="tool-group">
    <summary><span className="tool-indicator" data-active={live} aria-hidden="true" /><span className="tool-summary-text">{live ? `${last.name}…` : toolsSummary(tools.length)}</span><span className="tool-chevron" aria-hidden="true">▸</span></summary>
    <ul>{tools.map((tool) => <li key={tool.seq}><details className="tool-detail"><summary><span>{tool.name}</span><span className="tool-detail-state">{tool.finished === false && live ? "En cours" : tool.finished !== undefined ? "Terminé" : ""}</span></summary><div className="tool-detail-content">{tool.input && <section><strong>Commande / entrée</strong><pre>{tool.input}</pre></section>}{tool.output && <section><strong>Sortie</strong><pre>{tool.output}</pre></section>}{!tool.input && !tool.output && <p>Les détails ne sont pas disponibles.</p>}{tool.finished === false && live && !tool.output && <p>En attente de la sortie…</p>}</div></details></li>)}</ul>
  </details>;
}

/** Rend les chemins de documents directement à leur place dans le message. */
function DocumentText({ text, remote }: { text: string; remote: boolean }) {
  const [error, setError] = useState<string | null>(null);
  const pattern = /\[([^\]\n]+)\]\((<\/[^>\n]+>|\/[^)\n]+)\)|\/(?:[^\s`<>()[\]]+\/)*[^\s`<>()[\]]+/g;
  const parts: ReactNode[] = [];
  let end = 0;
  for (const match of text.matchAll(pattern)) {
    const rawPath = (match[2] ?? match[0]).replace(/^<|>$/g, "");
    const path = rawPath.replace(/[.,;:!?]+$/, "");
    if (!/^\/(?!\/).+\.(pdf|docx?|odt|rtf|txt|md|xlsx?|ods|pptx?|odp|csv|png|jpe?g)$/i.test(path) && !(match[1] && /^\/(?!\/).+\/$/.test(path))) continue;
    const start = match.index;
    parts.push(text.slice(end, start));
    const label = match[1] ?? path;
    parts.push(<button key={start} type="button" className="document-link" disabled={remote || !isTauri()} title={remote ? "Ce document se trouve sur un autre nœud" : !isTauri() ? "Disponible dans l’application Argos" : path} onClick={() => { setError(null); void invoke("open_document", { path }).catch((reason: unknown) => setError(String(reason))); }}>{label}</button>);
    if (!match[1]) parts.push(rawPath.slice(path.length));
    end = start + match[0].length;
  }
  parts.push(text.slice(end));
  return <>{parts}{error && <p className="document-error" role="alert">{error}</p>}</>;
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

function Entry({
  entry,
  nameOf,
  remote,
  onReply,
}: {
  entry: ThreadEntry;
  nameOf: (agent: AgentId) => string;
  remote: boolean;
  onReply?: () => void;
}) {
  switch (entry.kind) {
    case "escalation":
      // Surligné avec discrétion : l'agent attend une réponse de l'utilisateur.
      return (
        <div className="entry entry-escalation">
          <div className="escalation-card">
            <div className="input-tag">a besoin de vous</div>
            <div className="escalation-text">{entry.text}</div>
            {onReply && (
              <button type="button" className="button" onClick={onReply}>
                Répondre
              </button>
            )}
          </div>
        </div>
      );
    case "input":
      return (
        <InputEntry
          text={entry.text}
          from={entry.from}
          sent={entry.console}
          voice={entry.voice ?? false}
          nameOf={nameOf}
        />
      );
    case "agent":
      return (
        <div className="entry entry-agent">
          <div className="bubble"><DocumentText text={entry.text} remote={remote} /></div>
        </div>
      );
    case "streaming":
      // Le texte s'écrit ; le message complet le remplacera.
      return (
        <div className="entry entry-agent entry-streaming" aria-busy="true">
          <div className="bubble">
            {entry.text}
            <span className="caret" aria-hidden />
          </div>
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
  voice,
  nameOf,
}: {
  text: string;
  from: Origin | null;
  sent: ConsoleSend | null;
  /** Demande faite à la voix. */
  voice: boolean;
  nameOf: (agent: AgentId) => string;
}) {
  const { category, tag, body } = classifyInput(text);
  const note = sent ? deliveryLabel[sent.delivery] : null;
  const label = category ? tag : voice ? "à la voix" : sent ? null : `reçu · ${senderLabel(from, nameOf)}`;
  return (
    <div
      className="entry entry-input"
      data-category={category ?? (sent || voice ? "user" : "free")}
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

function Composer({
  agent,
  connected,
  send,
  focusTick,
}: Pick<Props, "agent" | "connected" | "send"> & { focusTick: number }) {
  const [text, setText] = useState("");
  const field = useRef<HTMLTextAreaElement>(null);

  // Nouvel agent affiché, ou « Répondre » : la saisie prend le focus.
  useEffect(() => field.current?.focus(), [focusTick]);

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
        rows={1}
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
