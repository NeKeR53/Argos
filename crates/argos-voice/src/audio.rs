//! Traitement du signal sans dépendance : mixage mono, rééchantillonnage vers 16 kHz,
//! niveau RMS, encodage WAV.

use std::borrow::Cow;
use std::f64::consts::PI;

/// Fréquence attendue par Whisper.
pub const SAMPLE_RATE: u32 = 16_000;

/// whisper.cpp refuse une entrée de moins d'une seconde : on complète par du silence.
const WHISPER_MIN_SAMPLES: usize = SAMPLE_RATE as usize * 11 / 10;
/// Longueur du filtre anti-repliement (impaire, pour un retard entier).
const FIR_TAPS: usize = 63;
/// Coupure à 90 % de la nouvelle fréquence de Nyquist (7,2 kHz) : la voix est intacte.
const CUTOFF_RATIO: f64 = 0.45;

/// Moyenne des canaux d'un tampon entrelacé, ajoutée à `out`.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
}

/// Niveau efficace d'un tampon, entre 0 et 1 pour un signal normalisé.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Rééchantillonneur en flux vers 16 kHz : filtre passe-bas (sinus cardinal fenêtré,
/// seulement si l'on descend en fréquence) puis interpolation linéaire. Découper
/// l'entrée en morceaux quelconques donne la même sortie qu'un seul appel.
pub struct Resampler {
    /// Pas de lecture dans l'entrée pour une sortie (fréquence d'entrée / 16 kHz).
    step: f64,
    fir: Vec<f32>,
    /// Les `FIR_TAPS - 1` dernières entrées, pour filtrer à cheval sur deux morceaux.
    history: Vec<f32>,
    /// Position de la prochaine sortie, relative au morceau courant (-1 = `last`).
    position: f64,
    /// Dernier échantillon filtré du morceau précédent.
    last: f32,
    filtered: Vec<f32>,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Self {
        let step = f64::from(input_rate) / f64::from(SAMPLE_RATE);
        let fir = if input_rate > SAMPLE_RATE {
            low_pass(CUTOFF_RATIO / step)
        } else {
            Vec::new()
        };
        let history = vec![0.0; fir.len().saturating_sub(1)];
        Self {
            step,
            fir,
            history,
            position: 0.0,
            last: 0.0,
            filtered: Vec::new(),
        }
    }

    /// Rééchantillonne `input` (mono, à la fréquence d'entrée) et ajoute le résultat à `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.step == 1.0 {
            out.extend_from_slice(input);
            return;
        }
        self.filter(input);
        let samples = &self.filtered;
        let len = samples.len();
        while self.position < (len - 1) as f64 {
            let index = self.position.floor();
            let fraction = (self.position - index) as f32;
            let index = index as isize;
            let a = if index < 0 {
                self.last
            } else {
                samples[index as usize]
            };
            let b = samples[(index + 1) as usize];
            out.push(a + (b - a) * fraction);
            self.position += self.step;
        }
        self.position -= len as f64;
        self.last = samples[len - 1];
    }

    /// Remplit `self.filtered` avec l'entrée filtrée (ou recopiée si pas de filtre).
    fn filter(&mut self, input: &[f32]) {
        self.filtered.clear();
        if self.fir.is_empty() {
            self.filtered.extend_from_slice(input);
            return;
        }
        let keep = self.history.len();
        self.history.extend_from_slice(input);
        let window = &self.history;
        self.filtered.extend((0..input.len()).map(|i| {
            window[i..i + self.fir.len()]
                .iter()
                .zip(&self.fir)
                .map(|(x, h)| x * h)
                .sum::<f32>()
        }));
        let drop = self.history.len() - keep;
        self.history.drain(..drop);
    }
}

/// Coefficients d'un passe-bas de fréquence de coupure `cutoff` (en cycles par
/// échantillon), fenêtre de Hamming, gain unitaire en continu.
fn low_pass(cutoff: f64) -> Vec<f32> {
    let middle = (FIR_TAPS - 1) as f64 / 2.0;
    let raw: Vec<f64> = (0..FIR_TAPS)
        .map(|k| {
            let x = k as f64 - middle;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let window = 0.54 - 0.46 * (2.0 * PI * k as f64 / (FIR_TAPS - 1) as f64).cos();
            sinc * window
        })
        .collect();
    let sum: f64 = raw.iter().sum();
    raw.iter().map(|h| (h / sum) as f32).collect()
}

