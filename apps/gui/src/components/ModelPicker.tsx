// Choix du modèle d'un agent par l'utilisateur (override, D21) : les modèles des profils
// de la politique, groupés par profil. Le changement parke la session ; l'agent reprend
// sur le nouveau modèle au prochain message.

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
  const current = agent.model ? modelValue(agent.engine, agent.model) : "";
  const listed = new Set((profiles ?? []).flatMap((profile) => profile.models.map((m) => modelValue(m.engine, m.model))));
  return (
    <select
      className="model-picker"
      aria-label="Modèle de l'agent"
      title="Changer le modèle : la session est parkée, l'agent reprend sur ce modèle au prochain message"
      value={current}
      disabled={!connected || !profiles}
      onChange={(event) => {
        const choice = parseModelValue(event.target.value);
        if (choice && event.target.value !== current) {
          send({ command: "set_model", agent: agent.id, engine: choice.engine, model: choice.model });
        }
      }}
    >
      {!agent.model && <option value="">{agent.engine} · modèle par défaut</option>}
      {agent.model && !listed.has(current) && (
        <option value={current}>
          {agent.engine} · {agent.model}
        </option>
      )}
      {(profiles ?? []).map((profile) => (
        <optgroup key={profile.name} label={`${profile.name} — ${profile.description}`}>
          {profile.models.map((choice) => (
            <option key={`${profile.name}-${choice.engine}-${choice.model}`} value={modelValue(choice.engine, choice.model)}>
              {choice.engine} · {choice.model}
            </option>
          ))}
        </optgroup>
      ))}
    </select>
  );
}
