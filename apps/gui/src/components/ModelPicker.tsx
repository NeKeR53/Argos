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
  buttonLabel?: string;
  inline?: boolean;
  onChoose?: () => void;
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

export function ModelPicker({ agent, profiles, connected, send, buttonLabel, inline = false, onChoose }: Props) {
  const [open, setOpen] = useState(false);
  const [customEngine, setCustomEngine] = useState<EngineKind>("codex");
  const [customModel, setCustomModel] = useState("");
  const root = useRef<HTMLDivElement>(null);
  const current = agent.model ? modelValue(agent.engine, agent.model) : "";

  // Profils pas encore reçus (connexion récente) : redemandés à l'ouverture.
  useEffect(() => {
    if ((open || inline) && !profiles && connected) send({ command: "models" });
  }, [open, inline, profiles, connected, send]);

  // Un clic ailleurs ou Échap referme le menu.
  useEffect(() => {
    if (!open || inline) return;
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
  }, [open, inline]);

  const choose = (engine: EngineKind, model: string) => {
    setOpen(false);
    onChoose?.();
    if (modelValue(engine, model) === current) return;
    send({ command: "set_model", agent: agent.id, engine, model });
  };

  return (
    <div className="model-picker" ref={root}>
      {!inline && <button
        type="button"
        className="model-picker-button"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={!connected}
        onClick={() => setOpen(!open)}
        aria-label="Choisir le modèle"
        title="Changer le modèle : la session est parkée, l'agent reprend sur ce modèle au prochain message"
      >
        {buttonLabel ?? agent.model ?? "Modèle par défaut"} <span aria-hidden="true">⌄</span>
      </button>}
      {(open || inline) && (
        <div className="model-picker-menu" role="group" aria-label="Modèle de l'agent">
          {!profiles && <p className="muted model-picker-empty">Chargement des modèles…</p>}
          {(["codex", "claude"] as const).map((engine) => {
            const configured = (profiles ?? []).flatMap((profile) => profile.models).filter((model) => model.engine === engine);
            const defaults = engine === "codex" ? ["gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] : [];
            const names = [...new Set([...configured.map((model) => model.model), ...defaults, ...(agent.engine === engine && agent.model ? [agent.model] : [])])];
            return <div key={engine} role="group" aria-label={engine === "codex" ? "Codex" : "Claude"} className="model-picker-group">
              <div className="model-picker-profile">{engine === "codex" ? "Codex" : "Claude"}</div>
              {names.map((model) => <button key={model} type="button" aria-pressed={modelValue(engine, model) === current} className="model-picker-option" onClick={() => choose(engine, model)}><span>{model}</span>{modelValue(engine, model) === current && <span aria-hidden="true">✓</span>}</button>)}
            </div>;
          })}
          <form className="model-custom" onSubmit={(event) => { event.preventDefault(); const model = customModel.trim(); if (model) { choose(customEngine, model); setCustomModel(""); } }}>
            <label htmlFor="custom-model">Autre modèle</label><div><select aria-label="Moteur" value={customEngine} onChange={(event) => setCustomEngine(event.target.value as EngineKind)}><option value="codex">Codex</option><option value="claude">Claude</option></select><input id="custom-model" value={customModel} onChange={(event) => setCustomModel(event.target.value)} placeholder="Identifiant du modèle" /><button type="submit" disabled={!customModel.trim()}>OK</button></div>
          </form>
        </div>
      )}
    </div>
  );
}
