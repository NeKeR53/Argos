import { useState, type FormEvent } from "react";
import type { AgentStatus, Command, Reply } from "../protocol";

export function WorkspacePicker({ agent, connected, request }: {
  agent: AgentStatus;
  connected: boolean;
  request: (command: Command) => Promise<Reply>;
}) {
  const [open, setOpen] = useState(false);
  const [path, setPath] = useState(agent.workdir);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!connected || saving || !path.trim()) return;
    setSaving(true);
    setError(null);
    try {
      const reply = await request({ command: "set_workspace", agent: agent.id, workdir: path.trim() });
      if (reply.kind === "error") setError(reply.message);
      else setOpen(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="workspace-picker">
      <button
        type="button"
        className="link"
        disabled={!connected || saving}
        aria-expanded={open}
        onClick={() => { setPath(agent.workdir); setError(null); setOpen(!open); }}
        title={agent.workdir}
      >
        Workspace : {agent.workdir} · Modifier…
      </button>
      {open && (
        <form onSubmit={submit} className="workspace-form" aria-label="Rattacher un workspace">
          <label htmlFor="workspace-path">Dossier sur {agent.node ?? "le hub"}</label>
          <input
            id="workspace-path"
            value={path}
            onChange={(event) => setPath(event.target.value)}
            placeholder="/chemin/du/projet"
            autoFocus
            disabled={saving}
            required
          />
          <p className="muted">L’Agent et ses prochains Workers héritent du AGENTS.md de ce dossier. Une nouvelle session repart de sa mémoire. Termine la tâche en cours et retire les Workers avant de changer.</p>
          {error && <p role="alert">{error}</p>}
          <div className="workspace-actions">
            <button type="submit" className="button button-primary" disabled={!connected || saving || agent.busy || !path.trim()}>
              {saving ? "Rattachement…" : "Rattacher"}
            </button>
            <button type="button" className="button" disabled={saving} onClick={() => setOpen(false)}>Annuler</button>
          </div>
        </form>
      )}
    </div>
  );
}
