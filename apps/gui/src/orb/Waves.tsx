// Ondes de l'écoute : des barres qui ondulent avec la voix de l'utilisateur, dessinées en
// canvas à la cadence de l'écran. Le niveau reçu (0..1) est lissé à chaque image : attaque
// rapide, retombée douce. Sans animation (prefers-reduced-motion), les barres suivent le
// niveau sans onduler.

import { useEffect, useRef } from "react";
import { barHeight, smoothLevel } from "./level";

interface Props {
  /** Niveau du micro, 0..1 (déjà étalé par le cœur Rust). */
  level: number;
  width: number;
  height: number;
  bars?: number;
  still?: boolean;
  /** Niveau lissé à chaque image, pour faire grossir la pastille avec la voix. */
  onLevel?: (smoothed: number) => void;
}

export function Waves({ level, width, height, bars = 22, still = false, onLevel }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const target = useRef(level);
  target.current = level;
  const report = useRef(onLevel);
  report.current = onLevel;

  useEffect(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context) return;
    const ratio = window.devicePixelRatio || 1;
    element.width = Math.round(width * ratio);
    element.height = Math.round(height * ratio);
    context.scale(ratio, ratio);

    const gradient = context.createLinearGradient(0, 0, width, 0);
    gradient.addColorStop(0, "#5fd4b0");
    gradient.addColorStop(0.5, "#8ee8ff");
    gradient.addColorStop(1, "#7f95ff");

    let smoothed = 0;
    let last = performance.now();
    let frame = 0;
    const gap = width / bars;
    const barWidth = Math.max(2, gap * 0.48);

    const draw = (now: number) => {
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      smoothed = smoothLevel(smoothed, target.current, dt);
      report.current?.(smoothed);
      context.clearRect(0, 0, width, height);
      context.fillStyle = gradient;
      const t = now / 1000;
      for (let i = 0; i < bars; i++) {
        const h = Math.max(barWidth, barHeight(i, bars, smoothed, t, still) * height);
        const x = i * gap + (gap - barWidth) / 2;
        const y = (height - h) / 2;
        context.beginPath();
        context.roundRect(x, y, barWidth, h, barWidth / 2);
        context.fill();
      }
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, [width, height, bars, still]);

  return <canvas ref={canvas} className="orb-waves" style={{ width, height }} aria-hidden />;
}
