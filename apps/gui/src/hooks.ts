// Base des URL de webhooks des déclencheurs (`http://<[mcp] listen>` du daemon). Dans
// l'app Tauri, le cœur Rust la lit dans `config.toml` ; dans un navigateur, elle se passe
// dans l'adresse de la page : `?hooks=<url encodée>`.

import { invoke, isTauri } from "@tauri-apps/api/core";

export async function resolveHooksBase(search: string = window.location.search): Promise<string | null> {
  if (isTauri()) return invoke<string>("hooks_base").catch(() => null);
  return new URLSearchParams(search).get("hooks");
}

/** URL complète d'un webhook : `POST <base>/hooks/<déclencheur>/<secret>`. */
export function webhookUrl(base: string, trigger: string, secret: string): string {
  return `${base.replace(/\/+$/, "")}/hooks/${encodeURIComponent(trigger)}/${encodeURIComponent(secret)}`;
}

/** La même, secret masqué, pour l'affichage. */
export function maskedWebhookUrl(base: string, trigger: string): string {
  return `${base.replace(/\/+$/, "")}/hooks/${encodeURIComponent(trigger)}/••••••••`;
}
