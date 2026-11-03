// Réglages de la console, ouverts depuis le ⚙ de la barre du haut : connexion, voix (état
// et essai sans micro), mises à jour, raccourcis et globe, canaux de discussion, nœuds et
// profils de modèles. Ce qui n'a pas à rester sous les yeux. Les canaux et les profils se lisent ici ;
// ils se règlent dans config.toml.

import { useEffect, useState, type ReactNode } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { AgentId, AgentStatus, ChannelCapabilities, ChannelStatus, NodeStatus, ProfileInfo } from "../protocol";
import type { Connection } from "../state";
import type { Updates } from "../updates";
import { simulateVoice } from "../useVoiceView";
import { NodeMap } from "./NodeMap";
import { voiceReason, voiceStatusLabel, type VoiceInfo, type VoiceStatus } from "../voice";

interface Props {
  connection: Connection;
  voice: VoiceStatus | null;
  nodes: NodeStatus[] | null;
  agents: Record<AgentId, AgentStatus>;
  channels: ChannelStatus[] | null;
  profiles: ProfileInfo[] | null;
  updates: Updates;
  connected: boolean;
  /** Essai de la voix sans micro : app Tauri de dev seulement. */
  voiceTrial: boolean;
  onShortcuts: () => void;
  /** Ouvre un agent depuis la carte des nœuds. */
  onSelectAgent: (agent: AgentId) => void;
  onOrb: () => void;
  onClose: () => void;
}

export function SettingsPanel(props: Props) {
  const { connection, voice, nodes, channels, profiles, connected } = props;
  const pending = <p className="settings-empty">{connected ? "Lecture…" : "Hors ligne."}</p>;
  return (
    <section className="settings-page" aria-label="Réglages">
      <header className="settings-head">
        <h1>Réglages</h1>
        <button type="button" className="button" onClick={props.onClose}>
          Fermer
        </button>
      </header>

      <SettingsGroup title="Général">
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
        {voice?.status === "ready" && isTauri() && (
          <div className="settings-row">
            <span>Écoute mains libres</span>
            <HandsFree />
          </div>
        )}
        {props.voiceTrial && (
          <div className="settings-row">
            <span>Simuler la voix</span>
            <VoiceTrial disabled={!connected} />
          </div>
        )}
      </SettingsGroup>

      <SettingsGroup title="Mises à jour">
        <UpdateRow updates={props.updates} connected={connected} />
      </SettingsGroup>

      <SettingsGroup title="Interface">
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
      </SettingsGroup>

      <SettingsGroup
        title="Canaux de discussion"
        hint={
          <>
            Les voies par lesquelles tu parles à l'Assistant hors de la console ; il choisit lui-même où il répond. Une section <code>[channels.&lt;id&gt;]</code> par canal dans config.toml.
          </>
        }
      >
        {channels === null ? (
          pending
        ) : channels.length === 0 ? (
          <p className="settings-empty">
            Aucun canal configuré. Pour Telegram : <code>[channels.telegram]</code> avec <code>type = "telegram"</code>, puis relancer le daemon.
          </p>
        ) : (
          <ul className="channel-list">
            {channels.map((channel) => (
              <ChannelCard key={channel.id} channel={channel} />
            ))}
          </ul>
        )}
      </SettingsGroup>

      <SettingsGroup
        title="Nœuds"
        hint={
          <>
            Les machines où l'Assistant peut placer des Agents, et ceux qui y tournent. Chaque nœud détient ses agents et tourne seul ; l'utilisateur les relie dans <code>[peers.&lt;nom&gt;]</code>.
          </>
        }
      >
        {nodes === null ? (
          pending
        ) : (
          <NodeMap agents={props.agents} nodes={nodes} onSelect={props.onSelectAgent} />
        )}
      </SettingsGroup>

      {profiles && profiles.length > 0 && (
        <SettingsGroup
          title="Profils de modèles"
          hint={
            <>
              Choisis par l'Assistant et les Agents selon le travail ; le premier modèle disponible de la chaîne sert. À régler dans <code>[models]</code>.
            </>
          }
        >
          <ul className="settings-list">
            {profiles.map((profile) => (
              <li key={profile.name} className="profile-row">
                <div className="profile-head">
                  <span className="profile-name">{profile.name}</span>
                  <span className="settings-detail">{profile.description}</span>
                </div>
                <ol className="profile-chain">
                  {profile.models.map((model) => (
                    <li key={`${model.engine}/${model.model}`}>
                      {model.model} <span className="muted">{model.engine}</span>
                    </li>
                  ))}
                </ol>
              </li>
            ))}
          </ul>
        </SettingsGroup>
      )}
    </section>
  );
}

function SettingsGroup({ title, hint, children }: { title: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <section className="settings-group" aria-label={title}>
      <h2>{title}</h2>
      {hint && <p className="settings-hint">{hint}</p>}
      {children}
    </section>
  );
}

const CAPABILITY_LABELS: [keyof ChannelCapabilities, string][] = [
  ["text", "Texte"],
  ["voice_in", "Vocal reçu"],
  ["voice_out", "Vocal envoyé"],
  ["buttons", "Boutons"],
  ["attachments", "Pièces jointes"],
];

const CHANNEL_TYPES: Record<string, string> = { telegram: "Telegram" };

