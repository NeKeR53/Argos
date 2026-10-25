// Console Argos (phase 1) : arbre des agents, chat avec l'agent sélectionné, panic stop.
// Une seule fenêtre ; l'orbe, l'overlay et les fenêtres flottantes viennent en phase 3.

import { useCallback, useEffect, useState } from "react";
import "./App.css";
import type { AgentStatus } from "./protocol";
import { displayName, selectedAgent, treeOf, type Connection, type Notice } from "./state";
import { useDaemon } from "./useDaemon";
import { AgentTree } from "./components/AgentTree";
import { ChatPanel } from "./components/ChatPanel";
import { ConfirmDialog } from "./components/ConfirmDialog";

/** Raccourci du panic stop dans l'app : ⌘⇧X (Ctrl+Maj+X hors macOS). */
function isPanicShortcut(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "x";
}

const isMac = navigator.platform.toLowerCase().startsWith("mac");
const PANIC_SHORTCUT = isMac ? "⌘⇧X" : "Ctrl+Maj+X";

export default function App() {
  const { state, dispatch, send } = useDaemon();
  const [panicAsked, setPanicAsked] = useState(false);
  const [retiring, setRetiring] = useState<AgentStatus | null>(null);
  const connected = state.connection.status === "connected";
  const agent = selectedAgent(state);
  const roots = treeOf(state);

  const panic = useCallback(() => {
    setPanicAsked(false);
    send({ command: "panic_stop" });
  }, [send]);

  // Raccourci : un premier appui demande confirmation, un second confirme.
  useEffect(() => {
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

  const select = (agent: AgentStatus) => dispatch({ type: "select", agent: agent.id });

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">Argos</span>
        <ConnectionBadge connection={state.connection} />
        <span className="spacer" />
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
            <AgentTree roots={roots} selected={agent?.id ?? null} onSelect={(id) => dispatch({ type: "select", agent: id })} />
          ) : (
            <p className="sidebar-empty muted">{connected ? "Arbre vide." : "En attente du daemon…"}</p>
          )}
        </aside>

        <main className="main">
          <Notices notices={state.notices} onDismiss={(id) => dispatch({ type: "dismiss", notice: id })} />
          {agent ? (
            <ChatPanel
              agent={agent}
              parent={agent.parent ? (state.agents[agent.parent] ?? null) : null}
              thread={state.threads[agent.id] ?? []}
              spent={state.spent[agent.id]}
              connected={connected}
              send={send}
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

function Notices({ notices, onDismiss }: { notices: Notice[]; onDismiss: (id: number) => void }) {
  if (notices.length === 0) return null;
  return (
    <div className="notices" role="alert">
      {notices.map((notice) => (
        <div key={notice.id} className="notice" data-tone={notice.tone}>
          <span>{notice.text}</span>
          <button type="button" className="notice-close" aria-label="Fermer" onClick={() => onDismiss(notice.id)}>
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
