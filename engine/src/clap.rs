//! Listening with CLAP (LAION's contrastive language-audio model, in the
//! ONNX export of its audio half): it turns a sound into a point where
//! sounds people would describe alike lie close together.

use std::path::Path;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use tract_onnx::prelude::*;

pub const DIMS: usize = 512;
/// How much of a sound it hears: the start.
pub const SECONDS: f64 = 10.0;
const RATE: u32 = 48_000;
const SAMPLES: usize = RATE as usize * SECONDS as usize;
const N_FFT: usize = 1024;
const HOP: usize = 480;
const MELS: usize = 64;
const FRAMES: usize = SAMPLES / HOP + 1;

/// Where CLAP puts a sound, at a byte a dimension: its unit-length point
/// over `scale`.
#[derive(Clone, Debug, PartialEq)]
pub struct Embedding {
    scale: f32,
    values: Box<[i8]>,
}

impl Embedding {
    /// Of a point the model made, scaled to unit length.
    pub fn new(point: &[f32]) -> Self {
        let norm = point.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        let max = point.iter().fold(0f32, |m, x| m.max(x.abs())).max(1e-12);
        let scale = max / norm / 127.0;
        Self {
            scale,
            values: point
                .iter()
                .map(|x| (x / norm / scale).round() as i8)
                .collect(),
        }
    }

    /// The cosine distance: 0 for the same sound (rounding can't take it
    /// below), about 0.85 for two unrelated ones.
    pub fn distance(&self, other: &Embedding) -> f32 {
        let (a, b) = (&self.values, &other.values);
        // Summed in lanes, so it vectorizes.
        let mut lanes = [0i32; 16];
        for (x, y) in a.as_chunks::<16>().0.iter().zip(b.as_chunks::<16>().0) {
            for k in 0..16 {
                lanes[k] += x[k] as i32 * y[k] as i32;
            }
        }
        (1.0 - lanes.iter().sum::<i32>() as f32 * self.scale * other.scale).max(0.0)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.scale.to_le_bytes().to_vec();
        out.extend(self.values.iter().map(|&v| v as u8));
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        (b.len() == 4 + DIMS).then(|| Self {
            scale: f32::from_le_bytes(b[..4].try_into().unwrap()),
            values: b[4..].iter().map(|&v| v as i8).collect(),
        })
    }
}

pub struct Clap {
    model: Arc<TypedRunnableModel>,
    features: Features,
}

impl Clap {
    pub fn load(path: &Path) -> Result<Self, String> {
        let model = (|| {
            tract_onnx::onnx()
                .with_ignore_value_info(true)
                .with_ignore_output_shapes(true)
                .model_for_path(path)?
                .with_input_fact(0, f32::fact([1, 1, FRAMES, MELS]).into())?
                .into_optimized()?
                .into_runnable()
        })()
        .map_err(|e| format!("can't load {}: {e}", path.display()))?;
        Ok(Self {
            model,
            features: Features::new(),
        })
    }

    /// Where its first ten seconds are, repeated to fill them if it's
    /// shorter.
    pub fn embed(&self, mono: &[f32], sample_rate: u32) -> Result<Embedding, String> {
        let features = self.features.of(&resample(mono, sample_rate, RATE));
        let run = || {
            let input = Tensor::from_shape(&[1, 1, FRAMES, MELS], &features)?;
            let out = self.model.run(tvec!(input.into_tvalue()))?;
            let point: Vec<f32> = out[0]
                .to_plain_array_view::<f32>()?
                .iter()
                .copied()
                .collect();
            TractResult::Ok(point)
        };
        let point = run().map_err(|e| e.to_string())?;
        Ok(Embedding::new(&point))
    }
}

/// What the model reads: log-mel frames, as the CLAP feature extractor in
/// Hugging Face's transformers makes them.
struct Features {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// `MELS` triangles over the FFT bins, Slaney-style, as librosa makes them.
    mel: Vec<Vec<(usize, f32)>>,
}

impl Features {
    fn new() -> Self {
        Self {
            fft: FftPlanner::new().plan_fft_forward(N_FFT),
            // Periodic Hann.
            window: (0..N_FFT)
                .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
                .collect(),
            mel: mel_bank(),
        }
    }

    /// `FRAMES` by `MELS` of them, for a sound at `RATE`.
    fn of(&self, x: &[f32]) -> Vec<f32> {
        let mut padded = vec![0.0f32; SAMPLES];
        let x = &x[..x.len().min(SAMPLES)];
        if !x.is_empty() {
            let repeats = SAMPLES / x.len();
            for r in 0..repeats {
                padded[r * x.len()..(r + 1) * x.len()].copy_from_slice(x);
            }
        }
        // Centered frames, the edges mirrored.
        let half = N_FFT / 2;
        let mut centered = Vec::with_capacity(SAMPLES + N_FFT);
        centered.extend((1..=half).rev().map(|i| padded[i]));
        centered.extend_from_slice(&padded);
        centered.extend((1..=half).map(|i| padded[SAMPLES - 1 - i]));
        let mut out = Vec::with_capacity(FRAMES * MELS);
        let mut buf = vec![Complex32::default(); N_FFT];
        let mut power = vec![0.0f32; N_FFT / 2 + 1];
        for f in 0..FRAMES {
            let frame = &centered[f * HOP..f * HOP + N_FFT];
            for (b, (s, w)) in buf.iter_mut().zip(frame.iter().zip(&self.window)) {
                *b = Complex32::new(s * w, 0.0);
            }
            self.fft.process(&mut buf);
            for (p, c) in power.iter_mut().zip(&buf) {
                *p = c.norm_sqr();
            }
            for tri in &self.mel {
                let e: f32 = tri.iter().map(|&(bin, w)| power[bin] * w).sum();
                out.push(10.0 * e.max(1e-10).log10());
            }
        }
        out
    }
}

