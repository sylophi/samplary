use crate::dsp::{Dct, MelBank, Stft, db, decimate, mean, median, stdev};
use crate::pitch::{Yin, hz_to_midi};

/// Bump whenever what a scan stores changes (descriptors, thumbnails).
/// Stored analyses older than this are redone.
pub const ANALYSIS_VERSION: i64 = 3;

/// Analysis looks at most this much of a file.
pub const MAX_SECONDS: f64 = 30.0;

const N_MELS: usize = 40;
const N_MFCC: usize = 12;
const ATTACK_FRAMES: usize = 5;
/// Envelope sample points after the onset, roughly log-spaced so attacks
/// get as much say as tails.
const ENV_TIMES_MS: [f32; 16] = [
    0.0, 10.0, 20.0, 35.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 450.0, 650.0, 900.0, 1300.0,
    1900.0, 2800.0,
];
const CONTOUR_POINTS: usize = 8;
/// Frames quieter than this, relative to the loudest, don't describe the sound.
const ACTIVE_DB: f32 = 45.0;
const FLOOR_DB: f32 = 60.0;

pub const ENV_DIMS: usize = ENV_TIMES_MS.len() + 5;
pub const SPEC_DIMS: usize = N_MELS + N_MELS / 2;
pub const TIMBRE_DIMS: usize = 3 * N_MFCC + 4;
pub const PITCH_DIMS: usize = 2 + CONTOUR_POINTS + 12;
pub const DESCRIPTOR_DIMS: usize = ENV_DIMS + SPEC_DIMS + TIMBRE_DIMS + PITCH_DIMS;

/// Per-aspect feature vectors, flattened: amplitude envelope, spectrum,
/// timbre, pitch.
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor(pub Vec<f32>);

impl Descriptor {
    pub fn env(&self) -> &[f32] {
        &self.0[..ENV_DIMS]
    }
    pub fn spec(&self) -> &[f32] {
        &self.0[ENV_DIMS..ENV_DIMS + SPEC_DIMS]
    }
    pub fn timbre(&self) -> &[f32] {
        &self.0[ENV_DIMS + SPEC_DIMS..ENV_DIMS + SPEC_DIMS + TIMBRE_DIMS]
    }
    pub fn pitch(&self) -> &[f32] {
        &self.0[DESCRIPTOR_DIMS - PITCH_DIMS..]
    }
    /// How much of the sound has a clear fundamental (0..1).
    pub fn voiced(&self) -> f32 {
        self.pitch()[0]
    }
    pub fn midi(&self) -> f32 {
        self.pitch()[1]
    }
    pub fn contour(&self) -> &[f32] {
        &self.pitch()[2..2 + CONTOUR_POINTS]
    }
    pub fn chroma(&self) -> &[f32] {
        &self.pitch()[2 + CONTOUR_POINTS..]
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        (bytes.len() == DESCRIPTOR_DIMS * 4).then(|| {
            Self(
                bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
            )
        })
    }
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub descriptor: Descriptor,
    /// Median MIDI pitch, when most of the sound has a clear fundamental.
    pub pitch: Option<f32>,
    pub key: Option<(usize, bool)>,
    pub tempo: Option<f32>,
    /// Sounds like a loop: sustained to the end, rhythmic, and cut to a
    /// whole number of beats.
    pub looped: bool,
    pub peak_db: f32,
    pub loudness_db: f32,
}

pub fn fft_size(sr: u32) -> usize {
    match sr {
        0..=26_000 => 1024,
        26_001..=50_000 => 2048,
        _ => 4096,
    }
}

