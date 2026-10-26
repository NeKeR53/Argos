// Page d'une surface, composée pour l'attribut `srcdoc` de son iframe bac à sable (D28) :
// CSP sans aucun accès réseau, puis le SDK, puis le HTML de l'agent tel quel.

import type { SurfaceDoc } from "../protocol";
import markdownSource from "./markdown.js?raw";
import sdkSource from "./sdk.js?raw";

/**
 * Politique de la page : scripts et styles en ligne seulement, images, polices et médias
 * en `data:` ou `blob:`, aucune connexion, aucun cadre, aucun formulaire envoyé. Une
 * autre CSP posée par l'agent ne peut que la resserrer.
 */
export const SURFACE_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline'",
  "style-src 'unsafe-inline'",
  "img-src data: blob:",
  "font-src data:",
  "media-src data: blob:",
  "connect-src 'none'",
  "frame-src 'none'",
  "worker-src 'none'",
  "object-src 'none'",
  "form-action 'none'",
  "base-uri 'none'",
].join("; ");

/** Attributs de l'iframe : scripts permis, rien d'autre (pas d'`allow-same-origin`). */
export const SURFACE_SANDBOX = "allow-scripts";

export function buildSrcdoc(doc: SurfaceDoc): string {
  const surface = { id: doc.id, title: doc.title, kind: doc.kind, version: doc.version };
  return [
    "<!doctype html>",
    '<html lang="fr">',
    "<head>",
    '<meta charset="utf-8">',
    // En premier : la politique s'applique à tout ce qui suit, SDK compris.
    `<meta http-equiv="Content-Security-Policy" content="${SURFACE_CSP}">`,
    '<meta name="viewport" content="width=device-width, initial-scale=1">',
    `<title>${escapeHtml(doc.title)}</title>`,
    `<script>window.__argosSurface = ${scriptJson(surface)};</script>`,
    `<script>\n${markdownSource}\n</script>`,
    `<script>\n${sdkSource}\n</script>`,
    "</head>",
    "<body>",
    doc.html,
    "</body>",
    "</html>",
  ].join("\n");
}

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/**
 * JSON sûr dans un `<script>` : `<`, `>` et `&` échappés, ainsi que les séparateurs de
 * ligne Unicode. Un titre contenant `</script>` ne ferme donc pas le script.
 */
function scriptJson(value: unknown): string {
  return JSON.stringify(value)
    .replace(/</g, "\\u003c")
    .replace(/>/g, "\\u003e")
    .replace(/&/g, "\\u0026")
    .replace(/\u2028/g, "\\u2028")
    .replace(/\u2029/g, "\\u2029");
}
