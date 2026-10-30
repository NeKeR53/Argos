// La pieuvre : le visage de l'Assistant, dans la console comme dans l'orbe. Elle respire,
// ses tentacules ondulent, elle te regarde (et suit le pointeur si on le lui demande), et
// son attitude dit ce qu'elle fait : elle écoute, réfléchit, parle ou dort. La posture
// vient de `octopus/pose.ts` ; ici, on ne fait que la tracer à chaque image.

import { useEffect, useId, useRef, useState } from "react";
import type { MouthShape } from "../orb/mouth";
import { mixPose, pose, type Mouth, type OctopusMode, type Pose, type Pt } from "../octopus/pose";

interface Props {
  size?: number;
  mode?: OctopusMode;
  /** Niveau de la voix (écoute), 0..1. */
  level?: number;
  /** Forme de la bouche à la parole. */
  mouth?: MouthShape;
  /** Animations coupées (préférence système) : une pose figée. */
  still?: boolean;
  /** Les yeux suivent le pointeur quand il bouge. */
  followPointer?: boolean;
  /** Ventouses et sourcils : à partir d'une certaine taille seulement. */
  detail?: boolean;
  /**
   * Cadrage : `full` montre toute la pieuvre, `portrait` serre sur le visage (petites
   * tailles, où un corps entier rendrait les yeux minuscules).
   */
  framing?: "full" | "portrait";
  className?: string;
}

/** Durée du fondu d'une attitude à l'autre (s). */
const BLEND_S = 0.4;
/** Le pointeur compte tant qu'il a bougé il y a moins de ça (ms). */
const POINTER_MS = 2500;

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

// Dernière position du pointeur, partagée par toutes les pieuvres de la page.
const pointer = { x: 0, y: 0, at: -Infinity };
let listening = false;
function trackPointer() {
  if (listening || typeof window === "undefined") return;
  listening = true;
  window.addEventListener(
    "pointermove",
    (event) => {
      pointer.x = event.clientX;
      pointer.y = event.clientY;
      pointer.at = performance.now();
    },
    { passive: true },
  );
}