/// `seconds` is the whole file's length. `mono` may be a prefix of it.
pub fn analyze(mono: &[f32], sr: u32, seconds: f64) -> Analysis {
    let srf = sr as f32;
    let n_fft = fft_size(sr);
    let hop = n_fft / 4;
    let hop_s = hop as f32 / srf;

    // Block RMS envelope, one block per STFT hop.
    let env_db: Vec<f32> = mono
        .chunks(hop)
        .map(|b| db(b.iter().map(|v| v * v).sum::<f32>() / b.len() as f32))
        .collect();
    let peak_db = db(mono.iter().fold(0f32, |m, v| m.max(v.abs())).powi(2));
    let (peak_block, &peak_env) = env_db
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap_or((0, &-120.0));
    let threshold = (peak_env - ACTIVE_DB).max(-90.0);
    let first = env_db.iter().position(|&v| v > threshold).unwrap_or(0);
    let last = env_db.iter().rposition(|&v| v > threshold).unwrap_or(first);
    let active: Vec<usize> = (first..=last).filter(|&t| env_db[t] > threshold).collect();
    let active_s = (last - first + 1) as f32 * hop_s;
    let loudness_db = db(mean(
        &active
            .iter()
            .map(|&t| 10f32.powf(env_db[t] / 10.0))
            .collect::<Vec<_>>(),
    ));

    // Spectral frames.
    let stft = Stft::new(n_fft, hop);
    let bin_hz = stft.bin_hz(srf);
    let mel_bank = MelBank::new(N_MELS, stft.bins(), bin_hz, 30.0, (srf / 2.0).min(16_000.0));
    let dct = Dct::new(N_MELS, N_MFCC);
    let frames = stft.frame_count(mono.len());
    let mut mel = vec![[0f32; N_MELS]; frames];
    let mut mfcc = vec![[0f32; N_MFCC]; frames];
    let mut centroid = vec![0f32; frames];
    let mut flatness = vec![0f32; frames];
    let mut rolloff = vec![0f32; frames];
    let mut chroma = [0f32; 12];
    let chroma_bins: Vec<(usize, usize)> = (1..stft.bins())
        .filter_map(|k| {
            let f = k as f32 * bin_hz;
            (200.0..5000.0).contains(&f).then(|| {
                let pc = (hz_to_midi(f).round() as i32).rem_euclid(12) as usize;
                (k, pc)
            })
        })
        .collect();
    let flat_lo = (50.0 / bin_hz).ceil() as usize;
    let flat_hi = ((16_000f32.min(srf / 2.0)) / bin_hz) as usize;
    let mel_floor = peak_env - 80.0;
    stft.for_each_frame(mono, |t, power| {
        mel_bank.apply(power, &mut mel[t]);
        let mel_db: Vec<f32> = mel[t].iter().map(|&p| db(p).max(mel_floor)).collect();
        dct.apply(&mel_db, &mut mfcc[t]);
        let total: f32 = power.iter().sum::<f32>() + 1e-12;
        centroid[t] = power
            .iter()
            .enumerate()
            .map(|(k, p)| k as f32 * bin_hz * p)
            .sum::<f32>()
            / total;
        let band = &power[flat_lo..flat_hi.max(flat_lo + 1)];
        let log_mean = mean(&band.iter().map(|p| (p + 1e-12).ln()).collect::<Vec<_>>());
        flatness[t] = log_mean.exp() / (mean(band) + 1e-12);
        let mut acc = 0.0;
        rolloff[t] = power
            .iter()
            .position(|p| {
                acc += p;
                acc >= 0.85 * total
            })
            .unwrap_or(0) as f32
            * bin_hz;
        if t >= first && t <= last && env_db.get(t).is_some_and(|&v| v > threshold) {
            for &(k, pc) in &chroma_bins {
                chroma[pc] += power[k];
            }
        }
    });
    let chroma_max = chroma.iter().cloned().fold(1e-12, f32::max);
    chroma.iter_mut().for_each(|c| *c /= chroma_max);

    // Onset strength: summed rises in log-mel energy.
    let onset: Vec<f32> = (0..frames)
        .map(|t| {
            if t == 0 {
                return 0.0;
            }
            (0..N_MELS)
                .map(|m| (db(mel[t][m]).max(mel_floor) - db(mel[t - 1][m]).max(mel_floor)).max(0.0))
                .sum::<f32>()
                / N_MELS as f32
        })
        .collect();
    let onsets = count_onsets(&onset[first..=last.min(frames - 1)]);

    // Amplitude.
    let mut env = Vec::with_capacity(ENV_DIMS);
    for ms in ENV_TIMES_MS {
        let t = first + (ms / 1000.0 / hop_s).round() as usize;
        let v = if t <= last {
            env_db[t] - peak_env
        } else {
            -FLOOR_DB
        };
        env.push((v.max(-FLOOR_DB) + FLOOR_DB) / FLOOR_DB);
    }
    let attack_s = peak_block.saturating_sub(first) as f32 * hop_s;
    let span: Vec<f32> = env_db[first..=last]
        .iter()
        .map(|v| v.max(peak_env - FLOOR_DB))
        .collect();
    let tail_len = (span.len() / 10).max(1);
    let tail_db = mean(&span[span.len() - tail_len..]) - peak_env;
    env.push((active_s + 0.02).ln());
    env.push((attack_s + 0.005).ln());
    env.push(stdev(&span) / 20.0);
    env.push((1.0 + onsets as f32 / active_s).ln());
    env.push((tail_db.max(-FLOOR_DB) + FLOOR_DB) / FLOOR_DB);

    // Spectrum: long-term and attack shapes, relative to their loudest band.
    let mean_bands = |range: &[usize]| -> [f32; N_MELS] {
        let mut acc = [0f32; N_MELS];
        for &t in range {
            for m in 0..N_MELS {
                acc[m] += mel[t.min(frames - 1)][m];
            }
        }
        acc
    };
    let attack: Vec<usize> = active.iter().copied().take(ATTACK_FRAMES).collect();
    let shape = |bands: &[f32]| -> Vec<f32> {
        let top = bands.iter().map(|&p| db(p)).fold(f32::MIN, f32::max);
        bands
            .iter()
            .map(|&p| (db(p) - top).max(-FLOOR_DB) / FLOOR_DB + 1.0)
            .collect()
    };
    let body_bands = mean_bands(&active);
    let attack_bands = mean_bands(&attack);
    let attack_coarse: Vec<f32> = attack_bands.chunks(2).map(|c| c[0] + c[1]).collect();
    let mut spec = shape(&body_bands);
    spec.extend(shape(&attack_coarse));

    // Timbre.
    let frames_of = |idx: &[usize], f: &dyn Fn(usize) -> f32| -> Vec<f32> {
        idx.iter().map(|&t| f(t.min(frames - 1))).collect()
    };
    let mut timbre = Vec::with_capacity(TIMBRE_DIMS);
    for c in 0..N_MFCC {
        timbre.push(mean(&frames_of(&active, &|t| mfcc[t][c])));
    }
    for c in 0..N_MFCC {
        timbre.push(stdev(&frames_of(&active, &|t| mfcc[t][c])));
    }
    for c in 0..N_MFCC {
        timbre.push(mean(&frames_of(&attack, &|t| mfcc[t][c])));
    }
    let log_centroid = frames_of(&active, &|t| (centroid[t] + 20.0).log2());
    timbre.push(mean(&log_centroid));
    timbre.push(stdev(&log_centroid));
    timbre.push(mean(&frames_of(&active, &|t| (flatness[t] + 1e-6).log10())));
    timbre.push(mean(&frames_of(&active, &|t| (rolloff[t] + 20.0).log2())));

    // Pitch.
    let factor = ((srf / 11_025.0).round() as usize).max(1);
    let sr_d = srf / factor as f32;
    let low = decimate(
        &mono[(first * hop).min(mono.len())..((last + 1) * hop).min(mono.len())],
        factor,
    );
    let yin = Yin::new(sr_d, 30.0, 2000.0, (0.07 * sr_d) as usize);
    let step = ((hop_s * sr_d) as usize).max(low.len() / 64).max(1);
    let mut padded = low.clone();
    padded.resize(low.len() + yin.span(), 0.0);
    let mut voiced_midi = Vec::new();
    let mut track: Vec<Option<f32>> = Vec::new();
    // Voicing is weighed by energy: a clear note matters more than the
    // noisy end of its decay.
    let (mut voiced_energy, mut loud_energy) = (0f32, 0f32);
    for pos in (0..low.len().max(1)).step_by(step) {
        let block = first + (pos as f32 * factor as f32 / hop as f32) as usize;
        let level = env_db.get(block).copied().unwrap_or(-120.0);
        let loud = level > peak_env - 30.0;
        let est = yin.estimate(&padded[pos..pos + yin.span()]);
        let voiced = est
            .filter(|p| loud && p.clarity >= 0.8)
            .map(|p| hz_to_midi(p.hz));
        let energy = if loud {
            10f32.powf((level - peak_env) / 10.0)
        } else {
            0.0
        };
        loud_energy += energy;
        if let Some(m) = voiced {
            voiced_midi.push(m);
            voiced_energy += energy;
        }
        track.push(voiced);
    }
    let voiced_frac = voiced_energy / loud_energy.max(1e-9);
    let pitched = voiced_frac >= 0.4 && voiced_midi.len() >= 2;
    let midi = if pitched { median(&voiced_midi) } else { 0.0 };
    let mut pitch_vec = vec![voiced_frac, midi];
    for k in 0..CONTOUR_POINTS {
        let i = (k * (track.len() - 1)) / (CONTOUR_POINTS - 1).max(1);
        let rel = match track.get(i).copied().flatten() {
            Some(m) if pitched => (m - midi).clamp(-12.0, 12.0),
            _ => 0.0,
        };
        pitch_vec.push(rel);
    }
    pitch_vec.extend(chroma);

    // Tempo, and whether the file looks cut to a whole number of beats.
    let tempo = estimate_tempo(&onset[first..=last.min(frames - 1)], hop_s, seconds as f32);
    let looped = active_s >= 1.0 && tail_db > -30.0 && onsets >= 3 && tempo.is_some_and(|t| t.1);

    let mut values = env;
    values.extend(spec);
    values.extend(timbre);
    values.extend(pitch_vec);
    debug_assert_eq!(values.len(), DESCRIPTOR_DIMS);
    let values = values
        .into_iter()
        .map(|v| if v.is_finite() { v } else { 0.0 })
        .collect();

    Analysis {
        descriptor: Descriptor(values),
        pitch: pitched.then_some(midi),
        key: (active_s >= 0.5).then(|| detect_key(&chroma)).flatten(),
        tempo: tempo.map(|t| t.0),
        looped,
        peak_db,
        loudness_db,
    }
}

