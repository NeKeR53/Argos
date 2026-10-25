// Mise en forme des valeurs affichées (coûts, tokens, états).

import type { AgentStatus, TurnOutcome, Usage } from "./protocol";

const usd = new Intl.NumberFormat("fr-FR", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 4,
});
const integer = new Intl.NumberFormat("fr-FR");

export function formatCost(cost: number): string {
  return usd.format(cost);
}

/** « 13 → 2 tokens · 0,001 $ US » ; vide si le moteur n'a rien rapporté. */
export function formatUsage(usage: Usage | null): string {
  if (!usage) return "";
  const parts = [`${integer.format(usage.input_tokens)} → ${integer.format(usage.output_tokens)} tokens`];
  if (usage.cost_usd != null) parts.push(formatCost(usage.cost_usd));
  return parts.join(" · ");
}

export function outcomeLabel(outcome: TurnOutcome): string {
  switch (outcome.status) {
    case "completed":
      return "Tour terminé";
    case "interrupted":
      return "Tour interrompu";
    case "failed":
      return `Tour en échec : ${outcome.message}`;
  }
}

/** « claude · claude-opus-5-5 », ou le modèle par défaut du moteur. */
export function engineLabel(agent: AgentStatus): string {
  return `${agent.engine} · ${agent.model ?? "modèle par défaut"}`;
}

export const levelLabel: Record<AgentStatus["level"], string> = {
  assistant: "Assistant",
  agent: "Agent",
  worker: "Worker",
};

export const stateLabel: Record<AgentStatus["state"], string> = {
  active: "actif",
  parked: "parké",
  retired: "retiré",
};
