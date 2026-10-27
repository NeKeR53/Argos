// Démo de l'orbe dans un navigateur (`orb.html?demo=…`), sans daemon ni voix : pour voir
// et vérifier chaque état, la fenêtre Tauri ne se laissant pas capturer.
//   ?demo=all                tous les états
//   ?demo=idle|escalation|mute|offline|thinking|listening|speaking|panic|stopped
//   &t=0.42                  bouche figée à cet instant de la phrase (parole)

import { useEffect, useState } from "react";
import type { Utterance } from "./Face";
import { Pill, type PillView } from "./Pill";
import "./orb.css";

const SENTENCE = "Bonjour, je suis là si vous avez besoin de quelque chose.";

const BASE: PillView = {
  mode: "idle",
  level: 0,
  utterance: null,
  caption: null,
  armed: false,
  stopped: false,
  mute: null,
  escalation: false,
  still: false,
};

const STATES: Record<string, Partial<PillView>> = {
  idle: {},
  escalation: { escalation: true },
  mute: { mute: "pas de section [voice] dans config.toml : voix désactivée" },
  offline: { mode: "offline" },
  thinking: { mode: "thinking" },
  listening: { mode: "listening", caption: "Allume la lumière du salon" },
  speaking: { mode: "speaking" },
  panic: { armed: true },
  stopped: { stopped: true },
};

/** Niveau de micro synthétique : des syllabes qui montent et retombent. */
function syntheticLevel(t: number): number {
  const syllables = Math.max(0, Math.sin(t * 8.5)) ** 0.7;
  const phrase = 0.55 + 0.45 * Math.sin(t * 1.1);
  const jitter = 0.85 + 0.15 * Math.sin(t * 23.7);
  return Math.min(1, syllables * phrase * jitter);
}

export function OrbDemo({ demo, frozenAt }: { demo: string; frozenAt?: number }) {
  const [level, setLevel] = useState(0);
  const [utterance, setUtterance] = useState<Utterance>({ text: SENTENCE, startedAt: performance.now() });

  useEffect(() => {
    let frame = 0;
    const tick = (now: number) => {
      setLevel(syntheticLevel(now / 1000));
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    // La phrase reprend toutes les 5 s, comme une suite de réponses.
    const timer = setInterval(() => setUtterance({ text: SENTENCE, startedAt: performance.now() }), 5000);
    return () => {
      cancelAnimationFrame(frame);
      clearInterval(timer);
    };
  }, []);

  const names = demo === "all" ? Object.keys(STATES) : [demo in STATES ? demo : "idle"];
  return (
    <div className="orb-demo" data-gallery={demo === "all" || undefined}>
      {names.map((name) => (
        <div key={name} className="orb-demo-cell">
          <Pill
            {...BASE}
            {...STATES[name]}
            level={level}
            utterance={utterance}
            frozenAt={frozenAt}
            still={window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false}
          />
          {demo === "all" && <span className="orb-demo-label">{name}</span>}
        </div>
      ))}
    </div>
  );
}