fn count_onsets(strength: &[f32]) -> usize {
    if strength.len() < 3 {
        return 1;
    }
    let threshold = (mean(strength) + 0.5 * stdev(strength)).max(1.5);
    let mut count = 0;
    let mut last_hit: Option<usize> = None;
    for t in 0..strength.len() {
        let lo = t.saturating_sub(3);
        let hi = (t + 4).min(strength.len());
        let is_peak = strength[lo..hi].iter().all(|&v| v <= strength[t]);
        if is_peak && strength[t] > threshold && last_hit.is_none_or(|h| t - h >= 4) {
            count += 1;
            last_hit = Some(t);
        }
    }
    count.max(1)
}

/// Tempo from the periodicity of onsets, nudged toward 120 BPM. Loops are
/// cut to whole bars, so when a tempo that divides the file's length into
/// bars fits about as well as the best free reading, it wins, exactly.
/// Returns the tempo and whether it fit the length.
fn estimate_tempo(strength: &[f32], hop_s: f32, seconds: f32) -> Option<(f32, bool)> {
    let n = strength.len();
    let max_lag = (60.0 / 60.0 / hop_s).ceil() as usize;
    let min_lag = ((60.0 / 200.0 / hop_s).floor() as usize).max(1);
    if n < 2 * max_lag + 2 {
        return None;
    }
    // A light blur so a beat landing between frames still lines up.
    let smooth: Vec<f32> = (0..n)
        .map(|t| {
            0.5 * strength[t]
                + 0.25 * strength[t.saturating_sub(1)]
                + 0.25 * strength[(t + 1).min(n - 1)]
        })
        .collect();
    let m = mean(&smooth);
    let x: Vec<f32> = smooth.iter().map(|v| v - m).collect();
    let energy: f32 = x.iter().map(|v| v * v).sum();
    if energy <= 0.0 {
        return None;
    }
    // Normalized by overlap, so long lags (whole bars) stay comparable.
    let ac: Vec<f32> = (0..n - 1)
        .map(|lag| {
            let dot: f32 = x[..n - lag].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum();
            dot / (n - lag) as f32 / (energy / n as f32)
        })
        .collect();
    let ac_at = |lag: f32| {
        let i = lag.floor() as usize;
        let f = lag - i as f32;
        (i + 1 < ac.len()).then(|| ac[i] * (1.0 - f) + ac[i + 1] * f)
    };
    // The beat, plus the half bar and bar it builds when they fit in the
    // file: a dotted rhythm repeats at its own period but not at the bar's.
    let score = |bpm: f32| {
        let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        let beat = 60.0 / bpm / hop_s;
        let (mut sum, mut weight) = (0.0, 0.0);
        for (mult, w) in [(1.0, 1.0), (2.0, 0.5), (4.0, 0.5)] {
            if let Some(v) = ac_at(beat * mult).filter(|_| beat * mult < 0.75 * n as f32) {
                sum += w * v;
                weight += w;
            }
        }
        prior * sum / weight.max(1.0)
    };

    let (free_lag, free_score) = (min_lag..=max_lag)
        .map(|lag| (lag, score(60.0 / (lag as f32 * hop_s))))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    let fitted = (4..=256)
        .step_by(4)
        .map(|beats| 60.0 * beats as f32 / seconds)
        .filter(|bpm| (60.0..=200.0).contains(bpm))
        .map(|bpm| (bpm, score(bpm)))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((bpm, s)) = fitted
        && s >= 0.1
        && s >= 0.6 * free_score
    {
        return Some(((bpm * 100.0).round() / 100.0, true));
    }
    if free_score < 0.1 {
        return None;
    }
    let (a, b, c) = (ac[free_lag - 1], ac[free_lag], ac[free_lag + 1]);
    let denom = a - 2.0 * b + c;
    let refined = if denom.abs() > 1e-9 {
        free_lag as f32 + 0.5 * (a - c) / denom
    } else {
        free_lag as f32
    };
    Some(((60.0 / (refined * hop_s) * 10.0).round() / 10.0, false))
}

