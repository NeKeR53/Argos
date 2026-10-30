import { describe, expect, it } from "vitest";
import { SHAPES } from "../orb/mouth";
import { eyeOpen, gaze, mixPose, pose, type OctopusMode, type Pose } from "./pose";

const MODES: OctopusMode[] = ["idle", "listening", "thinking", "speaking", "offline"];

/** Tous les points d'une posture, pour vérifier qu'elle tient dans la grille. */
function points(p: Pose): [number, number][] {
  return [...p.head, ...p.back.flat(), ...p.front.flat(), ...p.over.flat()];
}

describe("posture de la pieuvre", () => {
  it("tient dans la grille 64×64 dans toutes les attitudes, à tout instant", () => {
    for (const mode of MODES) {
      for (let t = 0; t < 12; t += 0.37) {
        const p = pose({ mode, t, level: 1, mouth: SHAPES.open });
        for (const [x, y] of points(p)) {
          expect(x).toBeGreaterThan(0);
          expect(x).toBeLessThan(64);
          expect(y).toBeGreaterThan(0);
          expect(y).toBeLessThan(64);
        }
      }
    }
  });

  it("est la même pour le même instant (pas de hasard)", () => {
    expect(pose({ mode: "idle", t: 3.21 })).toEqual(pose({ mode: "idle", t: 3.21 }));
  });

  it("cligne des yeux de temps en temps, mais les garde ouverts presque toujours", () => {
    let closed = 0;
    const samples = 2000;
    for (let i = 0; i < samples; i++) if (eyeOpen(i * 0.01) < 0.5) closed++;
    expect(closed).toBeGreaterThan(0);
    expect(closed / samples).toBeLessThan(0.1);
  });

  it("dort hors ligne, lève un bras sous le menton pour réfléchir", () => {
    expect(pose({ mode: "offline", t: 1 }).eyes[0].open).toBe(0);
    expect(pose({ mode: "thinking", t: 1 }).over).toHaveLength(1);
    expect(pose({ mode: "idle", t: 1 }).over).toHaveLength(0);
  });

  it("regarde le pointeur quand on le lui donne, en restant dans l'œil", () => {
    expect(gaze("idle", 5, [3, 0])).toEqual([1, 0]);
    const eye = pose({ mode: "idle", t: 0, look: [0, -1] }).eyes[0];
    expect(eye.py - eye.pr).toBeGreaterThanOrEqual(eye.cy - eye.ry);
  });

  it("ouvre la bouche avec le visème", () => {
    const closed = pose({ mode: "speaking", t: 1, mouth: SHAPES.closed }).mouth;
    const open = pose({ mode: "speaking", t: 1, mouth: SHAPES.open }).mouth;
    expect(open.lower).toBeGreaterThan(closed.lower + 3);
  });

  it("passe d'une attitude à l'autre sans perdre de bras", () => {
    for (const [a, b] of [
      ["idle", "thinking"],
      ["thinking", "speaking"],
    ] as const) {
      const mid = mixPose(pose({ mode: a, t: 2 }), pose({ mode: b, t: 2 }), 0.5);
      expect(mid.front.length + mid.over.length).toBe(6);
    }
  });
});
