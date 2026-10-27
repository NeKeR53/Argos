import { describe, expect, it } from "vitest";
import { captureShortcut, formatShortcut, matchesShortcut, parseShortcut, type KeyPress } from "./shortcuts";

const press = (code: string, mods: Partial<KeyPress> = {}): KeyPress => ({
  code,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe("capture d'un raccourci", () => {
  it("assemble modificateurs et touche dans l'ordre attendu", () => {
    expect(captureShortcut(press("KeyK", { metaKey: true, shiftKey: true }))).toEqual({
      kind: "shortcut",
      keys: "Cmd+Shift+K",
    });
    expect(captureShortcut(press("Space", { altKey: true }))).toEqual({ kind: "shortcut", keys: "Alt+Space" });
    expect(captureShortcut(press("Digit1", { ctrlKey: true, altKey: true }))).toEqual({
      kind: "shortcut",
      keys: "Ctrl+Alt+1",
    });
    expect(captureShortcut(press("ArrowUp", { ctrlKey: true }))).toEqual({ kind: "shortcut", keys: "Ctrl+Up" });
  });

  it("accepte une touche de fonction seule, pas une lettre seule", () => {
    expect(captureShortcut(press("F13"))).toEqual({ kind: "shortcut", keys: "F13" });
    expect(captureShortcut(press("KeyA"))).toEqual({ kind: "needs_modifier", key: "A" });
  });

  it("attend la touche principale et s'annule avec Échap", () => {
    expect(captureShortcut(press("ShiftLeft", { shiftKey: true }))).toEqual({ kind: "waiting" });
    expect(captureShortcut(press("IntlBackslash", { altKey: true }))).toEqual({ kind: "waiting" });
    expect(captureShortcut(press("Escape"))).toEqual({ kind: "cancel" });
  });
});

describe("lecture et affichage", () => {
  it("CmdOrCtrl dépend du système", () => {
    expect(parseShortcut("CmdOrCtrl+Shift+X", true)).toMatchObject({ cmd: true, ctrl: false, shift: true, key: "x" });
    expect(parseShortcut("CmdOrCtrl+Shift+X", false)).toMatchObject({ cmd: false, ctrl: true });
    expect(parseShortcut("Alt+", true)).toBeNull();
    expect(parseShortcut("A+B", true)).toBeNull();
  });

  it("affiche les symboles de macOS ou les noms ailleurs", () => {
    expect(formatShortcut("Alt+Space", true)).toBe("⌥Espace");
    expect(formatShortcut("CmdOrCtrl+Shift+A", true)).toBe("⇧⌘A");
    expect(formatShortcut("CmdOrCtrl+Shift+A", false)).toBe("Ctrl+Maj+A");
    expect(formatShortcut("F13", true)).toBe("F13");
  });

  it("reconnaît le raccourci dans la page", () => {
    const panic = "CmdOrCtrl+Shift+X";
    expect(matchesShortcut(press("KeyX", { metaKey: true, shiftKey: true }), panic, true)).toBe(true);
    expect(matchesShortcut(press("KeyX", { metaKey: true }), panic, true)).toBe(false);
    expect(matchesShortcut(press("KeyX", { ctrlKey: true, shiftKey: true }), panic, false)).toBe(true);
    expect(matchesShortcut(press("KeyP", { altKey: true }), "Alt+P", true)).toBe(true);
  });
});
