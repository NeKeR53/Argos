//! Posture de la pieuvre pour la tuile du Dock : portage de `src/octopus/pose.ts`, pour
//! que le Dock montre la même interlocutrice que la console et l'orbe (respiration,
//! tentacules qui ondulent, regard, attitude selon l'état). Calcul pur, sans hasard.
//!
//! Grille 64×64, origine en haut à gauche ; tout est déjà placé, le rendu n'a qu'à tracer.

use crate::dock::Mode;
use std::f64::consts::PI;

pub type Pt = (f64, f64);

pub struct Eye {
    pub cx: f64,
    pub cy: f64,
    pub rx: f64,
    pub ry: f64,
    /// Ouverture de la paupière : 1 grand ouvert, 0 fermé.
    pub open: f64,
    pub px: f64,
    pub py: f64,
    pub pr: f64,
}

/// Bouche : une courbe (`curve` > 0 sourit) tant qu'elle est fermée, des lèvres sinon.
pub struct Mouth {
    pub x: f64,
    pub y: f64,
    pub half: f64,
    pub upper: f64,
    pub lower: f64,
    pub curve: f64,
}

pub struct Dot {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub alpha: f64,
}

pub struct Pose {
    pub head: Vec<Pt>,
    pub back: Vec<Vec<Pt>>,
    pub front: Vec<Vec<Pt>>,
    /// Le bras qui passe devant la tête (sous le menton, à la réflexion).
    pub over: Vec<Vec<Pt>>,
    pub suckers: Vec<Dot>,
    pub eyes: [Eye; 2],
    /// Sourcils : départ, contrôle, arrivée (courbe quadratique).
    pub brows: [[Pt; 3]; 2],
    pub brow_alpha: f64,
    pub cheeks: [Dot; 2],
    pub mouth: Mouth,
    pub bubbles: [Dot; 3],
}

/// Forme de bouche (même sens que `MouthShape` côté front).
#[derive(Clone, Copy)]
pub struct Viseme {
    pub open: f64,
    pub width: f64,
    pub round: f64,
}

pub fn hash(n: f64) -> f64 {
    let x = (n * 127.1 + 311.7).sin() * 43758.5453;
    x - x.floor()
}

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let k = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

fn lerp(a: f64, b: f64, k: f64) -> f64 {
    a + (b - a) * k
}

const BLINK_PERIOD: f64 = 3.7;
const BLINK_S: f64 = 0.17;

pub fn eye_open(t: f64) -> f64 {
    let n = (t / BLINK_PERIOD).floor();
    let at = 0.4 + hash(n) * 2.6;
    let local = t - n * BLINK_PERIOD;
    let mut closure: f64 = 0.0;
    let starts: &[f64] = if hash(n + 0.5) > 0.7 {
        &[at, at + 0.3]
    } else {
        &[at]
    };
    for start in starts {
        let dt = local - start;
        if (0.0..BLINK_S).contains(&dt) {
            closure = closure.max((PI * dt / BLINK_S).sin());
        }
    }
    1.0 - closure
}

const GAZE_PERIOD: f64 = 1.9;

fn gaze_target(mode: Mode, n: f64) -> Pt {
    let a = hash(n * 1.31) * 2.0 - 1.0;
    let b = hash(n * 2.17 + 4.0) * 2.0 - 1.0;
    match mode {
        Mode::Thinking => (-0.55 + 0.35 * a, -0.8 + 0.15 * b),
        Mode::Listening | Mode::Speaking => (0.18 * a, 0.12 * b),
        Mode::Offline => (0.0, 0.4),
        Mode::Idle => {
            if hash(n * 0.77 + 9.0) > 0.45 {
                (0.12 * a, 0.1 * b)
            } else {
                (0.85 * a, 0.55 * b)
            }
        }
    }
}

fn gaze(mode: Mode, t: f64) -> Pt {
    let n = (t / GAZE_PERIOD).floor();
    let from = gaze_target(mode, n - 1.0);
    let to = gaze_target(mode, n);
    let k = smooth(0.0, 0.2, t - n * GAZE_PERIOD);
    (lerp(from.0, to.0, k), lerp(from.1, to.1, k))
}

/// Bouche à la parole quand on n'a pas le texte : des syllabes qui s'enchaînent.
pub fn synthetic_viseme(t: f64) -> Viseme {
    let syllable = (t * 7.5).sin().abs();
    let phrase = 0.6 + 0.4 * (t * 1.3).sin();
    Viseme {
        open: syllable * phrase,
        width: 0.8 + 0.15 * (t * 3.1).sin(),
        round: ((t * 2.3).sin() - 0.6).max(0.0) * 2.0,
    }
}

