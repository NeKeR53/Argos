// Lissage du niveau du micro pour les ondes de l'orbe : attaque rapide (la voix se voit
// tout de suite), retombée douce (pas de scintillement entre deux syllabes).

/** Constante de temps de la montée (s). */
export const ATTACK_S = 0.04;
/** Constante de temps de la descente (s). */
export const RELEASE_S = 0.28;

/**
 * Un pas de lissage exponentiel de `current` vers `target` après `dt` secondes, avec une
 * constante de temps différente selon que le niveau monte ou descend. Résultat dans 0..1.
 */
export function smoothLevel(current: number, target: number, dt: number): number {
  const goal = clamp01(target);
  if (!(dt > 0)) return clamp01(current);
  const tau = goal > current ? ATTACK_S : RELEASE_S;
  const alpha = 1 - Math.exp(-dt / tau);
  return clamp01(current + (goal - current) * alpha);
}

export function clamp01(value: number): number {
  return Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0;
}

/**
 * Hauteur (0..1) de la barre `index` sur `count` à l'instant `t` (s) pour un niveau lissé :
 * une enveloppe en cloche (plus haut au centre), une ondulation lente propre à chaque
 * barre, et un plancher pour que les ondes restent visibles dans le silence.
 */
export function barHeight(index: number, count: number, level: number, t: number, still = false): number {
  const position = count > 1 ? index / (count - 1) : 0.5;
  const envelope = Math.exp(-((position - 0.5) ** 2) / 0.08);
  const wave = still ? 1 : 0.62 + 0.38 * Math.sin(t * 7.3 + index * 0.9) * Math.sin(t * 3.1 + index * 0.37);
  return clamp01(0.1 + 0.9 * level * envelope * wave);
}
