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
} from "./state";
import { useDaemon } from "./useDaemon";
import { simulateVoice, useVoiceView } from "./useVoiceView";
import { voiceReason, voiceStatusLabel, type VoiceStatus } from "./voice";
import { NotificationCenter } from "./components/NotificationCenter";
import { AgentContextPanel } from "./components/AgentContextPanel";
import { AgentTree } from "./components/AgentTree";
import { ChatPanel } from "./components/ChatPanel";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { OrbSettings } from "./components/OrbSettings";
import { DEFAULT_SHORTCUTS, formatShortcut, loadShortcuts, matchesShortcut, type Shortcuts } from "./shortcuts";

const isMac = navigator.platform.toLowerCase().startsWith("mac");
/** Liste vide stable : évite de relancer les effets des surfaces à chaque rendu. */
const NO_SURFACES: SurfaceMeta[] = [];
/** Cadence de relecture de l'état des nœuds (pas d'événement du daemon pour ça). */
const NODES_REFRESH_MS = 15_000;
/** Essai de la voix sans micro (`voice_simulate`) : app Tauri de dev seulement. */
const VOICE_TRIAL = import.meta.env.DEV && isTauri();

export default function App() {
  const { state, dispatch, send, request } = useDaemon();
  useVoiceView(dispatch);
  const [hooksBase, setHooksBase] = useState<string | null>(null);
  useEffect(() => {
    void resolveHooksBase().then(setHooksBase);
  }, []);
  const [panicAsked, setPanicAsked] = useState(false);
  const [shortcuts, setShortcuts] = useState<Shortcuts>(DEFAULT_SHORTCUTS);
  const [editingShortcuts, setEditingShortcuts] = useState(false);
  const [editingOrb, setEditingOrb] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  useEffect(() => {
    void loadShortcuts().then(setShortcuts).catch(() => undefined);
  }, []);
  const panicLabel = formatShortcut(shortcuts.panic, isMac);
  const [retiring, setRetiring] = useState<AgentStatus | null>(null);
  const connected = state.connection.status === "connected";
  const agent = selectedAgent(state);
  const roots = treeOf(state);

  // Le daemon ne signale pas les changements d'état des nœuds : on les relit.
  useEffect(() => {
    if (!connected) return;
    send({ command: "nodes" });
    send({ command: "models" });
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
      if (!matchesShortcut(event, shortcuts.panic, isMac)) return;
      event.preventDefault();
      if (!connected) return;
      if (panicAsked) panic();
      else setPanicAsked(true);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [connected, panicAsked, panic, shortcuts.panic]);

  const select = (id: AgentId) => dispatch({ type: "select", agent: id });
  const isLiving = (id: AgentId) => id in state.agents;
  const assistant = assistantOf(state);
  // La marque d'escalade reste sur l'Assistant jusqu'à ce qu'on le consulte.
  const alerts: Record<AgentId, number> = assistant ? { [assistant.id]: state.unseenEscalations } : {};

  return (
    <div className="app">
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
          <div className="sidebar-actions">
            <NotificationCenter
              notices={state.notices}
              escalations={state.escalations}
              agents={state.agents}
              onOpenNotice={(id) => { setSettingsOpen(false); select(id); }}
              onOpenEscalation={(escalation) => {
                setSettingsOpen(false);
                const target = escalation.agent && isLiving(escalation.agent) ? escalation.agent : assistant?.id;
                if (target) select(target);
                dispatch({ type: "dismiss_escalation", escalation: escalation.id });
              }}
              onDismissNotice={(id) => dispatch({ type: "dismiss", notice: id })}
              onDismissEscalation={(id) => dispatch({ type: "dismiss_escalation", escalation: id })}
            />
            <button type="button" className="sidebar-action danger-action" disabled={!connected} onClick={() => setPanicAsked(true)}><svg className="sidebar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><rect x="5" y="5" width="14" height="14" rx="2" /></svg><span>Tout arrêter</span></button>
            <button type="button" className="sidebar-action" onClick={() => setSettingsOpen(true)}><svg className="sidebar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M10.4 3.4h3.2l.5 2a7 7 0 0 1 1.6.9l2-.6 1.6 2.7-1.5 1.4a7 7 0 0 1 0 1.9l1.5 1.4-1.6 2.7-2-.6a7 7 0 0 1-1.6.9l-.5 2h-3.2l-.5-2a7 7 0 0 1-1.6-.9l-2 .6-1.6-2.7 1.5-1.4a7 7 0 0 1 0-1.9L4.7 8.4l1.6-2.7 2 .6a7 7 0 0 1 1.6-.9z" transform="translate(0 1)"/><circle cx="12" cy="12" r="2.5" /></svg><span>Réglages</span></button>
          </div>
        </aside>

        <main className="main">
          {settingsOpen ? <section className="settings-page"><button className="button" onClick={() => setSettingsOpen(false)}>← Retour</button><h1>Réglages</h1><div className="settings-row"><span>Petit bonhomme</span><button className="button" onClick={() => setEditingOrb(true)}>Configurer</button></div><div className="settings-row"><span>Raccourcis</span><button className="button" onClick={() => setEditingShortcuts(true)}>Configurer</button></div><div className="settings-row"><span>Connexion</span><ConnectionBadge connection={state.connection} /></div>{state.voice.status && <div className="settings-row"><span>Voix</span><VoiceBadge status={state.voice.status} /></div>}<NodeDots nodes={state.nodes} />{VOICE_TRIAL && <VoiceTrial disabled={!connected} />}</section> : <>
          {agent ? (
            <div className="agent-workspace"><ChatPanel
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
              profiles={state.profiles}
              send={send}
              request={request}
              isLiving={isLiving}
              nameOf={(id) => (state.agents[id] ? displayName(state.agents[id]) : shortId(id))}
              onRetire={setRetiring}
              onSelect={select}
            />
            <AgentContextPanel key={agent.id} profiles={state.profiles} agent={agent} triggers={state.triggers[agent.id]} revision={state.triggerRevisions[agent.id] ?? 0} hooksBase={hooksBase} connected={connected} send={send} request={request} nameOf={(id) => state.agents[id] ? displayName(state.agents[id]) : shortId(id)} />
            </div>
          ) : (
            <Placeholder connection={state.connection} />
          )}
          </>}
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
          <p className="muted">Entrée ou {panicLabel} confirme, Échap annule.</p>
        </ConfirmDialog>
      )}

      {editingShortcuts && (
        <ShortcutsDialog
          shortcuts={shortcuts}
          mac={isMac}
          onChange={setShortcuts}
          onClose={() => setEditingShortcuts(false)}
        />
      )}

      {editingOrb && <OrbSettings onClose={() => setEditingOrb(false)} />}

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
