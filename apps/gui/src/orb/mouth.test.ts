import { describe, expect, it } from "vitest";
import { lipsPath } from "./Face";
import { ATTACK_S, barHeight, RELEASE_S, smoothLevel } from "./level";
import { pillSize } from "./Pill";
import {
  duration,
  LONG_PAUSE_S,
  mouthAt,
  SAY_CHARS_PER_SECOND,
  SHAPES,
  SHORT_PAUSE_S,
  timeline,
  TRANSITION_S,
  visemeFor,
} from "./mouth";

describe("lissage du niveau", () => {
  it("monte vite et redescend doucement", () => {
    // Après une constante de temps, ~63 % du chemin est fait, dans les deux sens.
    expect(smoothLevel(0, 1, ATTACK_S)).toBeCloseTo(1 - Math.exp(-1), 5);
    expect(smoothLevel(1, 0, RELEASE_S)).toBeCloseTo(Math.exp(-1), 5);
    // À durée égale, la montée va bien plus loin que la descente.
    const up = smoothLevel(0, 1, 0.05);
    const down = 1 - smoothLevel(1, 0, 0.05);
    expect(up).toBeGreaterThan(0.7);
    expect(down).toBeLessThan(0.2);
  });

  it("reste dans 0..1 et ignore un pas nul ou un niveau illisible", () => {
    expect(smoothLevel(0.5, 3, 1)).toBeLessThanOrEqual(1);
    expect(smoothLevel(0.5, -2, 10)).toBeGreaterThanOrEqual(0);
    expect(smoothLevel(0.4, 1, 0)).toBe(0.4);
    expect(smoothLevel(0.4, Number.NaN, 0.1)).toBeLessThan(0.4);
  });

  it("dessine des ondes plus hautes au centre, visibles même en silence", () => {
    const heights = Array.from({ length: 9 }, (_, i) => barHeight(i, 9, 1, 0, true));
    expect(heights[4]).toBeGreaterThan(heights[0]);
    expect(heights[4]).toBeCloseTo(1, 5);
    expect(barHeight(4, 9, 0, 0)).toBeCloseTo(0.1, 5);
  });
});

describe("taille de la pastille", () => {
  const view = { mode: "idle" as const, armed: false, stopped: false, caption: null, escalation: false };

  it("reste petite au repos et ne grandit que s'il se passe quelque chose", () => {
    expect(pillSize(view)).toEqual({ width: 68, height: 36 });
    expect(pillSize({ ...view, mode: "offline" })).toEqual({ width: 68, height: 36 });
    for (const busy of [
      pillSize({ ...view, mode: "thinking" }),
      pillSize({ ...view, mode: "listening" }),
      pillSize({ ...view, mode: "speaking" }),
      pillSize({ ...view, armed: true }),
    ]) {
      expect(busy.width).toBeGreaterThan(68);
    }
    // La transcription élargit l'écoute ; la confirmation du panic stop passe avant tout.
    expect(pillSize({ ...view, mode: "listening", caption: "Allume" }).width).toBeGreaterThan(
      pillSize({ ...view, mode: "listening" }).width,
    );
    expect(pillSize({ ...view, mode: "speaking", armed: true })).toEqual(pillSize({ ...view, armed: true }));
  });
});

describe("formes de bouche", () => {
  it("associe lettres et formes", () => {
    expect(["m", "b", "P"].map((c) => visemeFor(c))).toEqual(["closed", "closed", "closed"]);
    expect(["a", "à", "â"].map((c) => visemeFor(c))).toEqual(["open", "open", "open"]);
    expect(["o", "u", "ô"].map((c) => visemeFor(c))).toEqual(["round", "round", "round"]);
    expect(visemeFor("o", "u")).toBe("round");
    expect(visemeFor("a", "u")).toBe("round");
    expect(["é", "i", "y"].map((c) => visemeFor(c))).toEqual(["wide", "wide", "wide"]);
    expect(["e", "è", "ê"].map((c) => visemeFor(c))).toEqual(["mid", "mid", "mid"]);
    expect(["s", "t", "r"].map((c) => visemeFor(c))).toEqual(["slight", "slight", "slight"]);
    expect([" ", ",", "."].map((c) => visemeFor(c))).toEqual(["rest", "rest", "rest"]);
  });

  it("suit le texte au débit de la voix, pauses comprises", () => {
    const text = "Bonjour, ça va.";
    const segments = timeline(text);
    const chars = Array.from(text).length;
    expect(duration(segments)).toBeCloseTo(chars / SAY_CHARS_PER_SECOND + SHORT_PAUSE_S + LONG_PAUSE_S, 5);
    // « Bonjour » : fermée (B), ronde (o), légère (n, j), ronde (ou), légère (r).
    expect(segments.slice(0, 5).map((s) => s.viseme)).toEqual(["closed", "round", "slight", "round", "slight"]);
    // Les segments se suivent sans trou.
    segments.slice(1).forEach((segment, index) => {
      const previous = segments[index];
      expect(segment.start).toBeCloseTo(previous.start + previous.duration, 9);
    });
  });

  it("interpole d'une forme à la suivante, puis revient au repos", () => {
    // « aaa » fusionne en une seule ouverture, assez longue pour être atteinte.
    const segments = timeline("maaa");
    const step = 1 / SAY_CHARS_PER_SECOND;
    expect(mouthAt(segments, 0)).toEqual(SHAPES.rest);
    // Une lettre dure moins qu'une transition : les lèvres se rapprochent sans se fermer
    // tout à fait (comme en parlant vite).
    const closing = mouthAt(segments, step - 0.001).open;
    expect(closing).toBeLessThan(SHAPES.rest.open);
    expect(closing).toBeGreaterThan(SHAPES.closed.open);
    // À mi-transition vers « a », la bouche s'ouvre à moitié.
    expect(mouthAt(segments, step + TRANSITION_S / 2).open).toBeCloseTo(0.5, 5);
    expect(mouthAt(segments, step + TRANSITION_S).open).toBeCloseTo(SHAPES.open.open, 5);
    expect(segments).toHaveLength(2);
    expect(mouthAt(segments, 4 * step + TRANSITION_S)).toEqual(SHAPES.rest);
    expect(mouthAt([], 1)).toEqual(SHAPES.rest);
  });

  it("dessine des lèvres plus ouvertes pour « a » que fermées pour « m »", () => {
    // « M x y Q cx haut x y Q cx bas x y Z » : écart entre les deux points de contrôle.
    const gap = (path: string) => {
      const numbers = (path.match(/-?[\d.]+/g) ?? []).map(Number);
      return numbers[7] - numbers[3];
    };
    expect(gap(lipsPath(SHAPES.open))).toBeGreaterThan(gap(lipsPath(SHAPES.closed)) + 10);
  });
});