/// Complète par du silence ce que whisper.cpp jugerait trop court.
pub fn pad_for_whisper(samples: &[f32]) -> Cow<'_, [f32]> {
    if samples.len() >= WHISPER_MIN_SAMPLES {
        return Cow::Borrowed(samples);
    }
    let mut padded = samples.to_vec();
    padded.resize(WHISPER_MIN_SAMPLES, 0.0);
    Cow::Owned(padded)
}

/// Fichier WAV PCM 16 bits, mono, 16 kHz.
pub fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // taille du bloc fmt
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // octets par seconde
    wav.extend_from_slice(&2u16.to_le_bytes()); // octets par trame
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits par échantillon
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        wav.extend_from_slice(&value.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let count = (f64::from(rate) * seconds) as usize;
        (0..count)
            .map(|n| (2.0 * PI * frequency * n as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    fn resample_in_chunks(input: &[f32], rate: u32, chunk: usize) -> Vec<f32> {
        let mut resampler = Resampler::new(rate);
        let mut out = Vec::new();
        for piece in input.chunks(chunk) {
            resampler.process(piece, &mut out);
        }
        out
    }

    /// Nombre de passages par zéro montants : deux fois rien, une fois par période.
    fn rising_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count()
    }

    #[test]
    fn downmix_averages_the_channels() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0]);
        downmix(&[0.25], 1, &mut out);
        assert_eq!(out.last(), Some(&0.25));
    }

    #[test]
    fn rms_of_known_signals() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5]) - 0.5).abs() < 1e-6);
        let tone = sine(440.0, 16_000, 1.0);
        assert!((rms(&tone) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }

    #[test]
    fn sixteen_khz_input_passes_through() {
        let input = sine(440.0, 16_000, 0.1);
        assert_eq!(resample_in_chunks(&input, 16_000, 37), input);
    }

    #[test]
    fn common_rates_give_one_second_of_16khz() {
        for rate in [48_000, 44_100, 8_000] {
            let out = resample_in_chunks(&sine(440.0, rate, 1.0), rate, 512);
            assert!(
                (15_990..=16_010).contains(&out.len()),
                "{rate} Hz → {} échantillons",
                out.len()
            );
        }
    }

    #[test]
    fn chunking_does_not_change_the_output() {
        let input = sine(523.0, 44_100, 0.5);
        let whole = resample_in_chunks(&input, 44_100, input.len());
        let pieces = resample_in_chunks(&input, 44_100, 113);
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn speech_band_is_kept_and_aliases_are_removed() {
        let voice = resample_in_chunks(&sine(440.0, 48_000, 1.0), 48_000, 480);
        // Le retard du filtre (31 échantillons à 48 kHz) n'ajoute qu'un début atténué.
        let steady = &voice[100..];
        assert!((rms(steady) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02);
        // 440 Hz sur ~0,99 s : la fréquence est conservée.
        let periods = rising_crossings(steady);
        assert!((434..=438).contains(&periods), "{periods} périodes");

        // 15 kHz se replierait à 1 kHz sans filtre.
        let alias = resample_in_chunks(&sine(15_000.0, 48_000, 1.0), 48_000, 480);
        assert!(rms(&alias[100..]) < 0.02, "{}", rms(&alias[100..]));
    }

    #[test]
    fn short_audio_is_padded_for_whisper() {
        let short = vec![0.1; 1_000];
        let padded = pad_for_whisper(&short);
        assert_eq!(padded.len(), 17_600);
        assert_eq!(&padded[..1_000], &short[..]);
        assert!(padded[1_000..].iter().all(|&s| s == 0.0));
        let long = vec![0.1; 20_000];
        assert!(matches!(pad_for_whisper(&long), Cow::Borrowed(_)));
    }

    #[test]
    fn wav_header_and_samples() {
        let wav = encode_wav(&[0.0, 1.0, -1.0, 2.0, 0.5]);
        assert_eq!(wav.len(), 44 + 10);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 10);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1); // mono
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 10);
        let samples: Vec<i16> = wav[44..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        // Les valeurs hors de [-1, 1] sont écrêtées, pas repliées.
        assert_eq!(samples, vec![0, 32_767, -32_767, 32_767, 16_384]);
    }
}
