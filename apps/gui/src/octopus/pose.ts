// La pieuvre, interlocutrice : sa posture à l'instant `t`, calculée sans état ni hasard
// (le « hasard » des clignements et du regard vient d'un hachage du temps). Le même calcul
// sert la console, l'orbe et la tuile du Dock (portage dans `src-tauri/src/octopus.rs`).
//
// Grille 64×64, origine en haut à gauche. Tout est déjà placé (respiration, inclinaison) :
// le rendu n'a qu'à tracer.

import type { MouthShape } from "../orb/mouth";

export type OctopusMode = "idle" | "listening" | "thinking" | "speaking" | "offline";

export type Pt = [number, number];

export interface PoseInput {
  mode: OctopusMode;
  /** Temps (s) : anime tout. Figé à 0 quand les animations sont coupées. */
  t: number;
  /** Niveau de la voix (micro à l'écoute), 0..1. */
  level?: number;
  /** Bouche à la parole (visème du moment, voir `orb/mouth.ts`). */
  mouth?: MouthShape;
  /** Direction du regard imposée (pointeur), dans le disque unité ; sinon elle regarde où elle veut. */
  look?: Pt | null;
}

export interface Eye {
  cx: number;
  cy: number;
  rx: number;
  ry: number;
  /** Ouverture de la paupière : 1 grand ouvert, 0 fermé. */
  open: number;
  /** Centre de la pupille et rayon. */
  px: number;
  py: number;
  pr: number;
}

/** Bouche : une courbe (`curve` > 0 sourit) tant qu'elle est fermée, des lèvres sinon. */
export interface Mouth {
  x: number;
  y: number;
  half: number;
  upper: number;
  lower: number;
  curve: number;
}

export interface Bubble {
  x: number;
  y: number;
  r: number;
  alpha: number;
}

export interface Pose {
  head: Pt[];
  /** Contours fermés des tentacules : derrière, devant, et celui qui passe devant la tête. */
  back: Pt[][];
  front: Pt[][];
  over: Pt[][];
  suckers: Bubble[];
  eyes: [Eye, Eye];
  /** Sourcils : départ, contrôle, arrivée (courbe quadratique), et opacité. */
  brows: [Pt, Pt, Pt][];
  browAlpha: number;
  cheeks: [Bubble, Bubble];
  mouth: Mouth;
  bubbles: Bubble[];
}

// --- Outils ---------------------------------------------------------------------------