/** Un canal configuré : son état, ce qu'il sait faire, ses envois de l'heure. */
function ChannelCard({ channel }: { channel: ChannelStatus }) {
  const state = !channel.active ? "failed" : channel.unavailable ? "waiting" : "ready";
  const label = { ready: "Actif", waiting: "En attente", failed: "Désactivé" }[state];
  const used = Math.min(1, channel.max_per_hour ? channel.sent_last_hour / channel.max_per_hour : 0);
  return (
    <li className="channel-card" data-state={state}>
      <div className="channel-head">
        <span className="channel-name">{channel.id}</span>
        <span className="channel-type">{CHANNEL_TYPES[channel.type] ?? channel.type}</span>
        <span className="channel-state">
          <span className="channel-dot" aria-hidden />
          {label}
        </span>
      </div>
      {channel.unavailable && <p className="channel-reason">{channel.unavailable}</p>}
      <ul className="channel-capabilities" aria-label="Capacités">
        {CAPABILITY_LABELS.map(([key, text]) => (
          <li key={key} data-on={channel.capabilities[key]} title={channel.capabilities[key] ? undefined : "Non pris en charge"}>
            {text}
          </li>
        ))}
      </ul>
      <dl className="channel-facts">
        <div>
          <dt>Escalades</dt>
          <dd>{channel.escalations ? "Annoncées ici" : "Non annoncées"}</dd>
        </div>
        <div>
          <dt>Envois de l'Assistant</dt>
          <dd>
            <span className="channel-meter" aria-hidden>
              <span style={{ width: `${used * 100}%` }} />
            </span>
            {channel.sent_last_hour} / {channel.max_per_hour} dans l'heure
          </dd>
        </div>
      </dl>
    </li>
  );
}

/** Version installée, version disponible, avancement d'une installation (D46). */
function UpdateRow({ updates, connected }: { updates: Updates; connected: boolean }) {
  const { view } = updates;
  const checkButton = (
    <button type="button" className="button" disabled={!connected || updates.checking} onClick={updates.check}>
      {updates.checking ? "Recherche…" : "Vérifier"}
    </button>
  );
  switch (view.kind) {
    case "unknown":
      return <p className="settings-empty">{connected ? "Lecture…" : "Hors ligne."}</p>;
    case "up_to_date":
      return (
        <div className="settings-row">
          <span>Argos {view.version} est à jour</span>
          {checkButton}
        </div>
      );
    case "error":
      return (
        <div className="settings-row">
          <span className="settings-detail">Recherche impossible : {view.message}</span>
          {checkButton}
        </div>
      );
    case "available":
      return (
        <div className="update-card">
          <div className="settings-row">
            <span>
              Argos {view.version} est disponible{" "}
              <a href={view.url} target="_blank" rel="noreferrer">
                notes de version
              </a>
            </span>
            <button type="button" className="button button-primary" disabled={!connected} onClick={updates.apply}>
              Installer
            </button>
          </div>
          {view.notes.trim() && <pre className="update-notes">{view.notes.trim()}</pre>}
          <p className="settings-hint">
            Remplace l'app et le daemon, migre config.toml (l'ancien est gardé), puis relance le daemon : les sessions en cours sont mises au repos et reprendront. Sur macOS, la permission du micro peut être à redonner après la mise à jour.
          </p>
        </div>
      );
    case "installing":
      return (
        <div className="settings-row" role="status">
          <span>{view.version ? `Installation d'Argos ${view.version}…` : "Mise à jour…"}</span>
          <span className="settings-detail">{connected ? view.message : "redémarrage du daemon…"}</span>
        </div>
      );
    case "failed":
      return (
        <div className="settings-row">
          <span className="settings-detail">Échec de la mise à jour vers {view.version} : {view.message}</span>
          {checkButton}
        </div>
      );
    case "relaunch":
      return (
        <div className="settings-row">
          <span>
            Argos {view.version} est installé ; cette fenêtre est encore en {view.app}.
          </span>
          <button type="button" className="button button-primary" onClick={updates.relaunch}>
            Relancer l'app
          </button>
        </div>
      );
  }
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

/**
 * Écoute continue : le micro reste ouvert, et une phrase qui commence par le mot de
 * déclenchement (« Argos, … ») part à l'Assistant en coupant sa lecture.
 */
function HandsFree() {
  const [on, setOn] = useState<boolean | null>(null);
  const [wake, setWake] = useState("");
  useEffect(() => {
    void invoke<VoiceInfo>("voice_status").then((info) => {
      setOn(info.hands_free ?? false);
      setWake(info.wake_word ?? "");
    });
  }, []);
  const active = wake ? `Active : dites « ${wake}, … »` : "Active : toute voix est une demande (casque conseillé)";
  return (
    <label className="hands-free" title="Mot de déclenchement : [voice] wake_word dans config.toml">
      <input
        type="checkbox"
        checked={on ?? false}
        disabled={on === null}
        onChange={(event) => {
          const wanted = event.target.checked;
          setOn(null);
          void invoke<boolean>("voice_set_hands_free", { on: wanted }).then(setOn, () => setOn(!wanted));
        }}
      />
      {on ? active : "Coupée (push-to-talk)"}
    </label>
  );
}
