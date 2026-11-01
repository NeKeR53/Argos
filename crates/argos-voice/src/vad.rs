//! Détection de parole pour l'écoute mains libres : un seuil d'énergie qui suit le bruit
//! ambiant. Sans dépendance ni modèle, déterministe, donc testable sur des signaux
//! synthétiques.
//!
//! Pendant que la synthèse parle, le haut-parleur revient dans le micro. Ce n'est pas une
//! vraie annulation d'écho : le seuil suit alors un plancher d'écho à part, plus haut, et
//! il faut parler plus fort et plus longtemps pour couper la parole (barge-in). Un casque
//! supprime le problème.

use crate::audio::{SAMPLE_RATE, rms};
use std::collections::VecDeque;

/// Trame d'analyse : 20 ms.
pub const FRAME: usize = SAMPLE_RATE as usize / 50;
const FRAME_MS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadSettings {
    /// Rapport au plancher de bruit au-delà duquel une trame est voisée.
    pub start_ratio: f32,
    /// Le même pendant que la synthèse parle (écho du haut-parleur).
    pub echo_ratio: f32,
    /// Niveau RMS minimal d'une trame voisée, même dans un silence parfait.
    pub min_level: f32,
    /// Parole continue qui ouvre un énoncé.
    pub start_ms: u32,
    /// Le même pendant que la synthèse parle : on ne coupe pas sur un raclement de gorge.
    pub echo_start_ms: u32,
    /// Silence qui clôt un énoncé.
    pub end_ms: u32,
    /// Audio gardé avant l'ouverture, pour ne pas couper la première syllabe.
    pub preroll_ms: u32,
    /// Au-delà, l'énoncé est clos de force (bruit continu, télévision…).
    pub max_ms: u32,
}

/// Plancher d'écho de départ, avant que le haut-parleur ait été entendu : plus haut que
/// le plancher de la voix, pour que les premières phrases de la synthèse n'ouvrent pas
/// d'énoncé.
const INITIAL_ECHO_FLOOR: f32 = 0.012;

