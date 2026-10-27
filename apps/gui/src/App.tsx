// Console Argos (phase 1) : arbre des agents, chat avec l'agent sélectionné, panic stop.
// Une seule fenêtre ; l'orbe, l'overlay et les fenêtres flottantes viennent en phase 3.

import { useCallback, useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { resolveHooksBase } from "./hooks";
import "./App.css";
import type { AgentId, AgentStatus, NodeStatus, SurfaceMeta } from "./protocol";
import {
  assistantOf,
  displayName,
  selectedAgent,
  shortId,
  treeOf,
  type Connection,
  type Escalation,
  type Notice,
} from "./state";
import { useDaemon } from "./useDaemon";
import { simulateVoice, useVoiceView } from "./useVoiceView";
import { voiceReason, voiceStatusLabel, type VoiceStatus } from "./voice";
import { AgentTree } from "./components/AgentTree";
import { ChatPanel } from "./components/ChatPanel";
import { ConfirmDialog } from "./components/ConfirmDialog";

/** Raccourci du panic stop dans l'app : ⌘⇧X (Ctrl+Maj+X hors macOS). */
function isPanicShortcut(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "x";
}

const isMac = navigator.platform.toLowerCase().startsWith("mac");
const PANIC_SHORTCUT = isMac ? "⌘⇧X" : "Ctrl+Maj+X";
/** Liste vide stable : évite de relancer les effets des surfaces à chaque rendu. */
const NO_SURFACES: SurfaceMeta[] = [];
/** Cadence de relecture de l'état des nœuds (pas d'événement du daemon pour ça). */
const NODES_REFRESH_MS = 15_000;
/** Essai de la voix sans micro (`voice_simulate`) : app Tauri de dev seulement. */
const VOICE_TRIAL = import.meta.env.DEV && isTauri();

export default function App() {
  const { state, dispatch, send } = useDaemon();
  useVoiceView(dispatch);
  const [hooksBase, setHooksBase] = useState<string | null>(null);
  useEffect(() => {
    void resolveHooksBase().then(setHooksBase);
  }, []);
  const [panicAsked, setPanicAsked] = useState(false);
  const [retiring, setRetiring] = useState<AgentStatus | null>(null);
  const connected = state.connection.status === "connected";
  const agent = selectedAgent(state);
  const roots = treeOf(state);

  // Le daemon ne signale pas les changements d'état des nœuds : on les relit.
  useEffect(() => {
    if (!connected) return;
    send({ command: "nodes" });
    const timer = setInterval(() => send({ command: "nodes" }), NODES_REFRESH_MS);
    return () => clearInterval(timer);
  }, [connected, send]);

  const panic = useCallback(() => {
    setPanicAsked(false);
    send({ command: "panic_stop" });
  }, [send]);

  // Raccourci : un premier appui demande confirmation, un second confirme. Dans l'app
  // Tauri, le raccourci global (géré par l'orbe) le remplace.
  useEffect(() => {
    if (isTauri()) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isPanicShortcut(event)) return;
      event.preventDefault();
      if (!connected) return;
      if (panicAsked) panic();
      else setPanicAsked(true);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [connected, panicAsked, panic]);

  const select = (id: AgentId) => dispatch({ type: "select", agent: id });
  const isLiving = (id: AgentId) => id in state.agents;
  const assistant = assistantOf(state);
  // La marque d'escalade reste sur l'Assistant jusqu'à ce qu'on le consulte.
  const alerts: Record<AgentId, number> = assistant ? { [assistant.id]: state.unseenEscalations } : {};

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">Argos</span>
        <ConnectionBadge connection={state.connection} />
        {state.voice.status && <VoiceBadge status={state.voice.status} />}
        <NodeDots nodes={state.nodes} />
        <span className="spacer" />
        {VOICE_TRIAL && <VoiceTrial disabled={!connected} />}
        <button
          type="button"
          className="button panic"
          disabled={!connected}
          onClick={() => setPanicAsked(true)}
          title={`Arrêt dur de tout l'arbre (${PANIC_SHORTCUT})`}
        >
          Panic stop <kbd>{PANIC_SHORTCUT}</kbd>
        </button>
      </header>

      <div className="body">
        <aside className="sidebar" aria-label="Agents">
          <div className="sidebar-head">
            <span>Agents</span>
            <span className="muted">{state.order.length || ""}</span>
          </div>
          {roots.length > 0 ? (
            <AgentTree roots={roots} selected={agent?.id ?? null} alerts={alerts} onSelect={select} />
          ) : (
            <p className="sidebar-empty muted">{connected ? "Arbre vide." : "En attente du daemon…"}</p>
          )}
        </aside>

        <main className="main">
          <Escalations
            escalations={state.escalations}
            isLiving={isLiving}
            onOpen={(escalation) => {
              // L'Agent s'il est connu, sinon l'Assistant qui a reçu l'escalade.
              const target = escalation.agent && isLiving(escalation.agent) ? escalation.agent : assistant?.id;
              if (target) select(target);
              dispatch({ type: "dismiss_escalation", escalation: escalation.id });
            }}
            onDismiss={(id) => dispatch({ type: "dismiss_escalation", escalation: id })}
          />
          <Notices
            notices={state.notices}
            agents={state.agents}
            onOpen={select}
            onDismiss={(id) => dispatch({ type: "dismiss", notice: id })}
          />
          {agent ? (
            <ChatPanel
              agent={agent}
              parent={agent.parent ? (state.agents[agent.parent] ?? null) : null}
              thread={state.threads[agent.id] ?? []}
              spent={state.spent[agent.id]}
              board={state.boards[agent.id]}
              boardRevision={state.boardRevisions[agent.id] ?? 0}
              surfaces={state.surfaces[agent.id] ?? NO_SURFACES}
              surfaceDocs={state.surfaceDocs}
              surfaceFeeds={state.surfaceFeeds}
              surfaceRevision={state.surfaceRevisions[agent.id] ?? 0}
              triggers={state.triggers[agent.id]}
              triggerRevision={state.triggerRevisions[agent.id] ?? 0}
              hooksBase={hooksBase}
              dictation={state.voice.dictation}
              connected={connected}
              send={send}
              isLiving={isLiving}
              nameOf={(id) => (state.agents[id] ? displayName(state.agents[id]) : shortId(id))}
              onRetire={setRetiring}
              onSelect={select}
            />
          ) : (
            <Placeholder connection={state.connection} />
          )}
        </main>
      </div>

      {panicAsked && (
        <ConfirmDialog
          title="Panic stop"
          confirmLabel="Tout arrêter"
          danger
          onConfirm={panic}
          onCancel={() => setPanicAsked(false)}
        >
          <p>
            Arrêt dur de tout l'arbre : l'Assistant, les Agents et leurs Workers. Les sessions se reprendront au
            prochain message.
          </p>
          <p className="muted">Entrée ou {PANIC_SHORTCUT} confirme, Échap annule.</p>
        </ConfirmDialog>
      )}

      {retiring && (
        <ConfirmDialog
          title={`Retirer ${displayName(retiring)} ?`}
          confirmLabel="Retirer définitivement"
          danger
          onConfirm={() => {
            send({ command: "retire", agent: retiring.id });
            setRetiring(null);
          }}
          onCancel={() => setRetiring(null)}
        >
          <p>L'Agent et tous ses Workers sont arrêtés puis retirés du registre. Le retrait est définitif.</p>
        </ConfirmDialog>
      )}
    </div>
  );
}

