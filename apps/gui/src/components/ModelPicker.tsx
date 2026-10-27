// Choix du modèle d'un agent par l'utilisateur (override, D21) : les modèles des profils
// de la politique, groupés par profil. Le changement parke la session ; l'agent reprend
// sur le nouveau modèle au prochain message.
//
// Menu dessiné dans la page plutôt qu'un `<select>` : la liste native de macOS ne s'ouvre
// pas toujours dans la webview d'une app sans icône dans le Dock.

import { useEffect, useRef, useState } from "react";
import type { AgentStatus, Command, EngineKind, ProfileInfo } from "../protocol";

interface Props {
  agent: AgentStatus;
  profiles: ProfileInfo[] | null;
  connected: boolean;
  send: (command: Command) => boolean;
}

const SEPARATOR = "|";

export function modelValue(engine: EngineKind, model: string): string {
  return `${engine}${SEPARATOR}${model}`;
}

export function parseModelValue(value: string): { engine: EngineKind; model: string } | null {
  const at = value.indexOf(SEPARATOR);
  if (at < 0) return null;
  const engine = value.slice(0, at);
  const model = value.slice(at + 1);
  if ((engine !== "claude" && engine !== "codex") || !model) return null;
  return { engine, model };
}

export function ModelPicker({ agent, profiles, connected, send }: Props) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const current = agent.model ? modelValue(agent.engine, agent.model) : "";

  // Profils pas encore reçus (connexion récente) : redemandés à l'ouverture.
  useEffect(() => {
    if (open && !profiles && connected) send({ command: "models" });
  }, [open, profiles, connected, send]);

  // Un clic ailleurs ou Échap referme le menu.
  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const choose = (engine: EngineKind, model: string) => {
    setOpen(false);
    if (modelValue(engine, model) === current) return;
    send({ command: "set_model", agent: agent.id, engine, model });
  };

  return (
    <div className="model-picker" ref={root}>
      <button
        type="button"
        className="model-picker-button"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={!connected}
        onClick={() => setOpen(!open)}
        title="Changer le modèle : la session est parkée, l'agent reprend sur ce modèle au prochain message"
      >
        {agent.engine} · {agent.model ?? "modèle par défaut"}
        {agent.profile && <span className="muted"> ({agent.profile})</span>} ▾
      </button>
      {open && (
        <div className="model-picker-menu" role="listbox" aria-label="Modèle de l'agent">
          {!profiles && <p className="muted model-picker-empty">Chargement des modèles…</p>}
          {(profiles ?? []).map((profile) => (
            <div key={profile.name} role="group" aria-label={profile.name} className="model-picker-group">
              <div className="model-picker-profile">
                <strong>{profile.name}</strong> <span className="muted">{profile.description}</span>
              </div>
              {profile.models.map((choice) => {
                const value = modelValue(choice.engine, choice.model);
                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={value === current}
                    key={`${profile.name}-${value}`}
                    className="model-picker-option"
                    onClick={() => choose(choice.engine, choice.model)}
                  >
                    {value === current ? "✓ " : ""}
                    {choice.engine} · {choice.model}
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
