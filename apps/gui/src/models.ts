// Choix du modèle d'un agent, tel que l'utilisateur le voit : une liste de modèles aux noms
// lisibles (« Claude Opus 5.5 »), sans profils ni descriptions. La liste vient des profils
// des harnais (réponse `models`), dédoublonnée sur la paire (moteur, modèle).

import type { EngineKind, ProfileInfo } from "./protocol";

/** Gamme indicative d'un modèle, affichée en gris à côté de son nom. */
export type ModelTier = "économique" | "équilibré" | "puissant";

export interface ModelChoice {
  engine: EngineKind;
  model: string;
  label: string;
  tier: ModelTier | null;
}

/** Trait d'union insécable : « GPT‑6 » ne se coupe pas en fin de ligne. */
const NB_HYPHEN = "‑";

/**
 * Nom lisible d'un identifiant de modèle : `claude-opus-5-5` → « Claude Opus 5.5 »,
 * `gpt-6.1-sol` → « GPT‑6.1 Sol ». Un identifiant inconnu reste tel quel.
 */
export function modelLabel(model: string): string {
  const claude = /^claude-([a-z]+)-(\d+)(?:-(\d+))?$/i.exec(model);
  if (claude) {
    const version = claude[3] ? `${claude[2]}.${claude[3]}` : claude[2];
    return `Claude ${capitalize(claude[1])} ${version}`;
  }
  const gpt = /^gpt-([\d.]+)(?:-([a-z]+))?$/i.exec(model);
  if (gpt) return `GPT${NB_HYPHEN}${gpt[1]}${gpt[2] ? ` ${capitalize(gpt[2])}` : ""}`;
  return model;
}

/** Gamme d'un modèle d'après sa famille ; `null` si on ne sait pas. */
export function modelTier(model: string): ModelTier | null {
  const name = model.toLowerCase();
  if (/haiku|luna|mini|nano/.test(name)) return "économique";
  if (/sonnet/.test(name)) return "équilibré";
  if (/opus|sol/.test(name)) return "puissant";
  return null;
}

const TIER_ORDER: (ModelTier | null)[] = ["puissant", "équilibré", "économique", null];
const ENGINE_ORDER: EngineKind[] = ["claude", "codex"];

/**
 * Les modèles proposés, une fois chacun : Claude puis GPT, du plus puissant au plus
 * économique. Le modèle actuel de l'agent est ajouté s'il n'y figure pas, pour qu'on le
 * voie coché.
 */
export function modelChoices(profiles: ProfileInfo[], current?: { engine: EngineKind; model: string | null }): ModelChoice[] {
  const seen = new Set<string>();
  const choices: ModelChoice[] = [];
  const add = (engine: EngineKind, model: string) => {
    const key = `${engine}|${model}`;
    if (seen.has(key)) return;
    seen.add(key);
    choices.push({ engine, model, label: modelLabel(model), tier: modelTier(model) });
  };
  for (const profile of profiles) for (const choice of profile.models) add(choice.engine, choice.model);
  if (current?.model) add(current.engine, current.model);
  // Tri stable : à rang égal, l'ordre des profils est gardé.
  return choices.sort(
    (a, b) =>
      ENGINE_ORDER.indexOf(a.engine) - ENGINE_ORDER.indexOf(b.engine) ||
      TIER_ORDER.indexOf(a.tier) - TIER_ORDER.indexOf(b.tier),
  );
}

function capitalize(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1).toLowerCase();
}
