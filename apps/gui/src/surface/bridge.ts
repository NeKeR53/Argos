// Pont entre une surface (iframe bac à sable) et la console, par `postMessage`. La
// console n'accepte que les messages de la `contentWindow` de l'iframe de cette surface,
// au format du SDK (`sdk.js`) ; tout le reste est ignoré. L'iframe n'atteint jamais
// l'API Tauri ni le WebSocket : la console relaie, après vérification.

import type { Json } from "../protocol";
import type { FeedItem } from "../state";

/** Bornes des messages acceptés d'une surface. */
export const MAX_TEXT = 64 * 1024;
export const MAX_EVENT_NAME = 100;
export const MAX_PAYLOAD = 64 * 1024;

/** Ce qu'une surface peut demander. */
export type FromSurface =
  /** Le SDK est chargé : la console lui remet le flux complet. */
  | { type: "ready" }
  | { type: "send"; text: string }
  | { type: "emit"; name: string; payload: Json };

/** Ce que la console remet à une surface. */
export interface ToSurface {
  argos: 1;
  type: "feed";
  items: FeedItem[];
  /** État du tour de l'agent au chargement de la page. */
  busy?: boolean;
}

/**
 * Lit un message reçu par la fenêtre de la console. Rend `null` s'il ne vient pas de
 * `frame` (la `contentWindow` de l'iframe de la surface) ou s'il n'est pas au format du SDK.
 */
export function readBridgeMessage(
  event: { source: unknown; data: unknown },
  frame: Window | null | undefined,
): FromSurface | null {
  if (!frame || event.source !== frame) return null;
  const data = event.data;
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const message = data as Record<string, unknown>;
  if (message.argos !== 1) return null;
  switch (message.type) {
    case "ready":
      return { type: "ready" };
    case "send": {
      const text = message.text;
      if (typeof text !== "string" || !text.trim() || text.length > MAX_TEXT) return null;
      return { type: "send", text };
    }
    case "emit": {
      const name = message.name;
      if (typeof name !== "string" || !name.trim() || name.length > MAX_EVENT_NAME) return null;
      const payload = jsonOf(message.payload);
      return payload === undefined ? null : { type: "emit", name, payload };
    }
    default:
      return null;
  }
}

/** Copie JSON bornée d'une valeur ; `undefined` si elle n'est pas sérialisable ou trop grosse. */
function jsonOf(value: unknown): Json | undefined {
  try {
    const text = JSON.stringify(value === undefined ? null : value);
    if (text === undefined || text.length > MAX_PAYLOAD) return undefined;
    return JSON.parse(text) as Json;
  } catch {
    return undefined;
  }
}

export function feedMessage(items: FeedItem[], busy?: boolean): ToSurface {
  return busy === undefined ? { argos: 1, type: "feed", items } : { argos: 1, type: "feed", items, busy };
}
