// Choix du modèle d'un agent par l'utilisateur (override, D21) : une liste de modèles aux
// noms lisibles, sans profils (voir `models.ts`). Le changement parke la session ; l'agent
// reprend sur le nouveau modèle au prochain message.
//
// Liste dessinée dans la page plutôt qu'un `<select>` : la liste native de macOS ne
// s'ouvre pas toujours dans la webview d'une app sans icône dans le Dock.

import { useEffect, useState } from "react";
import type { AgentStatus, Command, EngineKind, ProfileInfo } from "../protocol";
import { modelChoices } from "../models";

interface Props {
  agent: AgentStatus;
  profiles: ProfileInfo[] | null;
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

export function ModelPicker({ agent, profiles, connected, send, onChoose }: Props) {
  const [customEngine, setCustomEngine] = useState<EngineKind>("codex");
  const [customModel, setCustomModel] = useState("");
  const current = agent.model ? modelValue(agent.engine, agent.model) : "";

  // Profils pas encore reçus (connexion récente) : redemandés à l'ouverture.
  useEffect(() => {
    if (!profiles && connected) send({ command: "models" });
  }, [profiles, connected, send]);

  const choose = (engine: EngineKind, model: string) => {
    onChoose?.();
    if (modelValue(engine, model) === current) return;
    send({ command: "set_model", agent: agent.id, engine, model });
  };

  return (
    <div className="model-picker">
      <div className="model-picker-menu" role="group" aria-label="Modèle de l'agent">
        {!profiles && <p className="muted model-picker-empty">Chargement des modèles…</p>}
        {modelChoices(profiles ?? [], agent).map((choice) => {
          const selected = modelValue(choice.engine, choice.model) === current;
          return (
            <button
              key={modelValue(choice.engine, choice.model)}
              type="button"
              aria-pressed={selected}
              className="model-picker-option"
              disabled={!connected}
              onClick={() => choose(choice.engine, choice.model)}
            >
              <span>
                {choice.label}
                {choice.tier && <small className="model-tier">{choice.tier}</small>}
              </span>
              {selected && <span aria-hidden="true">✓</span>}
            </button>
          );
        })}
        <details className="model-custom">
          <summary>Autre modèle…</summary>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              const model = customModel.trim();
              if (model) {
                choose(customEngine, model);
                setCustomModel("");
              }
            }}
          >
            <select aria-label="Moteur" value={customEngine} onChange={(event) => setCustomEngine(event.target.value as EngineKind)}>
              <option value="codex">GPT</option>
              <option value="claude">Claude</option>
            </select>
            <input
              aria-label="Identifiant du modèle"
              value={customModel}
              onChange={(event) => setCustomModel(event.target.value)}
              placeholder="Identifiant du modèle"
            />
            <button type="submit" disabled={!connected || !customModel.trim()}>OK</button>
          </form>
        </details>
      </div>
    </div>
  );
}