fn hz_to_mel(hz: f64) -> f64 {
    if hz < 1000.0 {
        3.0 * hz / 200.0
    } else {
        15.0 + (hz / 1000.0).ln() * 27.0 / 6.4f64.ln()
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    if mel < 15.0 {
        200.0 * mel / 3.0
    } else {
        1000.0 * ((mel - 15.0) * 6.4f64.ln() / 27.0).exp()
    }
}

fn mel_bank() -> Vec<Vec<(usize, f32)>> {
    let bins = N_FFT / 2 + 1;
    let (lo, hi) = (hz_to_mel(50.0), hz_to_mel(14_000.0));
    let edges: Vec<f64> = (0..MELS + 2)
        .map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (MELS + 1) as f64))
        .collect();
    (0..MELS)
        .map(|m| {
            let (l, c, r) = (edges[m], edges[m + 1], edges[m + 2]);
            let scale = 2.0 / (r - l);
            (0..bins)
                .filter_map(|b| {
                    let hz = b as f64 * (RATE / 2) as f64 / (bins - 1) as f64;
                    let w = ((hz - l) / (c - l)).min((r - hz) / (r - c)).max(0.0) * scale;
                    (w > 0.0).then_some((b, w as f32))
                })
                .collect()
        })
        .collect()
}

/// `x` at `to` Hz, through a low-pass at the lower rate's Nyquist so
/// nothing folds back: a Hann-windowed sinc, as torchaudio does it.
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    // Zero crossings each side.
    const WIDTH: f64 = 6.0;
    let g = gcd(from, to);
    let (up, down) = ((to / g) as usize, (from / g) as usize);
    // Output `i` lies `i * down / up` samples in: past sample `j` by one of
    // `up` fractions, each with its own weights for the samples around.
    let cutoff = 0.99 * (up as f64 / down as f64).min(1.0);
    let reach = (WIDTH / cutoff).ceil() as usize;
    let taps = 2 * reach;
    let weights: Vec<f32> = (0..up)
        .flat_map(|p| {
            (0..taps).map(move |k| {
                let u = (k as f64 + 1.0 - reach as f64 - p as f64 / up as f64) * cutoff;
                if u.abs() >= WIDTH {
                    return 0.0;
                }
                let pi = std::f64::consts::PI;
                let sinc = if u == 0.0 {
                    1.0
                } else {
                    (pi * u).sin() / (pi * u)
                };
                let hann = (pi * u / WIDTH / 2.0).cos().powi(2);
                (cutoff * sinc * hann) as f32
            })
        })
        .collect();
    let silence = std::iter::repeat_n(0.0, reach);
    let padded: Vec<f32> = silence
        .clone()
        .chain(x.iter().copied())
        .chain(silence)
        .collect();
    (0..x.len() * up / down)
        .map(|i| {
            let (j, p) = (i * down / up, i * down % up);
            let around = &padded[j + 1..][..taps];
            let w = &weights[p * taps..][..taps];
            around.iter().zip(w).map(|(a, b)| a * b).sum()
        })
        .collect()
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..DIMS)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                s as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn embeddings_keep_their_distances() {
        let (a, b) = (point(1), point(2));
        let cosine = {
            let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
            1.0 - dot / (norm(&a) * norm(&b))
        };
        let (ea, eb) = (Embedding::new(&a), Embedding::new(&b));
        assert!((ea.distance(&eb) - cosine).abs() < 0.01);
        assert!((0.0..0.01).contains(&ea.distance(&ea)));
        assert_eq!(Embedding::from_bytes(&ea.to_bytes()), Some(ea));
        assert_eq!(Embedding::from_bytes(&[]), None);
    }

    #[test]
    fn a_tone_lights_its_own_band() {
        // A short 1 kHz blip, repeated to fill the ten seconds.
        let tone: Vec<f32> = (0..4800)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / RATE as f32).sin())
            .collect();
        let features = Features::new().of(&tone);
        assert_eq!(features.len(), FRAMES * MELS);
        let frame = &features[500 * MELS..501 * MELS];
        let loudest = (0..MELS)
            .max_by(|&a, &b| frame[a].total_cmp(&frame[b]))
            .unwrap();
        let centre = |m: usize| {
            let (lo, hi) = (hz_to_mel(50.0), hz_to_mel(14_000.0));
            mel_to_hz(lo + (hi - lo) * (m + 1) as f64 / (MELS + 1) as f64)
        };
        assert!((centre(loudest) / 1000.0).ln().abs() < 0.15);
    }

    #[test]
    fn resampling_keeps_what_fits_and_folds_nothing_back() {
        let tone = |hz: f32, rate: u32| -> Vec<f32> {
            (0..rate as usize / 10)
                .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
                .collect()
        };
        // Away from the edges.
        let rms = |x: &[f32]| {
            let mid = &x[x.len() / 4..x.len() * 3 / 4];
            (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt()
        };
        let up = resample(&tone(1000.0, 44_100), 44_100, RATE);
        assert_eq!(up.len(), RATE as usize / 10);
        assert!((rms(&up) - 0.5f32.sqrt()).abs() < 0.01);
        // Taking every other sample would make this 12 kHz.
        let down = resample(&tone(36_000.0, 96_000), 96_000, RATE);
        assert!(rms(&down) < 0.01);
        assert!((rms(&resample(&tone(5000.0, 96_000), 96_000, RATE)) - 0.5f32.sqrt()).abs() < 0.01);
    }
}
