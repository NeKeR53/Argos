// Console Argos (phase 1) : arbre des agents, chat avec l'agent sélectionné, panic stop.
// Une seule fenêtre ; l'orbe, l'overlay et les fenêtres flottantes viennent en phase 3.

import { useCallback, useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { resolveHooksBase } from "./hooks";
import "./App.css";
import type { AgentId, AgentStatus, SurfaceMeta } from "./protocol";
import {
  attention,
  displayName,
  selectedAgent,
  shortId,
  treeOf,
  type Connection,
} from "./state";
import { useDaemon } from "./useDaemon";
import { useVoiceView } from "./useVoiceView";
import { voiceReason } from "./voice";
import { NotificationCenter } from "./components/NotificationCenter";
import { AgentContextPanel } from "./components/AgentContextPanel";
import { AgentTree } from "./components/AgentTree";
import { ChatPanel } from "./components/ChatPanel";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { OrbSettings } from "./components/OrbSettings";
import { ConnectionStatus, SettingsPanel } from "./components/SettingsPanel";
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
  // Réglages de l'agent affiché (bouton « ⋯ » de sa conversation).
  const [panelOpen, setPanelOpen] = useState(false);
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

  const select = (id: AgentId) => {
    setSettingsOpen(false);
    dispatch({ type: "select", agent: id });
  };
  const isLiving = (id: AgentId) => id in state.agents;
  const nameOf = (id: AgentId) => (state.agents[id] ? displayName(state.agents[id]) : shortId(id));
  // Pastilles d'escalade de l'arbre : l'Agent concerné, et l'Assistant qui l'a relayée.
  const alerts = attention(state);
  const voiceStatus = state.voice.status;

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">Argos</span>
        <span className="spacer" />
        <ConnectionStatus connection={state.connection} compact />
        {voiceStatus?.status === "failed" && (
          <span className="voice-badge" data-status="failed" title={voiceReason(voiceStatus) ?? undefined}>
            Voix en panne
          </span>
        )}
        <button
          type="button"
          className="icon-button"
          aria-label="Réglages"
          aria-pressed={settingsOpen}
          title="Réglages"
          onClick={() => setSettingsOpen((open) => !open)}
        >
          <svg className="topbar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M10.4 3.4h3.2l.5 2a7 7 0 0 1 1.6.9l2-.6 1.6 2.7-1.5 1.4a7 7 0 0 1 0 1.9l1.5 1.4-1.6 2.7-2-.6a7 7 0 0 1-1.6.9l-.5 2h-3.2l-.5-2a7 7 0 0 1-1.6-.9l-2 .6-1.6-2.7 1.5-1.4a7 7 0 0 1 0-1.9L4.7 8.4l1.6-2.7 2 .6a7 7 0 0 1 1.6-.9z" transform="translate(0 1)"/><circle cx="12" cy="12" r="2.5" /></svg>
        </button>
        <button
          type="button"
          className="panic"
          disabled={!connected}
          onClick={() => setPanicAsked(true)}
          title={`Arrêt dur de tout l'arbre (${panicLabel})`}
        >
          Panic stop
        </button>
      </header>
      <NotificationCenter
        notices={state.notices}
        agents={state.agents}
        onOpenNotice={select}
        onDismissNotice={(id) => dispatch({ type: "dismiss", notice: id })}
      />
      <div className="body">
        <aside className="sidebar" aria-label="Agents">
          {roots.length > 0 ? (
            <AgentTree roots={roots} selected={agent?.id ?? null} alerts={alerts} onSelect={select} />
          ) : (
            <p className="sidebar-empty muted">{connected ? "Arbre vide." : "En attente du daemon…"}</p>
          )}
        </aside>

        <main className="main">
          {settingsOpen ? (
            <SettingsPanel
              connection={state.connection}
              voice={voiceStatus}
              nodes={state.nodes}
              connected={connected}
              voiceTrial={VOICE_TRIAL}
              onShortcuts={() => setEditingShortcuts(true)}
              onOrb={() => setEditingOrb(true)}
              onClose={() => setSettingsOpen(false)}
            />
          ) : agent ? (
            <div className="agent-workspace">
              <ChatPanel
                agent={agent}
                parent={agent.parent ? (state.agents[agent.parent] ?? null) : null}
                thread={state.threads[agent.id] ?? []}
                surfaces={state.surfaces[agent.id] ?? NO_SURFACES}
                surfaceDocs={state.surfaceDocs}
                surfaceFeeds={state.surfaceFeeds}
                surfaceRevision={state.surfaceRevisions[agent.id] ?? 0}
                dictation={state.voice.dictation}
                connected={connected}
                send={send}
                request={request}
                nameOf={nameOf}
                onSelect={select}
                panelOpen={panelOpen}
                onTogglePanel={() => setPanelOpen((open) => !open)}
              />
              {panelOpen && (
                <AgentContextPanel
                  key={agent.id}
                  agent={agent}
                  parent={agent.parent ? (state.agents[agent.parent] ?? null) : null}
                  spent={state.spent[agent.id]}
                  board={state.boards[agent.id]}
                  boardRevision={state.boardRevisions[agent.id] ?? 0}
                  profiles={state.profiles}
                  triggers={state.triggers[agent.id]}
                  revision={state.triggerRevisions[agent.id] ?? 0}
                  hooksBase={hooksBase}
                  connected={connected}
                  send={send}
                  request={request}
                  nameOf={nameOf}
                  isLiving={isLiving}
                  onSelect={select}
                  onRetire={setRetiring}
                  onClose={() => setPanelOpen(false)}
                />
              )}
            </div>
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