/** Une pastille par nœud déclaré (D32) ; rien s'il n'y en a pas. */
function NodeDots({ nodes }: { nodes: NodeStatus[] | null }) {
  if (!nodes || nodes.length === 0) return null;
  return (
    <span className="nodes" aria-label="Nœuds">
      {nodes.map((node) => (
        <span
          key={node.name}
          className="node"
          data-connected={node.connected}
          title={node.connected ? `${node.name} : joignable` : `${node.name} : injoignable${node.error ? ` (${node.error})` : ""}`}
        >
          <span className="node-dot" aria-hidden />
          {node.name}
        </span>
      ))}
    </span>
  );
}

/** État de la voix ; s'il manque, la raison est dans l'infobulle (et dans un avis). */
function VoiceBadge({ status }: { status: VoiceStatus }) {
  const reason = voiceReason(status);
  return (
    <span className="voice-badge" data-status={status.status} title={reason ?? undefined}>
      {voiceStatusLabel(status)}
    </span>
  );
}

/** Dev seulement : une phrase « dite » sans micro, pour essayer toute la boucle vocale. */
function VoiceTrial({ disabled }: { disabled: boolean }) {
  const [text, setText] = useState("Dis-moi bonjour en une phrase");
  return (
    <form
      className="voice-trial"
      onSubmit={(event) => {
        event.preventDefault();
        if (text.trim()) simulateVoice(text.trim());
      }}
    >
      <input
        value={text}
        aria-label="Phrase à simuler"
        onChange={(event) => setText(event.target.value)}
        disabled={disabled}
      />
      <button type="submit" className="button" disabled={disabled} title="voice_simulate (dev)">
        Simuler la voix
      </button>
    </form>
  );
}