struct Arm {
    dx: f64,
    angle: f64,
    length: f64,
    curl: f64,
    phase: f64,
}

const FRONT: [Arm; 6] = [
    Arm {
        dx: -14.5,
        angle: -0.95,
        length: 15.0,
        curl: -1.9,
        phase: 0.0,
    },
    Arm {
        dx: -8.5,
        angle: -0.42,
        length: 18.0,
        curl: -1.2,
        phase: 1.3,
    },
    Arm {
        dx: -2.8,
        angle: -0.1,
        length: 19.0,
        curl: 0.9,
        phase: 2.6,
    },
    Arm {
        dx: 2.8,
        angle: 0.1,
        length: 19.0,
        curl: -0.9,
        phase: 3.7,
    },
    Arm {
        dx: 8.5,
        angle: 0.42,
        length: 18.0,
        curl: 1.2,
        phase: 4.9,
    },
    Arm {
        dx: 14.5,
        angle: 0.95,
        length: 15.0,
        curl: 1.9,
        phase: 6.1,
    },
];

const BACK: [Arm; 2] = [
    Arm {
        dx: -11.0,
        angle: -0.7,
        length: 16.0,
        curl: 1.4,
        phase: 0.7,
    },
    Arm {
        dx: 11.0,
        angle: 0.7,
        length: 16.0,
        curl: -1.4,
        phase: 3.3,
    },
];

const SEGMENTS: usize = 14;

fn spine(base: Pt, angle_at: impl Fn(f64) -> f64, length: f64) -> Vec<Pt> {
    let mut points = vec![base];
    let (mut x, mut y) = base;
    let step = length / SEGMENTS as f64;
    for i in 1..=SEGMENTS {
        let a = angle_at((i as f64 - 0.5) / SEGMENTS as f64);
        x += a.sin() * step;
        y += a.cos() * step;
        points.push((x, y));
    }
    points
}

fn outline(points: &[Pt], w0: f64, w1: f64) -> Vec<Pt> {
    let last = points.len() - 1;
    let mut left = Vec::with_capacity(points.len());
    let mut right = Vec::with_capacity(points.len());
    for i in 0..=last {
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(last)];
        let len = (b.0 - a.0).hypot(b.1 - a.1).max(1e-6);
        let (nx, ny) = (-(b.1 - a.1) / len, (b.0 - a.0) / len);
        let w = lerp(w0, w1, i as f64 / last as f64) / 2.0;
        left.push((points[i].0 + nx * w, points[i].1 + ny * w));
        right.push((points[i].0 - nx * w, points[i].1 - ny * w));
    }
    let tip = points[last];
    let start = (left[last].1 - tip.1).atan2(left[last].0 - tip.0);
    let mut out = left;
    for k in 1..6 {
        let a = start - PI * k as f64 / 6.0;
        out.push((tip.0 + a.cos() * w1 / 2.0, tip.1 + a.sin() * w1 / 2.0));
    }
    out.extend(right.into_iter().rev());
    out
}

const CX: f64 = 32.0;
const CY: f64 = 24.0;
const RX: f64 = 19.5;
const RY: f64 = 17.5;

