// Pièces jointes de la saisie : lues en base64 avant l'envoi. Les limites reprennent
// celles du daemon (`crates/argos-daemon/src/attachments.rs`) pour refuser tôt.

import type { Attachment } from "./protocol";

export const MAX_ATTACHMENTS = 8;
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

/** Les fichiers retenus, et pourquoi les autres ne l'ont pas été. */
export function admit(current: File[], added: File[]): { files: File[]; refused: string | null } {
  const files = [...current];
  let refused: string | null = null;
  for (const file of added) {
    if (file.size > MAX_ATTACHMENT_BYTES) refused = `${file.name} dépasse 10 Mo`;
    else if (files.length >= MAX_ATTACHMENTS) refused = `${MAX_ATTACHMENTS} pièces jointes au plus`;
    else files.push(file);
  }
  return { files, refused };
}

/** Une image collée n'a pas de vrai nom : on lui en donne un lisible. */
export function pastedName(file: File, index: number): File {
  if (file.name && file.name !== "image.png") return file;
  const extension = file.type.split("/")[1] ?? "png";
  return new File([file], `capture-${index + 1}.${extension}`, { type: file.type });
}

export async function toAttachment(file: File): Promise<Attachment> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  return { name: file.name, media_type: file.type, data: base64(bytes) };
}

export function base64(bytes: Uint8Array): string {
  let binary = "";
  // Par tranches : `String.fromCharCode(...bytes)` déborde la pile sur un gros fichier.
  for (let start = 0; start < bytes.length; start += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000));
  }
  return btoa(binary);
}
