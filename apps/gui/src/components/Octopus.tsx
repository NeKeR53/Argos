// La pieuvre : le visage de l'Assistant, dans la console comme dans l'orbe. Elle cligne
// des yeux et, quand l'Assistant parle, sa bouche articule (formes de `orb/mouth.ts`).

import type { MouthShape } from "../orb/mouth";

interface Props {
  size?: number;
  /** Forme de la bouche ; sans elle, un petit sourire au repos. */
  mouth?: MouthShape;
  /** Yeux fermés (clignement). */
  blink?: boolean;
  className?: string;
}

const MX = 32;
const MY = 36.5;

/** Bouche : petit sourire au repos, lèvres qui s'ouvrent quand elle parle. */
function mouthPath(shape: MouthShape | undefined): string {
  if (!shape) return `M ${MX - 3.6} ${MY - 0.6} Q ${MX} ${MY + 3} ${MX + 3.6} ${MY - 0.6}`;
  const half = 5 * shape.width * (1 - 0.3 * shape.round);
  const upper = 0.3 + shape.open * 1.4 + shape.round * 1;
  const lower = 0.5 + shape.open * 4.4 + shape.round * 1.2;
  return (
    `M ${(MX - half).toFixed(2)} ${MY} ` +
    `Q ${MX} ${(MY - upper).toFixed(2)} ${(MX + half).toFixed(2)} ${MY} ` +
    `Q ${MX} ${(MY + lower).toFixed(2)} ${(MX - half).toFixed(2)} ${MY} Z`
  );
}

export function Octopus({ size = 40, mouth, blink = false, className }: Props) {
  const eye = blink ? 0.2 : 1;
  return (
    <svg
      className={className ? `octopus ${className}` : "octopus"}
      viewBox="0 0 64 64"
      width={size}
      height={size}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id="octopus-skin" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#a99bff" />
          <stop offset="1" stopColor="#7d6cf2" />
        </linearGradient>
      </defs>
      <g fill="url(#octopus-skin)">
        {/* Tentacules, enroulés au bout. */}
        <path d="M17 40 C12 46 10 52 14 56 C17 59 21 56 19 53 C17 51 19 47 22 44 Z" />
        <path d="M24 43 C22 49 22 55 26 58 C29 60 31 57 29 55 C27 53 28 49 29 45 Z" />
        <path d="M40 43 C42 49 42 55 38 58 C35 60 33 57 35 55 C37 53 36 49 35 45 Z" />
        <path d="M47 40 C52 46 54 52 50 56 C47 59 43 56 45 53 C47 51 45 47 42 44 Z" />
        <path d="M30 44 C30 50 31 55 33 58 C35 55 35 50 34 44 Z" />
        {/* Tête */}
        <path d="M32 6 C46 6 54 16 54 28 C54 38 46 45 32 45 C18 45 10 38 10 28 C10 16 18 6 32 6 Z" />
      </g>
      {/* Reflet sur la tête */}
      <ellipse cx={24} cy={15} rx={6} ry={3.2} fill="#fff" opacity={0.28} transform="rotate(-22 24 15)" />
      {/* Joues */}
      <ellipse cx={20.5} cy={34} rx={3.6} ry={2.2} fill="#ff8fb3" opacity={0.55} />
      <ellipse cx={43.5} cy={34} rx={3.6} ry={2.2} fill="#ff8fb3" opacity={0.55} />
      {/* Yeux */}
      <g>
        <ellipse cx={25} cy={28} rx={3.4} ry={3.8 * eye} fill="#1d1a33" />
        <ellipse cx={39} cy={28} rx={3.4} ry={3.8 * eye} fill="#1d1a33" />
        {!blink && (
          <>
            <circle cx={26.2} cy={26.6} r={1.2} fill="#fff" />
            <circle cx={40.2} cy={26.6} r={1.2} fill="#fff" />
          </>
        )}
      </g>
      <path
        d={mouthPath(mouth)}
        fill={mouth ? "#3a1f3f" : "none"}
        stroke="#3a1f3f"
        strokeWidth={mouth ? 0.6 : 1.6}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
