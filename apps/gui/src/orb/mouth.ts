// Synchronisation labiale procédurale : `say` ne donne pas de niveau sonore, alors la
// bouche suit le texte de la phrase en cours (`VoiceEvent::Utterance`), lu au débit de la
// voix. Chaque lettre devient une forme de bouche (visème), et l'on interpole entre elles.

/**
 * Débit de `say -v Thomas`, ponctuation comprise : mesuré sur trois phrases rendues en
 * fichier (15,2, 18,8 et 17,2 caractères par seconde).
 */
export const SAY_CHARS_PER_SECOND = 17;
/** Pause ajoutée après une virgule (ou ; :), puis après un point (ou ! ?), en secondes. */
export const SHORT_PAUSE_S = 0.12;
export const LONG_PAUSE_S = 0.25;
/** Durée d'une transition d'une forme à la suivante (s). */
export const TRANSITION_S = 0.07;

export type Viseme = "rest" | "closed" | "open" | "round" | "wide" | "mid" | "slight";

/** Forme de bouche : ouverture, largeur et arrondi, chacun entre 0 et 1. */
export interface MouthShape {
  open: number;
  width: number;
  round: number;
}

export const SHAPES: Record<Viseme, MouthShape> = {
  rest: { open: 0.08, width: 0.8, round: 0 },
  closed: { open: 0, width: 0.82, round: 0 },
  open: { open: 1, width: 0.9, round: 0 },
  round: { open: 0.55, width: 0.5, round: 1 },
  wide: { open: 0.32, width: 1, round: 0 },
  mid: { open: 0.5, width: 0.78, round: 0.15 },
  slight: { open: 0.2, width: 0.84, round: 0 },
};

/** Une forme tenue de `start` à `start + duration` (s depuis le début de la phrase). */
export interface Segment {
  viseme: Viseme;
  start: number;
  duration: number;
}

/**
 * Visème d'une lettre, en regardant la suivante pour « ou » et « au » (sons ronds). Les
 * accents comptent : é étire la bouche, è et ê l'ouvrent à moitié.
 */
export function visemeFor(char: string, next = ""): Viseme {
  const c = char.toLowerCase();
  const n = next.toLowerCase();
  if ((c === "o" || c === "a") && n === "u") return "round";
  if ("aàâ".includes(c)) return "open";
  if ("oôuùûœ".includes(c)) return "round";
  if ("éiîïyÿ".includes(c)) return "wide";
  if ("eèêë".includes(c)) return "mid";
  if ("mbp".includes(c)) return "closed";
  if (/\p{L}/u.test(c) || /\d/.test(c)) return "slight";
  return "rest";
}

/**
 * Chronologie des formes d'une phrase : une lettre dure `1 / SAY_CHARS_PER_SECOND`, la
 * ponctuation ajoute une pause, et les lettres consécutives de même forme fusionnent.
 */
export function timeline(text: string, charsPerSecond = SAY_CHARS_PER_SECOND): Segment[] {
  const step = 1 / charsPerSecond;
  const chars = Array.from(text);
  const segments: Segment[] = [];
  let time = 0;
  chars.forEach((char, index) => {
    // Le « u » de « ou » ou « au » est déjà compté avec la voyelle d'avant.
    const previous = chars[index - 1]?.toLowerCase();
    const isDigraphTail = char.toLowerCase() === "u" && (previous === "o" || previous === "a");
    const viseme = isDigraphTail ? "round" : visemeFor(char, chars[index + 1]);
    let duration = step;
    if (/[,;:]/.test(char)) duration += SHORT_PAUSE_S;
    if (/[.!?…]/.test(char)) duration += LONG_PAUSE_S;
    const last = segments[segments.length - 1];
    if (last && last.viseme === viseme) last.duration += duration;
    else segments.push({ viseme, start: time, duration });
    time += duration;
  });
  return segments;
}

/** Durée totale d'une chronologie (s). */
export function duration(segments: Segment[]): number {
  const last = segments[segments.length - 1];
  return last ? last.start + last.duration : 0;
}

export function lerpShape(a: MouthShape, b: MouthShape, t: number): MouthShape {
  const k = Math.min(1, Math.max(0, t));
  // Aux bornes, la forme exacte (pas d'arrondi flottant).
  if (k === 0) return { ...a };
  if (k === 1) return { ...b };
  return {
    open: a.open + (b.open - a.open) * k,
    width: a.width + (b.width - a.width) * k,
    round: a.round + (b.round - a.round) * k,
  };
}

/** Courbe douce (0→1) pour les transitions. */
function ease(t: number): number {
  const k = Math.min(1, Math.max(0, t));
  return k * k * (3 - 2 * k);
}

/**
 * Forme de la bouche à l'instant `t` (s depuis le lancement de la phrase) : transition
 * douce depuis la forme précédente au début de chaque segment ; repos avant et après.
 */
export function mouthAt(segments: Segment[], t: number): MouthShape {
  if (segments.length === 0 || t < 0) return SHAPES.rest;
  const end = duration(segments);
  if (t >= end) {
    const last = SHAPES[segments[segments.length - 1].viseme];
    return lerpShape(last, SHAPES.rest, ease((t - end) / TRANSITION_S));
  }
  let index = segments.findIndex((segment) => t < segment.start + segment.duration);
  if (index < 0) index = segments.length - 1;
  const segment = segments[index];
  const from = index > 0 ? SHAPES[segments[index - 1].viseme] : SHAPES.rest;
  return lerpShape(from, SHAPES[segment.viseme], ease((t - segment.start) / TRANSITION_S));
}

/**
 * Petits mouvements de repos entre deux phrases (la voix n'a pas fini) : une respiration
 * à peine visible, pour que le visage ne se fige pas.
 */
export function idleMouth(t: number): MouthShape {
  return { ...SHAPES.rest, open: SHAPES.rest.open + 0.05 * (1 + Math.sin(t * 2.4)) };
}