impl Default for VadSettings {
    fn default() -> Self {
        Self {
            start_ratio: 3.0,
            echo_ratio: 4.0,
            min_level: 0.006,
            start_ms: 160,
            echo_start_ms: 320,
            end_ms: 900,
            preroll_ms: 300,
            max_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VadEvent {
    /// Un énoncé commence (assez de parole continue).
    Started,
    /// L'énoncé est fini ; `samples` le contient, pré-roulement compris.
    Ended { samples: Vec<f32> },
}

pub struct Vad {
    settings: VadSettings,
    /// Plancher de bruit ambiant, et celui mesuré pendant que la synthèse parle.
    floor: f32,
    echo_floor: f32,
    /// Fin d'une trame incomplète, en attente de la suite.
    partial: Vec<f32>,
    preroll: VecDeque<f32>,
    state: State,
}

enum State {
    Idle {
        /// Trames voisées consécutives, pas encore assez pour ouvrir.
        voiced: u32,
        candidate: Vec<f32>,
    },
    Speaking {
        utterance: Vec<f32>,
        silent: u32,
    },
}

impl Vad {
    pub fn new(settings: VadSettings) -> Self {
        Self {
            settings,
            floor: settings.min_level / settings.start_ratio,
            echo_floor: INITIAL_ECHO_FLOOR,
            partial: Vec::with_capacity(FRAME),
            preroll: VecDeque::new(),
            state: idle(),
        }
    }

    /// Oublie l'énoncé en cours (le push-to-talk reprend la main), garde les planchers.
    pub fn reset(&mut self) {
        self.partial.clear();
        self.preroll.clear();
        self.state = idle();
    }

    /// Plancher de bruit ambiant actuel (RMS), et seuil d'ouverture qui en découle.
    pub fn floor(&self) -> (f32, f32) {
        let s = self.settings;
        (self.floor, (self.floor * s.start_ratio).max(s.min_level))
    }

    /// L'énoncé en cours, pré-roulement compris ; `None` hors énoncé.
    pub fn current(&self) -> Option<&[f32]> {
        match &self.state {
            State::Speaking { utterance, .. } => Some(utterance),
            State::Idle { .. } => None,
        }
    }

    /// Analyse des échantillons 16 kHz mono. `playing` : la synthèse parle en ce moment.
    pub fn push(&mut self, samples: &[f32], playing: bool) -> Vec<VadEvent> {
        let mut events = vec![];
        self.partial.extend_from_slice(samples);
        let whole = self.partial.len() / FRAME * FRAME;
        let frames: Vec<f32> = self.partial.drain(..whole).collect();
        for frame in frames.chunks_exact(FRAME) {
            if let Some(event) = self.frame(frame, playing) {
                events.push(event);
            }
        }
        events
    }

    fn frame(&mut self, frame: &[f32], playing: bool) -> Option<VadEvent> {
        let level = rms(frame);
        let s = self.settings;
        let (floor, ratio, start_ms) = if playing {
            (self.echo_floor, s.echo_ratio, s.echo_start_ms)
        } else {
            (self.floor, s.start_ratio, s.start_ms)
        };
        let threshold = (floor * ratio).max(s.min_level);
        match &mut self.state {
            State::Idle { voiced, candidate } => {
                if level > threshold {
                    *voiced += 1;
                    candidate.extend_from_slice(frame);
                    if *voiced * FRAME_MS >= start_ms {
                        let mut utterance: Vec<f32> = self.preroll.drain(..).collect();
                        utterance.append(candidate);
                        self.state = State::Speaking {
                            utterance,
                            silent: 0,
                        };
                        return Some(VadEvent::Started);
                    }
                    return None;
                }
                // Fausse alerte : ces trames rejoignent le pré-roulement.
                let candidate = std::mem::take(candidate);
                *voiced = 0;
                self.remember(&candidate);
                self.remember(frame);
                let tracked = if playing {
                    &mut self.echo_floor
                } else {
                    &mut self.floor
                };
                *tracked = follow(*tracked, level);
                None
            }
            State::Speaking { utterance, silent } => {
                utterance.extend_from_slice(frame);
                // Hystérésis : on reste dans l'énoncé un peu sous le seuil d'ouverture.
                if level > threshold * 0.6 {
                    *silent = 0;
                } else {
                    *silent += 1;
                }
                let too_long = utterance.len() >= (s.max_ms as usize) * FRAME / FRAME_MS as usize;
                if *silent * FRAME_MS >= s.end_ms || too_long {
                    let samples = std::mem::take(utterance);
                    self.state = idle();
                    return Some(VadEvent::Ended { samples });
                }
                None
            }
        }
    }

    fn remember(&mut self, samples: &[f32]) {
        self.preroll.extend(samples);
        let keep = self.settings.preroll_ms as usize * FRAME / FRAME_MS as usize;
        while self.preroll.len() > keep {
            self.preroll.pop_front();
        }
    }
}

fn idle() -> State {
    State::Idle {
        voiced: 0,
        candidate: vec![],
    }
}

/// Suit le plancher : il descend vite (le bruit est retombé) et monte lentement (pour
/// qu'une voix soutenue ne devienne pas « le bruit »).
fn follow(floor: f32, level: f32) -> f32 {
    let rate = if level < floor { 0.2 } else { 0.01 };
    floor + (level - floor) * rate
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ms` millisecondes d'un signal carré d'amplitude `level` (son RMS vaut `level`).
    fn tone(ms: u32, level: f32) -> Vec<f32> {
        let n = ms as usize * FRAME / FRAME_MS as usize;
        (0..n)
            .map(|i| if i % 2 == 0 { level } else { -level })
            .collect()
    }

    fn run(vad: &mut Vad, chunks: &[(u32, f32, bool)]) -> Vec<VadEvent> {
        let mut events = vec![];
        for &(ms, level, playing) in chunks {
            // Par morceaux irréguliers, comme un vrai flux micro.
            for piece in tone(ms, level).chunks(117) {
                events.extend(vad.push(piece, playing));
            }
        }
        events
    }

    #[test]
    fn speech_after_silence_makes_one_utterance_with_its_preroll() {
        let mut vad = Vad::new(VadSettings::default());
        let events = run(
            &mut vad,
            &[
                (1000, 0.002, false),
                (600, 0.1, false),
                (1000, 0.002, false),
            ],
        );
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0], VadEvent::Started);
        let VadEvent::Ended { samples } = &events[1] else {
            panic!("fin attendue");
        };
        // Pré-roulement (300 ms) + parole (600 ms) + silence de fin (900 ms).
        let ms = samples.len() * FRAME_MS as usize / FRAME;
        assert!((1700..=1820).contains(&ms), "{ms} ms");
    }

    #[test]
    fn short_noises_and_steady_hum_do_not_open_an_utterance() {
        let mut vad = Vad::new(VadSettings::default());
        // Un claquement de 60 ms, puis un ronronnement qui monte doucement.
        let mut chunks = vec![(500, 0.002, false), (60, 0.2, false), (500, 0.002, false)];
        chunks.extend((1..=40).map(|step| (100, 0.002 + step as f32 * 0.0002, false)));
        assert_eq!(run(&mut vad, &chunks), vec![]);
    }

    #[test]
    fn while_the_speaker_talks_only_a_louder_longer_voice_barges_in() {
        let mut vad = Vad::new(VadSettings::default());
        // Le haut-parleur revient dans le micro vers 0,03 : le plancher d'écho le suit.
        let mut events = run(&mut vad, &[(2000, 0.03, true)]);
        // Une voix au même niveau que l'écho ne coupe pas.
        events.extend(run(&mut vad, &[(400, 0.035, true)]));
        assert_eq!(events, vec![]);
        // Une voix franche par-dessus coupe.
        let events = run(&mut vad, &[(500, 0.2, true)]);
        assert_eq!(events, vec![VadEvent::Started]);
    }

    #[test]
    fn a_continuous_noise_is_cut_at_the_maximum_length() {
        let settings = VadSettings {
            max_ms: 2000,
            ..VadSettings::default()
        };
        let mut vad = Vad::new(settings);
        let events = run(&mut vad, &[(300, 0.002, false), (5000, 0.3, false)]);
        let ended = events
            .iter()
            .filter(|event| matches!(event, VadEvent::Ended { .. }))
            .count();
        assert!(ended >= 2, "{events:?}");
    }

    #[test]
    fn reset_forgets_the_current_utterance() {
        let mut vad = Vad::new(VadSettings::default());
        assert_eq!(
            run(&mut vad, &[(500, 0.002, false), (400, 0.1, false)]),
            vec![VadEvent::Started]
        );
        vad.reset();
        assert_eq!(run(&mut vad, &[(1500, 0.002, false)]), vec![]);
    }
}
