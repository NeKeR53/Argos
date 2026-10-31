// Synchro de workspace (D44) d'un Agent placé sur un nœud : sa copie locale sur ce Mac,
// tenue à jour dans les deux sens, ou le formulaire pour l'ouvrir.

import { useState, type FormEvent } from "react";
import type { AgentStatus, Command, Reply, SyncState, SyncStatus } from "../protocol";

const stateLabel: Record<SyncState, string> = {
  idle: "à jour",
  syncing: "en cours",
  offline: "nœud injoignable",
  error: "en erreur",
};

export function syncSummary(syncs: SyncStatus[]): string {
  const [sync] = syncs;
  if (!sync) return "aucune";
  return `${stateLabel[sync.state]} · ${sync.files} fichier${sync.files > 1 ? "s" : ""}`;
}

export function SyncSection({ agent, syncs, connected, request }: {
  agent: AgentStatus;
  syncs: SyncStatus[];
  connected: boolean;
  request: (command: Command) => Promise<Reply>;
}) {
  const [open, setOpen] = useState(false);
  const [local, setLocal] = useState("");
  const [excludes, setExcludes] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (command: Command, then?: () => void) => {
    setBusy(true);
    setError(null);
    try {
      const reply = await request(command);
      if (reply.kind === "error") setError(reply.message);
      else then?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!connected || busy || !local.trim()) return;
    const patterns = excludes.split(",").map((pattern) => pattern.trim()).filter(Boolean);
    void run({ command: "open_sync", agent: agent.id, local: local.trim(), excludes: patterns }, () => setOpen(false));
  };

  return (
    <div className="sync-section">
      {syncs.map((sync) => (
        <div key={sync.id} className="sync-status" aria-label={`Synchro ${sync.id}`}>
          <p>
            <strong>{stateLabel[sync.state]}</strong> · {sync.files} fichier{sync.files > 1 ? "s" : ""} suivis
          </p>
          <p className="muted" title={sync.local}>Copie locale : {sync.local}</p>
          {sync.error && <p role="alert">{sync.error}</p>}
          {sync.conflicts.length > 0 && (
            <p>Conflits, version du hub mise de côté : {sync.conflicts.join(", ")}</p>
          )}
          {sync.held.length > 0 && <p className="muted">Trop gros pour voyager : {sync.held.join(", ")}</p>}
          <div className="workspace-actions">
            <button
              type="button"
              className="button"
              disabled={!connected || busy}
              onClick={() => void run({ command: "close_sync", sync: sync.id })}
            >
              {busy ? "Fermeture…" : "Fermer la synchro"}
            </button>
          </div>
        </div>
      ))}
      {syncs.length === 0 && (
        <button
          type="button"
          className="link"
          disabled={!connected || busy}
          aria-expanded={open}
          onClick={() => { setError(null); setOpen(!open); }}
        >
          Synchroniser sur ce Mac…
        </button>
      )}
      {open && syncs.length === 0 && (
        <form onSubmit={submit} className="workspace-form" aria-label="Synchroniser le workspace">
          <label htmlFor="sync-local">Copie locale (dossier vide ou absent)</label>
          <input
            id="sync-local"
            value={local}
            onChange={(event) => setLocal(event.target.value)}
            placeholder="/Users/moi/Projets/copie"
            autoFocus
            disabled={busy}
            required
          />
          <label htmlFor="sync-excludes">Exclusions en plus du .gitignore (facultatif)</label>
          <input
            id="sync-excludes"
            value={excludes}
            onChange={(event) => setExcludes(event.target.value)}
            placeholder="dist/, *.sqlite"
            disabled={busy}
          />
          <p className="muted">
            Le workspace de {agent.node} est copié ici, puis tenu à jour dans les deux sens. Ni .git, ni les fichiers
            ignorés ne voyagent. En cas de conflit, la version du nœud garde le chemin et celle du Mac est conservée à côté.
          </p>
          {error && <p role="alert">{error}</p>}
          <div className="workspace-actions">
            <button type="submit" className="button button-primary" disabled={!connected || busy || !local.trim()}>
              {busy ? "Ouverture…" : "Synchroniser"}
            </button>
            <button type="button" className="button" disabled={busy} onClick={() => setOpen(false)}>Annuler</button>
          </div>
        </form>
      )}
      {!open && error && <p role="alert">{error}</p>}
    </div>
  );
}
