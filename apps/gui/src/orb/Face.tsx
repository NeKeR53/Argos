// Visage de l'Assistant quand il parle : deux yeux qui clignent et une bouche qui articule
// le texte que `say` prononce (synchronisation labiale procédurale, voir `mouth.ts`).
// Entre deux phrases, la bouche respire à peine ; à la fin du discours, elle se ferme.

import { useEffect, useRef, useState } from "react";
import { idleMouth, mouthAt, SHAPES, timeline, type MouthShape, type Segment } from "./mouth";

/** Phrase en cours : son texte, et le moment (performance.now) où `say` l'a lancée. */
export interface Utterance {
  text: string;
  startedAt: number;
}

interface Props {
  utterance: Utterance | null;
  /** Le discours n'est pas fini (entre `SpeechStarted` et `SpeechFinished`). */
  speaking: boolean;
  still?: boolean;
  /** Pour la démo : figer la bouche à cet instant de la phrase (s). */
  frozenAt?: number;
}

const CX = 32;
const CY = 27;

/** Contour des lèvres pour une forme : une courbe en haut, une en bas. */
export function lipsPath(shape: MouthShape): string {
  const halfWidth = 15 * shape.width * (1 - 0.32 * shape.round);
  const upper = 0.6 + shape.open * 3.6 + shape.round * 2.4;
  const lower = 0.8 + shape.open * 10.5 + shape.round * 2.6;
  const left = (CX - halfWidth).toFixed(2);
  const right = (CX + halfWidth).toFixed(2);
  return (
    `M ${left} ${CY} ` +
    `Q ${CX} ${(CY - upper).toFixed(2)} ${right} ${CY} ` +
    `Q ${CX} ${(CY + lower).toFixed(2)} ${left} ${CY} Z`
  );
}

export function Face({ utterance, speaking, still = false, frozenAt }: Props) {
  const [shape, setShape] = useState<MouthShape>(SHAPES.rest);
  const [blink, setBlink] = useState(false);
  const segments = useRef<Segment[]>([]);
  const current = useRef(utterance);
  current.current = utterance;
  const live = useRef(speaking);
  live.current = speaking;

  useEffect(() => {
    segments.current = utterance ? timeline(utterance.text) : [];
  }, [utterance]);

  // La bouche, à chaque image.
  useEffect(() => {
    if (frozenAt !== undefined) {
      setShape(mouthAt(segments.current, frozenAt));
      return;
    }
    let frame = 0;
    const tick = (now: number) => {
      const said = current.current;
      if (!live.current) {
        setShape(SHAPES.closed);
      } else if (said) {
        const t = (now - said.startedAt) / 1000;
        const end = segments.current.length
          ? segments.current[segments.current.length - 1].start + segments.current[segments.current.length - 1].duration
          : 0;
        setShape(t <= end + 0.08 || still ? mouthAt(segments.current, t) : idleMouth(t));
      } else {
        setShape(still ? SHAPES.rest : idleMouth(now / 1000));
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [still, frozenAt, utterance]);

  // Un clignement toutes les 3 à 5 s.
  useEffect(() => {
    if (still) return;
    let timer: ReturnType<typeof setTimeout>;
    const schedule = () => {
      timer = setTimeout(
        () => {
          setBlink(true);
          timer = setTimeout(() => {
            setBlink(false);
            schedule();
          }, 120);
        },
        3000 + Math.random() * 2000,
      );
    };
    schedule();
    return () => clearTimeout(timer);
  }, [still]);

  const eye = blink ? 0.25 : 1;
  return (
    <svg className="orb-face" viewBox="0 0 64 40" width={64} height={40} aria-hidden>
      <ellipse cx={22} cy={13} rx={2.3} ry={2.6 * eye} className="orb-eye" />
      <ellipse cx={42} cy={13} rx={2.3} ry={2.6 * eye} className="orb-eye" />
      <path d={lipsPath(shape)} className="orb-mouth" />
    </svg>
  );
}
