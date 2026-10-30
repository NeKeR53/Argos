// Avatar d'une conversation : la pieuvre pour l'Assistant, un monogramme coloré pour un
// Agent (teinte tirée de son nom, stable d'une session à l'autre).

import type { AgentStatus } from "../protocol";
import { displayName } from "../state";
import { Octopus } from "./Octopus";

/** Initiales d'un nom : « Courrier Alexandre » → « CA », « maison » → « MA ». */
export function initials(name: string): string {
  const words = name.trim().split(/[\s\-_]+/).filter(Boolean);
  if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
  return (words[0] ?? "?").slice(0, 2).toUpperCase();
}

/** Teinte (0–359) tirée d'un nom. */
export function hueOf(name: string): number {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return hash % 360;
}

export function Avatar({ agent, size = "md" }: { agent: AgentStatus; size?: "sm" | "md" | "lg" }) {
  if (agent.level === "assistant") {
    return (
      <span className={`avatar avatar-${size} avatar-assistant`} aria-hidden="true">
        <Octopus size={size === "lg" ? 50 : size === "sm" ? 28 : 36} />
      </span>
    );
  }
  const name = displayName(agent);
  return (
    <span className={`avatar avatar-${size}`} style={{ ["--hue" as string]: hueOf(name) }} aria-hidden="true">
      {initials(name)}
    </span>
  );
}
