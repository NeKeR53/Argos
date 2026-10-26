// Lecture des messages qu'un agent reçoit (`input`). Ceux qui viennent des outils du
// daemon portent un préfixe entre crochets sur leur première ligne :
//   [Mission m-… de l'Assistant]            [Mission m-… annulée par l'Assistant]
//   [Tâche t-… confiée par …]               [Rapport de …]
//   [Rapport sur la tâche t-… (done) de …]  [Escalade pour l'utilisateur de …]
//   [Question de …]                         [Argos] …
// Un message sans préfixe reconnu est un message libre (utilisateur, ou parent via
// send_to_agent / send_to_worker : le protocole ne dit pas l'expéditeur).

import type { AgentId } from "./protocol";

export type InputCategory = "mission" | "task" | "report" | "escalation" | "question" | "argos";

export interface ClassifiedInput {
  /** `null` pour un message libre. */
  category: InputCategory | null;
  /** Le préfixe sans crochets, ex. « Mission m-1a2b3c4d de l'Assistant ». */
  tag: string | null;
  /** Le texte après le préfixe (le message entier s'il n'y en a pas). */
  body: string;
}

/** Début du préfixe → catégorie. L'ordre compte peu : les débuts sont disjoints. */
const CATEGORIES: [string, InputCategory][] = [
  ["Mission ", "mission"],
  ["Tâche ", "task"],
  ["Rapport", "report"],
  ["Escalade ", "escalation"],
  ["Question ", "question"],
  ["Argos", "argos"],
];

export function classifyInput(text: string): ClassifiedInput {
  const match = /^\[([^\]\n]+)\]/.exec(text);
  const tag = match?.[1];
  const category = tag ? CATEGORIES.find(([start]) => tag.startsWith(start))?.[1] : undefined;
  if (!match || !tag || !category) return { category: null, tag: null, body: text };
  return { category, tag, body: text.slice(match[0].length).replace(/^[ \t]*\n?/, "") };
}

export interface EscalationSource {
  /** Nom de l'Agent tel qu'écrit dans le préfixe. */
  name: string;
  /** Son id, quand le préfixe le donne. */
  agent: AgentId | null;
}

const ESCALATION = "Escalade pour l'utilisateur de ";

/**
 * Une escalade vers l'utilisateur (D12) : `[Escalade pour l'utilisateur de l'Agent « X » (id)]`.
 * Rend `null` pour tout autre préfixe.
 */
export function escalationSource(tag: string | null): EscalationSource | null {
  if (!tag?.startsWith(ESCALATION)) return null;
  const who = tag.slice(ESCALATION.length);
  const named = /^l'Agent « (.+) » \(([^)]+)\)$/.exec(who);
  if (named) return { name: named[1], agent: named[2] };
  return { name: who, agent: null };
}