/// Posture à l'instant `t` ; `level` : niveau du micro à l'écoute ; `mouth` : bouche à la parole.
pub fn pose(mode: Mode, t: f64, level: f64, mouth: Option<Viseme>) -> Pose {
    let lv = level.clamp(0.0, 1.0);
    let talk = match (mode, mouth) {
        (Mode::Speaking, Some(m)) => m.open,
        _ => 0.0,
    };
    let listening = matches!(mode, Mode::Listening);

    let rate = match mode {
        Mode::Offline => 0.9,
        Mode::Thinking => 1.4,
        _ => 1.7,
    };
    let breath = (t * rate * 2.0).sin();
    let bob = breath
        * if matches!(mode, Mode::Offline) {
            0.6
        } else {
            1.1
        }
        - talk * 1.2
        - if listening { lv * 1.2 } else { 0.0 };
    let stretch = 0.025 * breath + if listening { 0.03 * lv } else { 0.0 } + 0.02 * talk;
    let tilt = match mode {
        Mode::Listening => 0.11 + 0.03 * (t * 1.3).sin(),
        Mode::Thinking => -0.07 + 0.03 * (t * 0.9).sin(),
        Mode::Speaking => 0.04 * (t * 2.3).sin(),
        Mode::Offline => 0.06,
        Mode::Idle => 0.03 * (t * 0.7).sin(),
    };

    let cy = CY + bob;
    let rx = RX * (1.0 - stretch);
    let ry = RY * (1.0 + stretch);
    let pivot = (CX, cy + RY * 0.7);
    let (sin, cos) = tilt.sin_cos();
    let turn = |(x, y): Pt| -> Pt {
        let (dx, dy) = (x - pivot.0, y - pivot.1);
        (pivot.0 + dx * cos - dy * sin, pivot.1 + dx * sin + dy * cos)
    };

    let head = (0..56)
        .map(|i| {
            let a = i as f64 / 56.0 * PI * 2.0;
            let s = a.sin();
            turn((
                CX + rx * a.cos(),
                cy + ry * s * if s < 0.0 { 1.06 } else { 0.86 },
            ))
        })
        .collect();

    let amp = match mode {
        Mode::Offline => 0.08,
        Mode::Listening => 0.16 + 0.4 * lv,
        Mode::Speaking => 0.26 + 0.3 * talk,
        _ => 0.2,
    };
    let speed = match mode {
        Mode::Offline => 0.8,
        Mode::Speaking => 3.2,
        _ => 2.1,
    };
    let base_y = |dx: f64| cy + ry * 0.86 * (1.0 - (dx / rx).powi(2)).max(0.0).sqrt() - 3.5;
    let sleepy = if matches!(mode, Mode::Offline) {
        0.6
    } else {
        1.0
    };
    let arm = |a: &Arm, gesture: f64| -> Vec<Pt> {
        let curl = a.curl * sleepy * (1.0 + 0.15 * (t * 1.1 + a.phase).sin());
        spine(
            (CX + a.dx, base_y(a.dx)),
            |s| {
                a.angle
                    + curl * s * s
                    + (amp + gesture) * (t * speed - s * 3.4 + a.phase).sin() * (0.25 + s)
            },
            a.length,
        )
        .into_iter()
        .map(turn)
        .collect()
    };

    let back = BACK
        .iter()
        .map(|a| outline(&arm(a, 0.0), 4.6, 1.2))
        .collect();
    let mut front = Vec::new();
    let mut over = Vec::new();
    let mut suckers = Vec::new();
    for (i, a) in FRONT.iter().enumerate() {
        let gesture = if matches!(mode, Mode::Speaking) && (i == 0 || i == 5) {
            0.25
        } else {
            0.0
        };
        let points: Vec<Pt> = if matches!(mode, Mode::Thinking) && i == 4 {
            let k = 0.93 + 0.07 * (t * 1.6).sin();
            let points: Vec<Pt> = spine(
                (CX + a.dx, base_y(a.dx)),
                |s| 0.3 + 4.4 * k * smooth(0.0, 0.5, s),
                14.0,
            )
            .into_iter()
            .map(turn)
            .collect();
            over.push(outline(&points, 5.2, 2.6));
            points
        } else {
            let points = arm(a, gesture);
            front.push(outline(&points, 5.4, 1.1));
            points
        };
        if matches!(i, 0 | 1 | 4 | 5) {
            for s in [0.5, 0.66, 0.8] {
                let j = (s * SEGMENTS as f64).round() as usize;
                let (x0, y0) = points[j - 1];
                let (x1, y1) = points[j + 1];
                let len = (x1 - x0).hypot(y1 - y0).max(1e-6);
                let side = if a.curl > 0.0 { -1.0 } else { 1.0 };
                let w = lerp(5.4, 1.1, s);
                suckers.push(Dot {
                    x: points[j].0 + (-(y1 - y0) / len) * side * w * 0.22,
                    y: points[j].1 + ((x1 - x0) / len) * side * w * 0.22,
                    r: w * 0.2,
                    alpha: 0.75,
                });
            }
        }
    }

    let (gx, gy) = gaze(mode, t);
    let wide = match mode {
        Mode::Listening => 1.08 + 0.06 * lv,
        Mode::Speaking => 1.0 + 0.05 * talk,
        _ => 1.0,
    };
    let lid = match mode {
        Mode::Offline => 0.0,
        Mode::Thinking => 0.82 * eye_open(t),
        Mode::Listening => eye_open(t),
        _ => 0.96 * eye_open(t),
    };
    let pr = if listening { 2.95 } else { 2.65 };
    let eye = |dx: f64| -> Eye {
        let (ex, ey) = turn((CX + dx, cy + 0.5));
        let (erx, ery) = (4.5 * wide, 5.1 * wide);
        Eye {
            cx: ex,
            cy: ey,
            rx: erx,
            ry: ery,
            open: lid,
            px: ex + gx * (erx - pr - 0.3),
            py: ey + gy * (ery - pr - 0.3) * lid,
            pr,
        }
    };
    let eyes = [eye(-7.2), eye(7.2)];

    let raise = match mode {
        Mode::Listening => 1.6 + lv,
        Mode::Thinking => 0.8,
        Mode::Speaking => 0.5 + 1.6 * talk,
        _ => 0.3,
    };
    let brow = |i: usize| -> [Pt; 3] {
        let e = &eyes[i];
        let side = if i == 0 { -1.0 } else { 1.0 };
        let thinking = matches!(mode, Mode::Thinking);
        let lift = raise + if thinking && i == 0 { 1.4 } else { 0.0 };
        let y = e.cy - e.ry - 1.8 - lift;
        let slant = if thinking && i == 1 {
            0.9
        } else if listening {
            -0.5
        } else {
            0.0
        };
        [
            turn((e.cx - 3.2 * side, y + 1.0 + slant)),
            turn((e.cx, y - 1.1)),
            turn((e.cx + 3.2 * side, y + 1.4 - slant)),
        ]
    };
    let brows = [brow(0), brow(1)];
    let brow_alpha = if matches!(mode, Mode::Offline) {
        0.0
    } else {
        0.55
    };

    let blush = match mode {
        Mode::Speaking => 0.6 + 0.2 * talk,
        Mode::Offline => 0.25,
        _ => 0.5,
    };
    let cheek = |dx: f64| {
        let (x, y) = turn((CX + dx, cy + 7.2));
        Dot {
            x,
            y,
            r: 3.4,
            alpha: blush,
        }
    };
    let cheeks = [cheek(-12.6), cheek(12.6)];

    let (mx, my) = turn((
        CX + if matches!(mode, Mode::Thinking) {
            2.0
        } else {
            0.0
        },
        cy + 9.0,
    ));
    let flat = |half: f64, curve: f64| Mouth {
        x: mx,
        y: my,
        half,
        upper: 0.0,
        lower: 0.0,
        curve,
    };
    let mouth = match (mode, mouth) {
        (Mode::Speaking, Some(m)) => Mouth {
            x: mx,
            y: my,
            half: 5.4 * m.width * (1.0 - 0.3 * m.round),
            upper: 0.3 + m.open * 1.7 + m.round,
            lower: 0.6 + m.open * 4.8 + m.round * 1.2,
            curve: 0.0,
        },
        (Mode::Listening, _) => Mouth {
            x: mx,
            y: my,
            half: 1.7,
            upper: 0.9 + 0.4 * lv,
            lower: 1.4 + 0.9 * lv,
            curve: 0.0,
        },
        (Mode::Thinking, _) => flat(2.4, -0.4 + 0.3 * (t * 2.4).sin()),
        (Mode::Offline, _) => flat(2.2, 0.6),
        _ => flat(3.8, 2.6 + 0.3 * breath),
    };

    let bubble = |k: usize| {
        let offline = matches!(mode, Mode::Offline);
        let active = offline || matches!(mode, Mode::Thinking);
        let period = if offline { 3.0 } else { 1.8 };
        let p = (t + k as f64 * period / 3.0).rem_euclid(period) / period;
        let (x, y) = turn((
            CX + 17.0 + 4.0 * p + (p * 7.0 + k as f64).sin() * 1.2,
            cy - 6.0 - 15.0 * p,
        ));
        Dot {
            x,
            y,
            r: if offline { 1.0 } else { 0.9 } + 1.4 * p,
            alpha: if active { (PI * p).sin() * 0.9 } else { 0.0 },
        }
    };
    let bubbles = [bubble(0), bubble(1), bubble(2)];

    Pose {
        head,
        back,
        front,
        over,
        suckers,
        eyes,
        brows,
        brow_alpha,
        cheeks,
        mouth,
        bubbles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_inside_the_grid() {
        for mode in [
            Mode::Idle,
            Mode::Listening,
            Mode::Thinking,
            Mode::Speaking,
            Mode::Offline,
        ] {
            let mut t = 0.0;
            while t < 12.0 {
                let p = pose(mode, t, 1.0, Some(synthetic_viseme(t)));
                let all = p
                    .head
                    .iter()
                    .chain(p.back.iter().chain(&p.front).chain(&p.over).flatten());
                for &(x, y) in all {
                    assert!(
                        (0.0..64.0).contains(&x) && (0.0..64.0).contains(&y),
                        "{x},{y}"
                    );
                }
                t += 0.37;
            }
        }
    }

    #[test]
    fn sleeps_offline_and_thinks_with_an_arm_under_the_chin() {
        assert_eq!(pose(Mode::Offline, 1.0, 0.0, None).eyes[0].open, 0.0);
        assert_eq!(pose(Mode::Thinking, 1.0, 0.0, None).over.len(), 1);
        assert_eq!(pose(Mode::Idle, 1.0, 0.0, None).front.len(), 6);
    }

    #[test]
    fn blinks_now_and_then() {
        let closed = (0..2000)
            .filter(|i| eye_open(*i as f64 * 0.01) < 0.5)
            .count();
        assert!(closed > 0 && closed < 200);
    }
}
