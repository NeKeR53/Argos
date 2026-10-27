// Pastille de l'orbe (D38) : discrète au repos, elle grandit en douceur quand il se passe
// quelque chose. Composant de présentation : tout vient des props, pour que la page de
// démo (`orb.html?demo=…`) le montre sans daemon ni voix.

import { useState } from "react";
import { Face, type Utterance } from "./Face";
import type { OrbMode } from "./logic";
import { Waves } from "./Waves";

export interface PillView {
  mode: OrbMode;
  /** Niveau du micro (écoute), 0..1. */
  level: number;
  /** Phrase que `say` prononce (parole). */
  utterance: Utterance | null;
  /** Transcription partielle pendant l'écoute, ou dernière légende. */
  caption: string | null;
  /** Premier appui du panic stop, en attente du second. */
  armed: boolean;
  /** Le panic stop vient de partir. */
  stopped: boolean;
  /** Voix indisponible : la raison. */
  mute: string | null;
  /** Escalade en attente (D12). */
  escalation: boolean;
  still: boolean;
  /** Démo : bouche figée à cet instant de la phrase. */
  frozenAt?: number;
}

export interface Size {
  width: number;
  height: number;
}

/** Taille de la pastille selon ce qu'elle montre (points). */
export function pillSize(view: Pick<PillView, "mode" | "armed" | "stopped" | "caption" | "escalation">): Size {
  if (view.armed) return { width: 236, height: 38 };
  if (view.stopped) return { width: 160, height: 36 };
  switch (view.mode) {
    case "listening":
      return view.caption ? { width: 280, height: 44 } : { width: 196, height: 44 };
    case "speaking":
      return { width: 148, height: 52 };
    default:
      // Une légende (« Rien entendu », voix indisponible…) prend la place du motif.
      if (view.caption) return { width: 260, height: 36 };
      if (view.mode === "thinking") return { width: 148, height: 36 };
      return { width: view.escalation ? 136 : 120, height: 36 };
  }
}

/** Marge autour de la pastille dans la fenêtre : de quoi grossir avec la voix. */
export const PILL_MARGIN = 4;

const statusLabel: Record<OrbMode, string> = {
  idle: "Argos au repos",
  listening: "Argos écoute",
  thinking: "Argos réfléchit",
  speaking: "Argos parle",
  offline: "Daemon injoignable",
};

export function Pill(view: PillView) {
  const size = pillSize(view);
  const [swell, setSwell] = useState(0);
  const listening = view.mode === "listening" && !view.armed && !view.stopped;
  // La pastille grossit un peu avec la voix (jamais sans animation permise).
  const scale = listening && !view.still ? 1 + 0.06 * swell : 1;
  const title = [statusLabel[view.mode], view.escalation ? "escalade en attente" : null, view.mute]
    .filter(Boolean)
    .join(" · ");

  return (
    <div
      className="pill"
      data-mode={view.mode}
      data-armed={view.armed || undefined}
      data-stopped={view.stopped || undefined}
      data-escalation={view.escalation || undefined}
      style={{ width: size.width, height: size.height, transform: `scale(${scale})` }}
      title={title}
    >
      {view.armed ? (
        <span className="pill-alert">Encore une fois : tout arrêter</span>
      ) : view.stopped ? (
        <span className="pill-alert">Arbre arrêté</span>
      ) : (
        <>
          <span className="pill-status" aria-hidden />
          {listening && (
            <>
              <Waves level={view.level} width={130} height={26} still={view.still} onLevel={setSwell} />
              {view.caption && <span className="pill-caption">{view.caption}</span>}
            </>
          )}
          {view.mode === "speaking" && (
            <Face utterance={view.utterance} speaking still={view.still} frozenAt={view.frozenAt} />
          )}
          {!listening && view.mode !== "speaking" && view.caption && (
            <span className="pill-caption">{view.caption}</span>
          )}
          {view.mode === "thinking" && !view.caption && (
            <span className="pill-thinking" aria-hidden>
              <i />
              <i />
              <i />
            </span>
          )}
          {(view.mode === "idle" || view.mode === "offline") && !view.caption && (
            <span className="pill-glyph" aria-hidden>
              <i />
              <i />
              <i />
              <i />
              <i />
            </span>
          )}
          {view.escalation && view.mode === "idle" && <span className="pill-badge">!</span>}
          {view.mute && (view.mode === "idle" || view.mode === "offline") && <MicOff />}
        </>
      )}
    </div>
  );
}

/** Micro barré : voix indisponible (la raison est dans l'infobulle). */
function MicOff() {
  return (
    <svg className="pill-mute" viewBox="0 0 16 16" width={13} height={13} aria-hidden>
      <rect x={5.5} y={2} width={5} height={8} rx={2.5} />
      <path d="M3.5 7.5a4.5 4.5 0 0 0 9 0M8 12v2" fill="none" />
      <path d="M2.5 2.5l11 11" className="pill-mute-bar" />
    </svg>
  );
}