function ConnectionBadge({ connection }: { connection: Connection }) {
  const seconds = (ms: number) => `${Math.round(ms / 1000) || 1} s`;
  const [label, detail] = ((): [string, string] => {
    switch (connection.status) {
      case "connecting":
        return ["Connexion…", ""];
      case "connected":
        return ["Connecté", ""];
      case "reconnecting":
        return ["Reconnexion", `${connection.reason} · essai ${connection.attempt}, nouvel essai dans ${seconds(connection.retryInMs)}`];
      case "unauthorized":
        return ["Jeton refusé", `le daemon refuse le jeton · nouvel essai dans ${seconds(connection.retryInMs)}`];
      case "unconfigured":
        return ["Daemon non configuré", ""];
    }
  })();
  return (
    <span className="connection" data-status={connection.status} role="status">
      <span className="connection-dot" aria-hidden />
      <span className="connection-label">{label}</span>
      {detail && <span className="connection-detail">{detail}</span>}
    </span>
  );
}

/** Erreurs des commandes et informations du daemon, avec un lien vers l'agent concerné. */
function Notices(props: {
  notices: Notice[];
  agents: Record<AgentId, AgentStatus>;
  onOpen: (agent: AgentId) => void;
  onDismiss: (id: number) => void;
}) {
  const { notices, agents, onOpen, onDismiss } = props;
  if (notices.length === 0) return null;
  return (
    <div className="notices" role="alert">
      {notices.map((notice) => (
        <div key={notice.id} className="notice" data-tone={notice.tone}>
          <span>{notice.text}</span>
          {notice.agent && agents[notice.agent] && (
            <button type="button" className="link" onClick={() => onOpen(notice.agent!)}>
              Ouvrir {displayName(agents[notice.agent])}
            </button>
          )}
          <button type="button" className="notice-close" aria-label="Fermer" onClick={() => onDismiss(notice.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

/** Escalades vers l'utilisateur (D12) : visibles quel que soit l'agent affiché. */
function Escalations(props: {
  escalations: Escalation[];
  isLiving: (id: AgentId) => boolean;
  onOpen: (escalation: Escalation) => void;
  onDismiss: (id: number) => void;
}) {
  if (props.escalations.length === 0) return null;
  return (
    <div className="escalations" role="alert">
      {props.escalations.map((escalation) => (
        <div key={escalation.id} className="escalation">
          <div className="escalation-text">
            <strong>L'Agent {escalation.name} a besoin de vous</strong>
            {escalation.text && <span className="escalation-body">{escalation.text}</span>}
          </div>
          <button type="button" className="button" onClick={() => props.onOpen(escalation)}>
            {escalation.agent && props.isLiving(escalation.agent) ? `Ouvrir ${escalation.name}` : "Voir l'Assistant"}
          </button>
          <button
            type="button"
            className="notice-close"
            aria-label="Fermer"
            onClick={() => props.onDismiss(escalation.id)}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

/** Rien à afficher : on dit pourquoi et comment y remédier. */
function Placeholder({ connection }: { connection: Connection }) {
  if (connection.status === "unconfigured") {
    return (
      <div className="placeholder">
        <h1>Daemon non configuré</h1>
        <p>{connection.reason}</p>
        <p className="mono muted">http://localhost:1420/?daemon=ws%3A%2F%2F127.0.0.1%3A7878%2F%3Ftoken%3D…</p>
      </div>
    );
  }
  if (connection.status === "unauthorized") {
    return (
      <div className="placeholder">
        <h1>Jeton refusé</h1>
        <p>Le daemon écoute mais refuse le jeton. Vérifiez le fichier token de $ARGOS_HOME.</p>
      </div>
    );
  }
  return (
    <div className="placeholder">
      <p className="muted">
        {connection.status === "connected" ? "Lecture de l'arbre…" : "En attente du daemon…"}
      </p>
    </div>
  );
}
