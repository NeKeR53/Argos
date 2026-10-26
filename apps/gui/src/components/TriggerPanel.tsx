// Déclencheurs d'un agent (phase 5, D29–D31) : ceux qu'il a créés et ceux qui le visent.
// Actionner (surtout les interrupteurs), activer ou suspendre, supprimer après
// confirmation, copier l'URL d'un webhook. Relue à l'ouverture, après chaque action et
// quand un déclencheur part vers cet agent.

import { useEffect, useState } from "react";
import type { AgentId, AgentStatus, Command, Trigger, TriggerKind } from "../protocol";
import { maskedWebhookUrl, webhookUrl } from "../hooks";
import { ConfirmDialog } from "./ConfirmDialog";

const REFRESH_DELAY_MS = 250;

interface Props {
  agent: AgentStatus;
  triggers: Trigger[] | undefined;
  revision: number;
  /** Base des webhooks (`http://hôte:port`), inconnue tant qu'elle n'est pas lue. */
  hooksBase: string | null;
  connected: boolean;
  open: boolean;
  onToggle: (open: boolean) => void;
  send: (command: Command) => boolean;
  nameOf: (agent: AgentId) => string;
}

export const triggerKindLabel: Record<TriggerKind, string> = {
  every: "intervalle",
  cron: "cron",
  switch: "interrupteur",
  watch: "dossier",
  webhook: "webhook",
};

export function TriggerPanel(props: Props) {
  const { agent, triggers, revision, connected, open, onToggle, send } = props;
  const [deleting, setDeleting] = useState<Trigger | null>(null);

  useEffect(() => {
    if (!connected) return;
    const timer = setTimeout(() => send({ command: "triggers", agent: agent.id }), REFRESH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [agent.id, revision, connected, send]);

  const count = triggers?.length;
  return (
    <details className="triggers" open={open} onToggle={(event) => onToggle(event.currentTarget.open)}>
      <summary>
        <span className="board-title">Déclencheurs</span>
        <span className="muted">{count === undefined ? "lecture…" : count === 0 ? "aucun" : count}</span>
      </summary>
      {triggers && triggers.length > 0 && (
        <ul className="trigger-list">
          {triggers.map((trigger) => (
            <TriggerRow key={trigger.id} trigger={trigger} {...props} onDelete={setDeleting} />
          ))}
        </ul>
      )}
      {deleting && (
        <ConfirmDialog
          title={`Supprimer « ${deleting.name} » ?`}
          confirmLabel="Supprimer"
          danger
          onConfirm={() => {
            send({ command: "delete_trigger", trigger: deleting.id });
            setDeleting(null);
          }}
          onCancel={() => setDeleting(null)}
        >
          <p>Le déclencheur est supprimé pour de bon ; son agent ne sera plus stimulé par lui.</p>
        </ConfirmDialog>
      )}
    </details>
  );
}

function TriggerRow({
  trigger,
  agent,
  hooksBase,
  connected,
  send,
  nameOf,
  onDelete,
}: Props & { trigger: Trigger; onDelete: (trigger: Trigger) => void }) {
  const [copied, setCopied] = useState(false);
  const relation =
    trigger.target !== agent.id
      ? `vers ${nameOf(trigger.target)}`
      : trigger.owner !== agent.id
        ? `créé par ${nameOf(trigger.owner)}`
        : null;

  const copy = async () => {
    if (!hooksBase || !trigger.secret) return;
    try {
      await navigator.clipboard.writeText(webhookUrl(hooksBase, trigger.id, trigger.secret));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <li className="trigger" data-enabled={trigger.enabled}>
      <div className="trigger-head">
        <span className="trigger-name">{trigger.name}</span>
        <span className="trigger-kind">{triggerKindLabel[trigger.kind]}</span>
        <span className="flag" data-state={trigger.enabled ? "active" : undefined}>
          {trigger.enabled ? "actif" : "suspendu"}
        </span>
        {relation && <span className="muted">{relation}</span>}
        <span className="spacer" />
        <button
          type="button"
          className="button"
          disabled={!connected}
          onClick={() => send({ command: "fire_trigger", trigger: trigger.id })}
          title="Remet la consigne à l'agent maintenant"
        >
          Actionner
        </button>
        <button
          type="button"
          className="button"
          disabled={!connected}
          onClick={() => send({ command: "set_trigger", trigger: trigger.id, enabled: !trigger.enabled })}
        >
          {trigger.enabled ? "Suspendre" : "Activer"}
        </button>
        <button
          type="button"
          className="button button-danger-quiet"
          disabled={!connected}
          onClick={() => onDelete(trigger)}
        >
          Supprimer…
        </button>
      </div>
      <div className="trigger-detail">
        <TriggerSpec trigger={trigger} hooksBase={hooksBase} copied={copied} onCopy={copy} />
        <span className="muted">dernier : {formatFired(trigger.last_fired)}</span>
      </div>
      {trigger.prompt && <p className="trigger-prompt">{trigger.prompt}</p>}
    </li>
  );
}

function TriggerSpec(props: { trigger: Trigger; hooksBase: string | null; copied: boolean; onCopy: () => void }) {
  const { trigger, hooksBase, copied, onCopy } = props;
  switch (trigger.kind) {
    case "every":
      return <span>{formatInterval(trigger.spec)}</span>;
    case "cron":
    case "watch":
      return <span className="mono">{trigger.spec}</span>;
    case "switch":
      return <span className="muted">à la main</span>;
    case "webhook":
      if (!hooksBase || !trigger.secret) {
        return <span className="muted">URL inconnue (adresse du daemon ou secret absents)</span>;
      }
      return (
        <span className="trigger-url">
          <span className="mono">POST {maskedWebhookUrl(hooksBase, trigger.id)}</span>
          <button type="button" className="link" onClick={onCopy}>
            {copied ? "copiée" : "copier l'URL"}
          </button>
        </span>
      );
  }
}

/** « toutes les 15 min », d'après des secondes. */
export function formatInterval(spec: string): string {
  const seconds = Number(spec);
  if (!Number.isFinite(seconds) || seconds <= 0) return spec;
  if (seconds % 3600 === 0) return `toutes les ${seconds / 3600} h`;
  if (seconds % 60 === 0) return `toutes les ${seconds / 60} min`;
  return `toutes les ${seconds} s`;
}

const firedFormat = new Intl.DateTimeFormat("fr-FR", { dateStyle: "short", timeStyle: "short" });

export function formatFired(lastFired: number | null): string {
  return lastFired == null ? "jamais" : firedFormat.format(new Date(lastFired * 1000));
}
