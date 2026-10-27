// Raccourcis globaux de l'app (`[gui]` de config.toml) : lecture, changement à chaud par
// le cœur Rust, et conversion entre une touche pressée et le format des raccourcis
// (`Alt+Space`, `CmdOrCtrl+Shift+A`, `F13`…).

import { invoke, isTauri } from "@tauri-apps/api/core";

export type ShortcutKey = "push_to_talk" | "console" | "panic";

export interface Shortcuts {
  push_to_talk: string;
  console: string;
  panic: string;
  /** Raccourcis refusés au démarrage (illisibles, ou pris par une autre app). */
  errors: string[];
}

export const SHORTCUT_LABELS: Record<ShortcutKey, string> = {
  push_to_talk: "Parler à l'Assistant (maintenu)",
  console: "Montrer ou cacher la console",
  panic: "Panic stop (deux appuis)",
};

/** Ceux par défaut, tant que le cœur Rust n'a pas répondu (ou hors de l'app). */
export const DEFAULT_SHORTCUTS: Shortcuts = {
  push_to_talk: "Alt+Space",
  console: "CmdOrCtrl+Shift+A",
  panic: "CmdOrCtrl+Shift+X",
  errors: [],
};

export async function loadShortcuts(): Promise<Shortcuts> {
  if (!isTauri()) return DEFAULT_SHORTCUTS;
  return invoke<Shortcuts>("shortcuts");
}

/** Actif tout de suite et enregistré ; rejeté avec la raison s'il est refusé. */
export async function saveShortcut(key: ShortcutKey, keys: string): Promise<Shortcuts> {
  return invoke<Shortcuts>("set_shortcut", { key, keys });
}

/** Ce qu'il faut d'un événement clavier. */
export interface KeyPress {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

const MODIFIER_CODES = new Set([
  "MetaLeft",
  "MetaRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "CapsLock",
  "Fn",
]);

const NAMED_CODES: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Minus: "Minus",
  Equal: "Equal",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Backslash: "Backslash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Backquote: "Backquote",
};

/** La touche principale au format des raccourcis, ou `null` (modificateur seul, inconnue). */
function mainKey(code: string): string | null {
  if (MODIFIER_CODES.has(code)) return null;
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return NAMED_CODES[code] ?? null;
}

export type Capture =
  | { kind: "shortcut"; keys: string }
  /** Encore rien d'utilisable : seulement des modificateurs, ou une touche inconnue. */
  | { kind: "waiting" }
  /** Une touche ordinaire sans modificateur volerait la frappe à toutes les apps. */
  | { kind: "needs_modifier"; key: string }
  | { kind: "cancel" };

/** Ce qu'une touche pressée donne pendant la capture d'un nouveau raccourci. */
export function captureShortcut(press: KeyPress): Capture {
  const modifiers = [
    press.metaKey && "Cmd",
    press.ctrlKey && "Ctrl",
    press.altKey && "Alt",
    press.shiftKey && "Shift",
  ].filter((modifier): modifier is string => Boolean(modifier));
  if (press.code === "Escape" && modifiers.length === 0) return { kind: "cancel" };
  const key = mainKey(press.code);
  if (!key) return { kind: "waiting" };
  const functionKey = /^F\d+$/.test(key);
  if (modifiers.length === 0 && !functionKey) return { kind: "needs_modifier", key };
  return { kind: "shortcut", keys: [...modifiers, key].join("+") };
}

interface Parsed {
  cmd: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

/** Lit un raccourci ; `CmdOrCtrl` vaut ⌘ sur macOS et Ctrl ailleurs. */
export function parseShortcut(keys: string, mac: boolean): Parsed | null {
  const parsed: Parsed = { cmd: false, ctrl: false, alt: false, shift: false, key: "" };
  for (const raw of keys.split("+")) {
    const part = raw.trim().toLowerCase();
    if (part === "cmdorctrl" || part === "commandorcontrol") {
      if (mac) parsed.cmd = true;
      else parsed.ctrl = true;
    } else if (part === "cmd" || part === "command" || part === "super" || part === "meta") parsed.cmd = true;
    else if (part === "ctrl" || part === "control") parsed.ctrl = true;
    else if (part === "alt" || part === "option") parsed.alt = true;
    else if (part === "shift") parsed.shift = true;
    else if (part && !parsed.key) parsed.key = part;
    else return null;
  }
  return parsed.key ? parsed : null;
}

/** La touche pressée correspond-elle à ce raccourci ? (raccourcis gérés dans la page) */
export function matchesShortcut(press: KeyPress, keys: string, mac: boolean): boolean {
  const wanted = parseShortcut(keys, mac);
  const key = mainKey(press.code);
  if (!wanted || !key) return false;
  return (
    wanted.cmd === press.metaKey &&
    wanted.ctrl === press.ctrlKey &&
    wanted.alt === press.altKey &&
    wanted.shift === press.shiftKey &&
    wanted.key === key.toLowerCase()
  );
}

const KEY_NAMES: Record<string, string> = {
  space: "Espace",
  enter: "Entrée",
  tab: "Tab",
  backspace: "⌫",
  delete: "Suppr",
  up: "↑",
  down: "↓",
  left: "←",
  right: "→",
};

/** Affichage : `⌥Espace`, `⌘⇧A` sur macOS ; `Alt+Espace`, `Ctrl+Maj+A` ailleurs. */
export function formatShortcut(keys: string, mac: boolean): string {
  const parsed = parseShortcut(keys, mac);
  if (!parsed) return keys;
  const key = KEY_NAMES[parsed.key] ?? (parsed.key.length === 1 ? parsed.key.toUpperCase() : capitalize(parsed.key));
  if (mac) {
    return `${parsed.ctrl ? "⌃" : ""}${parsed.alt ? "⌥" : ""}${parsed.shift ? "⇧" : ""}${parsed.cmd ? "⌘" : ""}${key}`;
  }
  return [parsed.cmd && "Super", parsed.ctrl && "Ctrl", parsed.alt && "Alt", parsed.shift && "Maj", key]
    .filter(Boolean)
    .join("+");
}

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}
