// Démo de la pieuvre (`orb.html?demo=octopus`), sans daemon ni voix : chaque attitude en
// grand, puis aux tailles réelles (avatar, orbe), et une scène où elle enchaîne les états
// comme pendant un vrai échange.

import { useEffect, useState } from "react";
import { Octopus } from "../components/Octopus";
import { idleMouth, mouthAt, timeline, type MouthShape } from "../orb/mouth";
import type { OctopusMode } from "./pose";
import "./octopus-demo.css";

const SENTENCE = "Bonjour ! J'ai rangé ta boîte mail, et il reste deux réponses à valider.";

const MODES: { mode: OctopusMode; label: string; hint: string }[] = [
  { mode: "idle", label: "Repos", hint: "respire, regarde autour puis revient vers toi" },
  { mode: "listening", label: "Écoute", hint: "se penche, sourcils levés, réagit au niveau du micro" },
  { mode: "thinking", label: "Réflexion", hint: "les yeux en l'air, un bras sous le menton, des bulles" },
  { mode: "speaking", label: "Parole", hint: "articule la phrase, gesticule, te regarde" },
  { mode: "offline", label: "Hors ligne", hint: "endormie" },
];

/** Scène : écoute → réflexion → parole → repos, en boucle. */
const SCENE: { mode: OctopusMode; seconds: number; caption: string }[] = [
  { mode: "idle", seconds: 2.5, caption: "…" },
  { mode: "listening", seconds: 3, caption: "« Range mes mails et dis-moi ce qui reste »" },
  { mode: "thinking", seconds: 3, caption: "Elle réfléchit" },
  { mode: "speaking", seconds: 5, caption: SENTENCE },
  { mode: "idle", seconds: 2.5, caption: "…" },
];
const SCENE_S = SCENE.reduce((sum, step) => sum + step.seconds, 0);

/** Niveau de micro synthétique : des syllabes qui montent et retombent. */
function syntheticLevel(t: number): number {
  const syllables = Math.max(0, Math.sin(t * 8.5)) ** 0.7;
  return Math.min(1, syllables * (0.55 + 0.45 * Math.sin(t * 1.1)));
}

const segments = timeline(SENTENCE);
const sentenceEnd = segments.length ? segments[segments.length - 1].start + segments[segments.length - 1].duration : 0;

/** Bouche en boucle sur la phrase de démo (une pause de 1 s entre deux lectures). */
function speakingMouth(t: number): MouthShape {
  const local = t % (sentenceEnd + 1);
  return local <= sentenceEnd ? mouthAt(segments, local) : idleMouth(t);
}

export function OctopusDemo() {
  const [now, setNow] = useState(0);
  useEffect(() => {
    let frame = 0;
    const origin = performance.now();
    const tick = (time: number) => {
      setNow((time - origin) / 1000);
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, []);

  const level = syntheticLevel(now);
  const props = (mode: OctopusMode) => ({
    mode,
    level: mode === "listening" ? level : 0,
    mouth: mode === "speaking" ? speakingMouth(now) : undefined,
  });

  // Où en est la scène.
  let at = now % SCENE_S;
  let step = SCENE[0];
  for (const s of SCENE) {
    if (at < s.seconds) {
      step = s;
      break;
    }
    at -= s.seconds;
  }
  const sceneMouth =
    step.mode === "speaking" ? (at <= sentenceEnd ? mouthAt(segments, at) : idleMouth(now)) : undefined;

  return (
    <div className="octo-demo">
      <header>
        <h1>La pieuvre</h1>
        <p>Déplace la souris : elle te suit des yeux.</p>
      </header>

      <section className="octo-scene">
        <div className="octo-stage">
          <Octopus
            size={260}
            mode={step.mode}
            level={step.mode === "listening" ? level : 0}
            mouth={sceneMouth}
            followPointer
          />
        </div>
        <div className="octo-bubble" data-mode={step.mode}>
          <span className="octo-tag">{MODES.find((m) => m.mode === step.mode)?.label}</span>
          <p>{step.caption}</p>
        </div>
      </section>

      <section className="octo-grid">
        {MODES.map(({ mode, label, hint }) => (
          <figure key={mode}>
            <Octopus size={150} {...props(mode)} followPointer />
            <figcaption>
              <strong>{label}</strong>
              <span>{hint}</span>
            </figcaption>
          </figure>
        ))}
      </section>

      <section className="octo-sizes">
        <h2>Aux tailles réelles</h2>
        <div className="octo-row">
          {MODES.map(({ mode, label }) => (
            <div key={mode} className="octo-chip">
              <span className="octo-avatar">
                <Octopus size={36} {...props(mode)} />
              </span>
              <span className="octo-pill">
                <Octopus size={30} {...props(mode)} />
              </span>
              <span className="octo-dock">
                <Octopus size={64} {...props(mode)} />
              </span>
              <small>{label}</small>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
