// Réglages de la console, ouverts depuis le ⚙ de la barre du haut : connexion, voix (état
// et essai sans micro), raccourcis, orbe et nœuds. Ce qui n'a pas à rester sous les yeux.

import { useState } from "react";
import type { NodeStatus } from "../protocol";
import type { Connection } from "../state";
import { simulateVoice } from "../useVoiceView";
import { voiceReason, voiceStatusLabel, type VoiceStatus } from "../voice";

interface Props {
  connection: Connection;
  voice: VoiceStatus | null;
  nodes: NodeStatus[] | null;
  connected: boolean;
  /** Essai de la voix sans micro : app Tauri de dev seulement. */
  voiceTrial: boolean;
  onShortcuts: () => void;
  onOrb: () => void;
  onClose: () => void;
}

export function SettingsPanel(props: Props) {
  const { connection, voice, nodes, connected } = props;
  return (
    <section className="settings-page" aria-label="Réglages">
      <header className="settings-head">
        <h1>Réglages</h1>
        <button type="button" className="button" onClick={props.onClose}>
          Fermer
        </button>
      </header>
      <div className="settings-row">
        <span>Connexion</span>
        <ConnectionStatus connection={connection} />
      </div>
      {voice && (
        <div className="settings-row">
          <span>Voix</span>
          <span className="voice-badge" data-status={voice.status} title={voiceReason(voice) ?? undefined}>
            {voiceStatusLabel(voice)}
          </span>
        </div>
      )}
      {props.voiceTrial && (
        <div className="settings-row">
          <span>Simuler la voix</span>
          <VoiceTrial disabled={!connected} />
        </div>
      )}
      <div className="settings-row">
        <span>Raccourcis</span>
        <button type="button" className="button" onClick={props.onShortcuts}>
          Configurer
        </button>
      </div>
      <div className="settings-row">
        <span>Globe</span>
        <button type="button" className="button" onClick={props.onOrb}>
          Configurer
        </button>
      </div>
      {nodes && nodes.length > 0 && (
        <div className="settings-row">
          <span>Nœuds</span>
          <NodeDots nodes={nodes} />
        </div>
      )}
    </section>
  );
}

/** Une pastille par nœud déclaré (D32). */
function NodeDots({ nodes }: { nodes: NodeStatus[] }) {
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
        Simuler
      </button>
    </form>
  );
}

/**
 * État de la connexion. Complet dans les réglages (raison, prochain essai) ; `compact`
 * pour la barre du haut : une pastille, et du texte seulement quand elle manque.
 */
export function ConnectionStatus({ connection, compact = false }: { connection: Connection; compact?: boolean }) {
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
  const quiet = compact && connection.status === "connected";
  return (
    <span className="connection" data-status={connection.status} role="status" title={compact ? [label, detail].filter(Boolean).join(" · ") : undefined}>
      <span className="connection-dot" aria-hidden />
      {quiet ? <span className="sr-only">{label}</span> : <span className="connection-label">{label}</span>}
      {!compact && detail && <span className="connection-detail">{detail}</span>}
    </span>
  );
}
