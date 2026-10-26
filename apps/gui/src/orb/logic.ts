// Logique pure de l'orbe (D38) : état affiché et double appui du panic stop.

/** Ce que fait la voix (posé par le cœur Rust, événement `voice-state`). */
export type VoiceMode = "listening" | "speaking";

export interface VoiceState {
  /** `null` : la voix ne dit rien, l'orbe suit l'état de l'Assistant. */
  state: VoiceMode | null;
  /** Niveau sonore, 0 à 1 : module l'écoute et la parole. */
  level: number;
}

/** Les quatre états de D38, plus « hors ligne » quand le daemon ne répond pas. */
export type OrbMode = "idle" | "listening" | "thinking" | "speaking" | "offline";

export const SILENT: VoiceState = { state: null, level: 0 };

/**
 * La voix a la main quand elle écoute ou parle ; sinon réflexion pendant un tour de
 * l'Assistant, repos le reste du temps.
 */
export function orbMode(connected: boolean, assistantBusy: boolean, voice: VoiceState): OrbMode {
  if (voice.state) return voice.state;
  if (!connected) return "offline";
  return assistantBusy ? "thinking" : "idle";
}

/** Niveau ramené à 0..1 (un NaN vaut 0). */
export function clampLevel(level: number): number {
  return Number.isFinite(level) ? Math.min(1, Math.max(0, level)) : 0;
}

/** Lecture tolérante de la charge de `voice-state`. */
export function readVoiceState(payload: unknown): VoiceState {
  if (typeof payload !== "object" || payload === null) return SILENT;
  const { state, level } = payload as Record<string, unknown>;
  const mode = state === "listening" || state === "speaking" ? state : null;
  return { state: mode, level: clampLevel(typeof level === "number" ? level : 0) };
}

/** Délai entre les deux appuis du panic stop global. */
export const PANIC_WINDOW_MS = 2000;

export interface PanicPress {
  /** Second appui à temps : l'arrêt part. */
  fire: boolean;
  /** Moment du premier appui en attente de confirmation, `null` sinon. */
  armedAt: number | null;
}

/**
 * Un appui sur le raccourci du panic stop : le premier arme (confirmation visible sur
 * l'orbe), un second dans les 2 s déclenche l'arrêt ; trop tard, il réarme.
 */
export function pressPanic(armedAt: number | null, now: number): PanicPress {
  if (armedAt !== null && now - armedAt <= PANIC_WINDOW_MS) return { fire: true, armedAt: null };
  return { fire: false, armedAt: now };
}
