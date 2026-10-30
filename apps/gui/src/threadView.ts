// Ce que la conversation affiche, à partir du fil brut d'un agent : les appels d'outils d'un
// tour se regroupent en une seule ligne (« 3 outils utilisés »), placée avant la réponse ;
// les fins de tour réussies et les sorties de process, techniques, ne s'affichent pas.

import type { ThreadEntry } from "./state";

export type ToolEntry = Extract<ThreadEntry, { kind: "tool" }>;

export type ThreadItem =
  | { kind: "entry"; key: number; entry: ThreadEntry }
  /** Outils consécutifs d'un tour ; `live` tant que l'agent travaille encore dessus. */
  | { kind: "tools"; key: number; tools: ToolEntry[]; live: boolean };

/**
 * Regroupe les outils d'un tour et masque ce qui n'aide pas à suivre la conversation.
 * `busy` : l'agent a un tour en cours ; le dernier groupe d'outils, s'il appartient à ce
 * tour, est alors « vivant » (une seule ligne, avec le dernier outil).
 */
export function threadItems(entries: ThreadEntry[], busy: boolean): ThreadItem[] {
  const items: ThreadItem[] = [];
  // Groupe d'outils du tour en cours : tous les outils d'un tour s'y rangent, même
  // séparés par un message intermédiaire (« Je regarde… »).
  let group: Extract<ThreadItem, { kind: "tools" }> | null = null;
  for (const entry of entries) {
    // Un message reçu ou une fin de tour ferme le tour.
    if (entry.kind === "input" || entry.kind === "turn" || entry.kind === "exited") group = null;
    if (entry.kind === "exited") continue;
    if (entry.kind === "turn" && entry.outcome.status === "completed") continue;
    if (entry.kind === "tool") {
      if (group) {
        group.tools.push(entry);
      } else {
        group = { kind: "tools", key: entry.seq, tools: [entry], live: false };
        items.push(group);
      }
      continue;
    }
    items.push({ kind: "entry", key: entry.seq, entry });
  }
  // Tour en cours : son groupe d'outils montre ce que l'agent fait maintenant.
  if (busy && group) group.live = true;
  return items;
}

/** « 1 outil utilisé », « 3 outils utilisés ». */
export function toolsSummary(count: number): string {
  return count > 1 ? `${count} outils utilisés` : "1 outil utilisé";
}