const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Krumhansl-Schmuckler key estimate, when one key clearly fits.
fn detect_key(chroma: &[f32; 12]) -> Option<(usize, bool)> {
    let corr = |profile: &[f32; 12], root: usize| {
        let p: Vec<f32> = (0..12).map(|i| profile[(i + 12 - root) % 12]).collect();
        let (mp, mc) = (mean(&p), mean(chroma));
        let num: f32 = (0..12).map(|i| (p[i] - mp) * (chroma[i] - mc)).sum();
        let den = (p.iter().map(|v| (v - mp).powi(2)).sum::<f32>()
            * chroma.iter().map(|v| (v - mc).powi(2)).sum::<f32>())
        .sqrt();
        if den > 0.0 { num / den } else { 0.0 }
    };
    let mut scores: Vec<((usize, bool), f32)> = (0..12)
        .flat_map(|root| {
            [
                ((root, false), corr(&MAJOR, root)),
                ((root, true), corr(&MINOR, root)),
            ]
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    (scores[0].1 >= 0.6).then_some(scores[0].0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: u32 = 44_100;

    fn decaying_tone(hz: f32, seconds: f32, decay: f32) -> Vec<f32> {
        (0..(seconds * SR as f32) as usize)
            .map(|i| {
                let t = i as f32 / SR as f32;
                (-t * decay).exp()
                    * (0..4)
                        .map(|h| (2.0 * PI * hz * (h + 1) as f32 * t).sin() / (h + 1) as f32)
                        .sum::<f32>()
                    * 0.5
            })
            .collect()
    }

    fn noise_burst(seconds: f32, decay: f32) -> Vec<f32> {
        let mut seed = 7u32;
        (0..(seconds * SR as f32) as usize)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
                n * (-(i as f32 / SR as f32) * decay).exp()
            })
            .collect()
    }

    #[test]
    fn descriptor_round_trips_through_bytes() {
        let a = analyze(&decaying_tone(220.0, 0.5, 4.0), SR, 0.5);
        assert_eq!(a.descriptor.0.len(), DESCRIPTOR_DIMS);
        assert_eq!(
            Descriptor::from_bytes(&a.descriptor.to_bytes()),
            Some(a.descriptor)
        );
    }

    #[test]
    fn hears_pitch_of_tones_and_none_in_noise() {
        let tone = analyze(&decaying_tone(110.0, 1.0, 2.0), SR, 1.0);
        assert!((tone.pitch.unwrap() - 45.0).abs() < 0.3, "{:?}", tone.pitch);
        assert!(analyze(&noise_burst(0.5, 8.0), SR, 0.5).pitch.is_none());
    }

    #[test]
    fn stays_finite_on_silence_and_tiny_files() {
        for mono in [vec![0.0; 4410], vec![0.3; 10], vec![0.0; 1]] {
            let a = analyze(&mono, SR, mono.len() as f64 / SR as f64);
            assert!(a.descriptor.0.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn finds_tempo_of_a_click_loop() {
        // Two bars at 125 BPM: 3.84 s, a click on every beat.
        let beat = (60.0 / 125.0 * SR as f32) as usize;
        let mut mono = vec![0f32; beat * 8];
        for b in 0..8 {
            for (i, v) in noise_burst(0.08, 60.0).into_iter().enumerate() {
                mono[b * beat + i] += v;
            }
            for (i, v) in decaying_tone(80.0, 0.3, 6.0).into_iter().enumerate() {
                if b * beat + i < mono.len() {
                    mono[b * beat + i] += v;
                }
            }
        }
        let a = analyze(&mono, SR, mono.len() as f64 / SR as f64);
        assert!((a.tempo.unwrap() - 125.0).abs() < 0.5, "{:?}", a.tempo);
    }

    #[test]
    fn reads_a_minor_chord_as_a_minor() {
        let len = SR as usize * 2;
        let mono: Vec<f32> = (0..len)
            .map(|i| {
                let t = i as f32 / SR as f32;
                [220.0, 261.63, 329.63, 440.0, 523.25]
                    .iter()
                    .map(|f| (2.0 * PI * f * t).sin() * 0.15)
                    .sum()
            })
            .collect();
        assert_eq!(analyze(&mono, SR, 2.0).key, Some((9, true)));
    }
}