/** Pseudo-hasard stable dans [0, 1). */
export function hash(n: number): number {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const smooth = (e0: number, e1: number, x: number) => {
  const k = clamp((x - e0) / (e1 - e0), 0, 1);
  return k * k * (3 - 2 * k);
};
const lerp = (a: number, b: number, k: number) => a + (b - a) * k;

// --- Comportements --------------------------------------------------------------------

const BLINK_PERIOD = 3.7;
const BLINK_S = 0.17;

/** Ouverture des yeux : un clignement à un moment différent de chaque période, parfois double. */
export function eyeOpen(t: number): number {
  const n = Math.floor(t / BLINK_PERIOD);
  const at = 0.4 + hash(n) * 2.6;
  const local = t - n * BLINK_PERIOD;
  let closure = 0;
  for (const start of hash(n + 0.5) > 0.7 ? [at, at + 0.3] : [at]) {
    const dt = local - start;
    if (dt >= 0 && dt < BLINK_S) closure = Math.max(closure, Math.sin((Math.PI * dt) / BLINK_S));
  }
  return 1 - closure;
}

const GAZE_PERIOD = 1.9;

/** Où regarder pendant la période `n` : vers toi le plus souvent, ailleurs de temps en temps. */
function gazeTarget(mode: OctopusMode, n: number): Pt {
  const a = hash(n * 1.31) * 2 - 1;
  const b = hash(n * 2.17 + 4) * 2 - 1;
  switch (mode) {
    case "thinking":
      // Les yeux en l'air, qui cherchent.
      return [-0.55 + 0.35 * a, -0.8 + 0.15 * b];
    case "listening":
    case "speaking":
      return [0.18 * a, 0.12 * b];
    case "offline":
      return [0, 0.4];
    default:
      return hash(n * 0.77 + 9) > 0.45 ? [0.12 * a, 0.1 * b] : [0.85 * a, 0.55 * b];
  }
}

/** Regard : des saccades d'une cible à la suivante. */
export function gaze(mode: OctopusMode, t: number, look?: Pt | null): Pt {
  if (look) {
    const len = Math.hypot(look[0], look[1]);
    const k = len > 1 ? 1 / len : 1;
    return [look[0] * k, look[1] * k];
  }
  const n = Math.floor(t / GAZE_PERIOD);
  const from = gazeTarget(mode, n - 1);
  const to = gazeTarget(mode, n);
  const k = smooth(0, 0.2, t - n * GAZE_PERIOD);
  return [lerp(from[0], to[0], k), lerp(from[1], to[1], k)];
}

// --- Tentacules -----------------------------------------------------------------------

interface Arm {
  /** Décalage horizontal de la base depuis le centre de la tête. */
  dx: number;
  /** Angle de départ (rad, 0 = vers le bas, positif = vers la droite). */
  angle: number;
  length: number;
  /** Enroulement du bout (rad, signe = sens). */
  curl: number;
  phase: number;
}

const FRONT: Arm[] = [
  { dx: -14.5, angle: -0.95, length: 15, curl: -1.9, phase: 0 },
  { dx: -8.5, angle: -0.42, length: 18, curl: -1.2, phase: 1.3 },
  { dx: -2.8, angle: -0.1, length: 19, curl: 0.9, phase: 2.6 },
  { dx: 2.8, angle: 0.1, length: 19, curl: -0.9, phase: 3.7 },
  { dx: 8.5, angle: 0.42, length: 18, curl: 1.2, phase: 4.9 },
  { dx: 14.5, angle: 0.95, length: 15, curl: 1.9, phase: 6.1 },
];

const BACK: Arm[] = [
  { dx: -11, angle: -0.7, length: 16, curl: 1.4, phase: 0.7 },
  { dx: 11, angle: 0.7, length: 16, curl: -1.4, phase: 3.3 },
];

const SEGMENTS = 14;

/** Ligne médiane : on avance pas à pas, l'angle ondule et s'enroule vers le bout. */
function spine(base: Pt, angleAt: (s: number) => number, length: number): Pt[] {
  const points: Pt[] = [base];
  let [x, y] = base;
  const step = length / SEGMENTS;
  for (let i = 1; i <= SEGMENTS; i++) {
    const a = angleAt((i - 0.5) / SEGMENTS);
    x += Math.sin(a) * step;
    y += Math.cos(a) * step;
    points.push([x, y]);
  }
  return points;
}

/** Contour d'un tentacule effilé (bout arrondi), à partir de sa ligne médiane. */
function outline(points: Pt[], w0: number, w1: number): Pt[] {
  const left: Pt[] = [];
  const right: Pt[] = [];
  const last = points.length - 1;
  for (let i = 0; i <= last; i++) {
    const a = points[Math.max(0, i - 1)];
    const b = points[Math.min(last, i + 1)];
    const len = Math.hypot(b[0] - a[0], b[1] - a[1]) || 1;
    const nx = -(b[1] - a[1]) / len;
    const ny = (b[0] - a[0]) / len;
    const w = lerp(w0, w1, i / last) / 2;
    left.push([points[i][0] + nx * w, points[i][1] + ny * w]);
    right.push([points[i][0] - nx * w, points[i][1] - ny * w]);
  }
  // Bout arrondi : un demi-cercle autour du dernier point.
  const tip = points[last];
  const [lx, ly] = left[last];
  const start = Math.atan2(ly - tip[1], lx - tip[0]);
  const cap: Pt[] = [];
  for (let k = 1; k < 6; k++) {
    const a = start - (Math.PI * k) / 6;
    cap.push([tip[0] + Math.cos(a) * (w1 / 2), tip[1] + Math.sin(a) * (w1 / 2)]);
  }
  return [...left, ...cap, ...right.reverse()];
}

// --- Pose -----------------------------------------------------------------------------

const CX = 32;
const CY = 24;
const RX = 19.5;
const RY = 17.5;

export function pose({ mode, t, level = 0, mouth, look }: PoseInput): Pose {
  const lv = clamp(level, 0, 1);
  const talk = mode === "speaking" && mouth ? mouth.open : 0;

  // Respiration : la tête monte, descend, s'étire un peu. Plus lente au repos, endormie hors ligne.
  const rate = mode === "offline" ? 0.9 : mode === "thinking" ? 1.4 : 1.7;
  const breath = Math.sin(t * rate * 2);
  const bob = breath * (mode === "offline" ? 0.6 : 1.1) - talk * 1.2 - (mode === "listening" ? lv * 1.2 : 0);
  const stretch = 0.025 * breath + (mode === "listening" ? 0.03 * lv : 0) + 0.02 * talk;

  // Inclinaison : la tête se penche pour écouter, hésite pour réfléchir.
  const tilt =
    mode === "listening"
      ? 0.11 + 0.03 * Math.sin(t * 1.3)
      : mode === "thinking"
        ? -0.07 + 0.03 * Math.sin(t * 0.9)
        : mode === "speaking"
          ? 0.04 * Math.sin(t * 2.3)
          : mode === "offline"
            ? 0.06
            : 0.03 * Math.sin(t * 0.7);

  const cy = CY + bob;
  const rx = RX * (1 - stretch);
  const ry = RY * (1 + stretch);
  // Pivot de l'inclinaison : la base de la tête.
  const pivot: Pt = [CX, cy + RY * 0.7];
  const cos = Math.cos(tilt);
  const sin = Math.sin(tilt);
  const turn = ([x, y]: Pt): Pt => {
    const dx = x - pivot[0];
    const dy = y - pivot[1];
    return [pivot[0] + dx * cos - dy * sin, pivot[1] + dx * sin + dy * cos];
  };

  // Tête : un dôme, plus haut que large en haut, aplati dessous.
  const head: Pt[] = [];
  for (let i = 0; i < 56; i++) {
    const a = (i / 56) * Math.PI * 2;
    const s = Math.sin(a);
    head.push(turn([CX + rx * Math.cos(a), cy + ry * s * (s < 0 ? 1.06 : 0.86)]));
  }

  // Tentacules : une vague qui court de la base au bout, plus vive quand elle parle ou entend.
  const amp =
    mode === "offline" ? 0.08 : mode === "listening" ? 0.16 + 0.4 * lv : mode === "speaking" ? 0.26 + 0.3 * talk : 0.2;
  const speed = mode === "offline" ? 0.8 : mode === "speaking" ? 3.2 : 2.1;
  const baseY = (dx: number) => cy + ry * 0.86 * Math.sqrt(Math.max(0, 1 - (dx / rx) ** 2)) - 3.5;
  const arm = (a: Arm, gesture = 0): Pt[] => {
    const curl = a.curl * (mode === "offline" ? 0.6 : 1) * (1 + 0.15 * Math.sin(t * 1.1 + a.phase));
    const points = spine(
      [CX + a.dx, baseY(a.dx)],
      (s) => a.angle + curl * s * s + (amp + gesture) * Math.sin(t * speed - s * 3.4 + a.phase) * (0.25 + s),
      a.length,
    );
    return points.map(turn);
  };

  const back = BACK.map((a) => outline(arm(a), 4.6, 1.2));
  const front: Pt[][] = [];
  const over: Pt[][] = [];
  const suckers: Bubble[] = [];
  FRONT.forEach((a, i) => {
    // À la parole, les bras extérieurs gesticulent.
    const gesture = mode === "speaking" && (i === 0 || i === 5) ? 0.25 : 0;
    let points: Pt[];
    if (mode === "thinking" && i === 4) {
      // Réflexion : ce bras remonte se poser sous le menton.
      const k = 0.93 + 0.07 * Math.sin(t * 1.6);
      points = spine([CX + a.dx, baseY(a.dx)], (s) => 0.3 + 4.4 * k * smooth(0, 0.5, s), 14).map(turn);
      over.push(outline(points, 5.2, 2.6));
    } else {
      points = arm(a, gesture);
      front.push(outline(points, 5.4, 1.1));
    }
    // Ventouses sur les bras extérieurs, côté intérieur de l'enroulement.
    if (i === 0 || i === 1 || i === 4 || i === 5) {
      for (const s of [0.5, 0.66, 0.8]) {
        const j = Math.round(s * SEGMENTS);
        const [x0, y0] = points[j - 1];
        const [x1, y1] = points[j + 1];
        const len = Math.hypot(x1 - x0, y1 - y0) || 1;
        const side = a.curl > 0 ? -1 : 1;
        const w = lerp(5.4, 1.1, s);
        suckers.push({
          x: points[j][0] + (-(y1 - y0) / len) * side * w * 0.22,
          y: points[j][1] + ((x1 - x0) / len) * side * w * 0.22,
          r: w * 0.2,
          alpha: 0.75,
        });
      }
    }
  });

  // Yeux : grands, tournés vers toi. Ils s'écarquillent pour écouter, se ferment hors ligne.
  const [gx, gy] = gaze(mode, t, look);
  const wide = mode === "listening" ? 1.08 + 0.06 * lv : mode === "speaking" ? 1 + 0.05 * talk : 1;
  const lid =
    mode === "offline" ? 0 : mode === "thinking" ? 0.82 * eyeOpen(t) : eyeOpen(t) * (mode === "listening" ? 1 : 0.96);
  const pr = mode === "listening" ? 2.95 : 2.65;
  const eye = (dx: number): Eye => {
    const [ex, ey] = turn([CX + dx, cy + 0.5]);
    const erx = 4.5 * wide;
    const ery = 5.1 * wide;
    return {
      cx: ex,
      cy: ey,
      rx: erx,
      ry: ery,
      open: lid,
      px: ex + gx * (erx - pr - 0.3),
      py: ey + gy * (ery - pr - 0.3) * lid,
      pr,
    };
  };
  const eyes: [Eye, Eye] = [eye(-7.2), eye(7.2)];

  // Sourcils : levés pour écouter, l'un plus haut que l'autre pour réfléchir, ils
  // suivent la parole.
  const raise =
    mode === "listening" ? 1.6 + lv : mode === "thinking" ? 0.8 : mode === "speaking" ? 0.5 + 1.6 * talk : 0.3;
  const brows = eyes.map((e, i) => {
    const side = i === 0 ? -1 : 1;
    const lift = raise + (mode === "thinking" && i === 0 ? 1.4 : 0);
    const y = e.cy - e.ry - 1.8 - lift;
    const slant = mode === "thinking" && i === 1 ? 0.9 : mode === "listening" ? -0.5 : 0;
    return [
      turn([e.cx - 3.2 * side, y + 1 + slant]),
      turn([e.cx, y - 1.1]),
      turn([e.cx + 3.2 * side, y + 1.4 - slant]),
    ] as [Pt, Pt, Pt];
  });
  const browAlpha = mode === "offline" ? 0 : 0.55;

  const cheekA = turn([CX - 12.6, cy + 7.2]);
  const cheekB = turn([CX + 12.6, cy + 7.2]);
  const blush = mode === "speaking" ? 0.6 + 0.2 * talk : mode === "offline" ? 0.25 : 0.5;
  const cheeks: [Bubble, Bubble] = [
    { x: cheekA[0], y: cheekA[1], r: 3.4, alpha: blush },
    { x: cheekB[0], y: cheekB[1], r: 3.4, alpha: blush },
  ];

  // Bouche.
  const [mx, my] = turn([CX + (mode === "thinking" ? 2 : 0), cy + 9]);
  let shape: Mouth;
  if (mode === "speaking" && mouth) {
    shape = {
      x: mx,
      y: my,
      half: 5.4 * mouth.width * (1 - 0.3 * mouth.round),
      upper: 0.3 + mouth.open * 1.7 + mouth.round,
      lower: 0.6 + mouth.open * 4.8 + mouth.round * 1.2,
      curve: 0,
    };
  } else if (mode === "listening") {
    // Un petit « oh » attentif, qui s'ouvre avec ce qu'elle entend.
    shape = { x: mx, y: my, half: 1.7, upper: 0.9 + 0.4 * lv, lower: 1.4 + 0.9 * lv, curve: 0 };
  } else if (mode === "thinking") {
    shape = { x: mx, y: my, half: 2.4, upper: 0, lower: 0, curve: -0.4 + 0.3 * Math.sin(t * 2.4) };
  } else if (mode === "offline") {
    shape = { x: mx, y: my, half: 2.2, upper: 0, lower: 0, curve: 0.6 };
  } else {
    shape = { x: mx, y: my, half: 3.8, upper: 0, lower: 0, curve: 2.6 + 0.3 * breath };
  }

  // Bulles : des idées qui montent pendant la réflexion, des « z » de sommeil hors ligne.
  const bubbles: Bubble[] = [0, 1, 2].map((k) => {
    const active = mode === "thinking" || mode === "offline";
    const period = mode === "offline" ? 3 : 1.8;
    const p = (((t + (k * period) / 3) % period) + period) % period / period;
    const [bx, by] = turn([CX + 17 + 4 * p + Math.sin(p * 7 + k) * 1.2, cy - 6 - 15 * p]);
    return {
      x: bx,
      y: by,
      r: (mode === "offline" ? 1 : 0.9) + 1.4 * p,
      alpha: active ? Math.sin(Math.PI * p) * 0.9 : 0,
    };
  });

  return { head, back, front, over, suckers, eyes, brows, browAlpha, cheeks, mouth: shape, bubbles };
}

// --- Transitions ----------------------------------------------------------------------

const mixPt = (a: Pt, b: Pt, k: number): Pt => [lerp(a[0], b[0], k), lerp(a[1], b[1], k)];
const mixPts = (a: Pt[], b: Pt[], k: number) => a.map((p, i) => (b[i] ? mixPt(p, b[i], k) : p));
const mixBubble = (a: Bubble, b: Bubble, k: number): Bubble => ({
  x: lerp(a.x, b.x, k),
  y: lerp(a.y, b.y, k),
  r: lerp(a.r, b.r, k),
  alpha: lerp(a.alpha, b.alpha, k),
});
function mixObj<T extends object>(a: T, b: T, k: number): T {
  const out = { ...a } as Record<string, number>;
  for (const key of Object.keys(a)) {
    out[key] = lerp((a as Record<string, number>)[key], (b as Record<string, number>)[key], k);
  }
  return out as T;
}

/**
 * Fondu d'une posture à l'autre quand l'état change. Le bras du menton (réflexion) n'a pas
 * d'équivalent : il prend la place du bras de devant correspondant.
 */
export function mixPose(a: Pose, b: Pose, k: number): Pose {
  if (k <= 0) return a;
  if (k >= 1) return b;
  const arms = (p: Pose) => {
    const all = [...p.front];
    if (p.over.length) all.splice(4, 0, ...p.over);
    return all;
  };
  const fa = arms(a);
  const fb = arms(b);
  const front = fa.map((arm, i) => mixPts(arm, fb[i] ?? arm, k));
  const target = b.over.length ? b : a;
  return {
    head: mixPts(a.head, b.head, k),
    back: a.back.map((arm, i) => mixPts(arm, b.back[i], k)),
    front: target.over.length ? front.filter((_, i) => i !== 4) : front,
    over: target.over.length ? [front[4]] : [],
    suckers: a.suckers.map((s, i) => mixBubble(s, b.suckers[i] ?? s, k)),
    eyes: [mixObj(a.eyes[0], b.eyes[0], k), mixObj(a.eyes[1], b.eyes[1], k)],
    brows: a.brows.map((br, i) => br.map((p, j) => mixPt(p, b.brows[i][j], k)) as [Pt, Pt, Pt]),
    browAlpha: lerp(a.browAlpha, b.browAlpha, k),
    cheeks: [mixBubble(a.cheeks[0], b.cheeks[0], k), mixBubble(a.cheeks[1], b.cheeks[1], k)],
    mouth: mixObj(a.mouth, b.mouth, k),
    bubbles: a.bubbles.map((s, i) => mixBubble(s, b.bubbles[i], k)),
  };
}
