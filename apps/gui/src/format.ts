// Mise en forme des valeurs affichées (coûts, tokens, états).

import type { AgentStatus, TaskState, TurnOutcome, Usage } from "./protocol";

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

/**
 * Part de la fenêtre de contexte occupée par ce tour (0 à 1), si le moteur donne la
 * fenêtre. Même calcul que `Usage::context_fill` côté Rust.
 */
export function contextFill(usage: Usage | null): number | null {
  const window = usage?.context_window;
  if (!usage || !window || window <= 0) return null;
  return usage.input_tokens / window;
}

/** « 13 → 2 tokens · 0,001 $ US · contexte 12 % » ; vide si le moteur n'a rien rapporté. */
export function formatUsage(usage: Usage | null): string {
  if (!usage) return "";
  const parts = [`${integer.format(usage.input_tokens)} → ${integer.format(usage.output_tokens)} tokens`];
  if (usage.cost_usd != null) parts.push(formatCost(usage.cost_usd));
  const fill = contextFill(usage);
  if (fill != null) parts.push(`contexte ${percent.format(fill)}`);
  return parts.join(" · ");
}

const percent = new Intl.NumberFormat("fr-FR", { style: "percent", maximumFractionDigits: 0 });

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

/** « claude · claude-opus-5-5 (code) » : moteur, modèle et profil de la politique. */
export function engineLabel(agent: AgentStatus): string {
  const label = `${agent.engine} · ${agent.model ?? "modèle par défaut"}`;
  return agent.profile ? `${label} (${agent.profile})` : label;
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

export const taskStateLabel: Record<TaskState, string> = {
  pending: "à faire",
  in_progress: "en cours",
  blocked: "bloquée",
  done: "terminée",
  cancelled: "annulée",
  interrupted: "interrompue",
};