/** Temps de l'animation (s), à ~30 images par seconde ; 0 quand elle est figée. */
function useClock(still: boolean): number {
  const [t, setT] = useState(0);
  useEffect(() => {
    if (still) {
      setT(0);
      return;
    }
    let frame = 0;
    let last = 0;
    const origin = performance.now();
    const tick = (now: number) => {
      if (now - last >= 32) {
        last = now;
        setT((now - origin) / 1000);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [still]);
  return t;
}

/** Contour fermé lissé : on passe par le milieu des côtés, les sommets servent de contrôle. */
function smoothPath(points: Pt[]): string {
  const n = points.length;
  const mid = (a: Pt, b: Pt) => `${((a[0] + b[0]) / 2).toFixed(2)} ${((a[1] + b[1]) / 2).toFixed(2)}`;
  let d = `M ${mid(points[n - 1], points[0])}`;
  for (let i = 0; i < n; i++) {
    const p = points[i];
    d += ` Q ${p[0].toFixed(2)} ${p[1].toFixed(2)} ${mid(p, points[(i + 1) % n])}`;
  }
  return `${d} Z`;
}

function mouthPath(m: Mouth): { d: string; open: boolean } {
  const l = `${(m.x - m.half).toFixed(2)} ${m.y.toFixed(2)}`;
  const r = `${(m.x + m.half).toFixed(2)} ${m.y.toFixed(2)}`;
  if (m.upper + m.lower < 0.9) {
    return { d: `M ${l} Q ${m.x.toFixed(2)} ${(m.y + m.curve).toFixed(2)} ${r}`, open: false };
  }
  return {
    d:
      `M ${l} Q ${m.x.toFixed(2)} ${(m.y - m.upper).toFixed(2)} ${r} ` +
      `Q ${m.x.toFixed(2)} ${(m.y + m.lower).toFixed(2)} ${l} Z`,
    open: true,
  };
}

export function Octopus({
  size = 40,
  mode = "idle",
  level = 0,
  mouth,
  still: stillProp,
  followPointer = false,
  detail = size >= 30,
  framing = size < 48 ? "portrait" : "full",
  className,
}: Props) {
  const still = stillProp ?? reducedMotion();
  const t = useClock(still);
  const id = useId().replace(/:/g, "");
  const svg = useRef<SVGSVGElement>(null);

  // Fondu entre l'ancienne attitude et la nouvelle.
  const previous = useRef<{ mode: OctopusMode; at: number }>({ mode, at: -Infinity });
  const current = useRef(mode);
  if (current.current !== mode) {
    previous.current = { mode: current.current, at: t };
    current.current = mode;
  }

  useEffect(() => {
    if (followPointer) trackPointer();
  }, [followPointer]);

  let look: Pt | null = null;
  if (followPointer && !still && svg.current && performance.now() - pointer.at < POINTER_MS) {
    const box = svg.current.getBoundingClientRect();
    const dx = pointer.x - (box.left + box.width / 2);
    const dy = pointer.y - (box.top + box.height * 0.4);
    const dist = Math.hypot(dx, dy) || 1;
    const reach = Math.min(1, dist / 140);
    look = [(dx / dist) * reach, (dy / dist) * reach];
  }

  const input = { t, level, mouth, look };
  let p: Pose = pose({ ...input, mode });
  const k = (t - previous.current.at) / BLEND_S;
  if (k < 1) {
    const eased = k * k * (3 - 2 * k);
    p = mixPose(pose({ ...input, mode: previous.current.mode }), p, eased);
  }

  const lips = mouthPath(p.mouth);
  const skin = `url(#${id}-skin)`;

  return (
    <svg
      ref={svg}
      className={className ? `octopus ${className}` : "octopus"}
      data-mode={mode}
      viewBox={framing === "portrait" ? "8 2 48 48" : "0 0 64 64"}
      width={size}
      height={size}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`${id}-skin`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--octopus-top, #b4a8ff)" />
          <stop offset="1" stopColor="var(--octopus-bottom, #7563ee)" />
        </linearGradient>
        {p.eyes.map((e, i) => (
          <clipPath key={i} id={`${id}-eye${i}`}>
            <ellipse cx={e.cx} cy={e.cy + e.ry * (1 - e.open) * 0.4} rx={e.rx} ry={Math.max(0.01, e.ry * e.open)} />
          </clipPath>
        ))}
      </defs>

      <g className="octopus-body">
        {p.back.map((arm, i) => (
          <path key={`b${i}`} d={smoothPath(arm)} fill="var(--octopus-back, #5f4fd6)" />
        ))}
        {p.front.map((arm, i) => (
          <path key={`f${i}`} d={smoothPath(arm)} fill={skin} />
        ))}
        {detail &&
          p.suckers.map((s, i) => (
            <circle key={`s${i}`} cx={s.x} cy={s.y} r={s.r} fill="#efeaff" opacity={s.alpha} />
          ))}
        <path d={smoothPath(p.head)} fill={skin} />
        {/* Reflet sur la tête */}
        <ellipse
          cx={p.head[42][0] + 3}
          cy={p.head[42][1] + 5.5}
          rx={6}
          ry={3}
          fill="#fff"
          opacity={0.3}
          transform={`rotate(-24 ${p.head[42][0] + 3} ${p.head[42][1] + 5.5})`}
        />
        {p.over.map((arm, i) => (
          <path key={`o${i}`} d={smoothPath(arm)} fill={skin} stroke="#5f4fd6" strokeWidth={0.5} strokeOpacity={0.5} />
        ))}
      </g>

      {p.cheeks.map((c, i) => (
        <ellipse key={`c${i}`} cx={c.x} cy={c.y} rx={c.r} ry={c.r * 0.6} fill="#ff8fb3" opacity={c.alpha} />
      ))}

      {p.eyes.map((e, i) =>
        e.open < 0.18 ? (
          // Paupière fermée : un trait courbe.
          <path
            key={`e${i}`}
            d={`M ${e.cx - e.rx * 0.85} ${e.cy} Q ${e.cx} ${e.cy + 2.6} ${e.cx + e.rx * 0.85} ${e.cy}`}
            fill="none"
            stroke="#1d1a33"
            strokeWidth={1.5}
            strokeLinecap="round"
          />
        ) : (
          <g key={`e${i}`} clipPath={`url(#${id}-eye${i})`}>
            <ellipse cx={e.cx} cy={e.cy} rx={e.rx} ry={e.ry} fill="#fdfcff" />
            <circle cx={e.px} cy={e.py} r={e.pr} fill="#1d1a33" />
            <circle cx={e.px + e.pr * 0.38} cy={e.py - e.pr * 0.4} r={e.pr * 0.36} fill="#fff" />
            <circle cx={e.px - e.pr * 0.35} cy={e.py + e.pr * 0.45} r={e.pr * 0.14} fill="#fff" opacity={0.8} />
          </g>
        ),
      )}

      {detail &&
        p.browAlpha > 0.01 &&
        p.brows.map(([a, c, b], i) => (
          <path
            key={`w${i}`}
            d={`M ${a[0]} ${a[1]} Q ${c[0]} ${c[1]} ${b[0]} ${b[1]}`}
            fill="none"
            stroke="#3b2f7a"
            strokeWidth={1.3}
            strokeLinecap="round"
            opacity={p.browAlpha}
          />
        ))}

      <path
        d={lips.d}
        fill={lips.open ? "#3a1f3f" : "none"}
        stroke="#3a1f3f"
        strokeWidth={lips.open ? 0.6 : 1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {lips.open && p.mouth.lower > 3 && (
        // La langue, quand la bouche s'ouvre grand.
        <ellipse
          cx={p.mouth.x}
          cy={p.mouth.y + p.mouth.lower * 0.55}
          rx={p.mouth.half * 0.45}
          ry={p.mouth.lower * 0.2}
          fill="#e86b8f"
        />
      )}

      {p.bubbles.map(
        (b, i) =>
          b.alpha > 0.02 &&
          (mode === "offline" ? (
            <text
              key={`z${i}`}
              x={b.x}
              y={b.y}
              fontSize={3 + b.r * 1.6}
              fontWeight={700}
              fill="var(--octopus-z, #9aa0c0)"
              opacity={b.alpha}
            >
              z
            </text>
          ) : (
            <circle
              key={`u${i}`}
              cx={b.x}
              cy={b.y}
              r={b.r}
              fill="#e8e4ff"
              fillOpacity={0.35}
              stroke="#b4a8ff"
              strokeWidth={0.6}
              opacity={b.alpha}
            />
          )),
      )}
    </svg>
  );
}
