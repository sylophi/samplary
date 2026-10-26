use std::f32::consts::PI;
use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// Short-time power spectra over centered, Hann-windowed frames: frame `t`
/// is centered on sample `t * hop`.
pub struct Stft {
    pub n_fft: usize,
    pub hop: usize,
    window: Vec<f32>,
    norm: f32,
    fft: Arc<dyn Fft<f32>>,
}

impl Stft {
    pub fn new(n_fft: usize, hop: usize) -> Self {
        let window: Vec<f32> = (0..n_fft)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n_fft as f32).cos())
            .collect();
        // A full-scale sine lands near 0 dB in its peak bin.
        let half_sum = window.iter().sum::<f32>() / 2.0;
        let fft = FftPlanner::new().plan_fft_forward(n_fft);
        Self {
            n_fft,
            hop,
            window,
            norm: 1.0 / (half_sum * half_sum),
            fft,
        }
    }

    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    pub fn frame_count(&self, len: usize) -> usize {
        len / self.hop + 1
    }

    pub fn bin_hz(&self, sr: f32) -> f32 {
        sr / self.n_fft as f32
    }

    pub fn for_each_frame(&self, x: &[f32], mut f: impl FnMut(usize, &[f32])) {
        let n = self.n_fft;
        let half = (n / 2) as isize;
        let mut buf = vec![Complex32::default(); n];
        let mut scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        let mut power = vec![0f32; self.bins()];
        for t in 0..self.frame_count(x.len()) {
            let start = (t * self.hop) as isize - half;
            for (i, c) in buf.iter_mut().enumerate() {
                let j = start + i as isize;
                let s = if j >= 0 && (j as usize) < x.len() {
                    x[j as usize]
                } else {
                    0.0
                };
                *c = Complex32::new(s * self.window[i], 0.0);
            }
            self.fft.process_with_scratch(&mut buf, &mut scratch);
            for (p, c) in power.iter_mut().zip(&buf) {
                *p = c.norm_sqr() * self.norm;
            }
            f(t, &power);
        }
    }
}

fn hz_to_mel(f: f32) -> f32 {
    2595.0 * (1.0 + f / 700.0).log10()
}

fn mel_to_hz(m: f32) -> f32 {
    700.0 * (10f32.powf(m / 2595.0) - 1.0)
}

/// Triangular mel filters, each normalized to average the power it covers.
pub struct MelBank {
    filters: Vec<(usize, Vec<f32>)>,
}

impl MelBank {
    pub fn new(n_mels: usize, bins: usize, bin_hz: f32, fmin: f32, fmax: f32) -> Self {
        let (lo, hi) = (hz_to_mel(fmin), hz_to_mel(fmax));
        let edges: Vec<f32> = (0..n_mels + 2)
            .map(|i| mel_to_hz(lo + (hi - lo) * i as f32 / (n_mels + 1) as f32))
            .collect();
        let filters = (0..n_mels)
            .map(|m| {
                let (a, b, c) = (edges[m], edges[m + 1], edges[m + 2]);
                let first = ((a / bin_hz).floor() as usize).min(bins - 1);
                let last = ((c / bin_hz).ceil() as usize).min(bins - 1);
                let mut w: Vec<f32> = (first..=last)
                    .map(|k| {
                        let f = k as f32 * bin_hz;
                        if f <= b {
                            (f - a) / (b - a)
                        } else {
                            (c - f) / (c - b)
                        }
                        .max(0.0)
                    })
                    .collect();
                // Bands narrower than a bin still read the bin they sit in.
                if w.iter().all(|&v| v == 0.0) {
                    let k = ((b / bin_hz).round() as usize).clamp(first, last);
                    w[k - first] = 1.0;
                }
                let sum: f32 = w.iter().sum();
                w.iter_mut().for_each(|v| *v /= sum);
                (first, w)
            })
            .collect();
        Self { filters }
    }

    pub fn len(&self) -> usize {
        self.filters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    pub fn apply(&self, power: &[f32], out: &mut [f32]) {
        for ((first, w), o) in self.filters.iter().zip(out.iter_mut()) {
            *o = power[*first..*first + w.len()]
                .iter()
                .zip(w)
                .map(|(p, w)| p * w)
                .sum();
        }
    }
}

/// Orthonormal DCT-II rows `1..=n_coeffs` (c0, overall level, is dropped).
pub struct Dct {
    rows: Vec<Vec<f32>>,
}

impl Dct {
    pub fn new(n_in: usize, n_coeffs: usize) -> Self {
        let scale = (2.0 / n_in as f32).sqrt();
        let rows = (1..=n_coeffs)
            .map(|k| {
                (0..n_in)
                    .map(|m| scale * (PI * k as f32 * (m as f32 + 0.5) / n_in as f32).cos())
                    .collect()
            })
            .collect();
        Self { rows }
    }

    pub fn apply(&self, x: &[f32], out: &mut [f32]) {
        for (row, o) in self.rows.iter().zip(out.iter_mut()) {
            *o = row.iter().zip(x).map(|(a, b)| a * b).sum();
        }
    }
}

pub fn db(power: f32) -> f32 {
    10.0 * (power + 1e-12).log10()
}

pub fn mean(x: &[f32]) -> f32 {
    if x.is_empty() {
        0.0
    } else {
        x.iter().sum::<f32>() / x.len() as f32
    }
}

pub fn stdev(x: &[f32]) -> f32 {
    let m = mean(x);
    mean(&x.iter().map(|v| (v - m) * (v - m)).collect::<Vec<_>>()).sqrt()
}

pub fn median(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let mut v = x.to_vec();
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Windowed-sinc low-pass and decimation by an integer factor.
pub fn decimate(x: &[f32], factor: usize) -> Vec<f32> {
    if factor <= 1 {
        return x.to_vec();
    }
    let taps = 16 * factor + 1;
    let half = (taps / 2) as isize;
    let cutoff = 0.45 / factor as f32;
    let kernel: Vec<f32> = (0..taps)
        .map(|i| {
            let n = i as f32 - half as f32;
            let sinc = if n == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * n).sin() / (PI * n)
            };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f32 / (taps - 1) as f32).cos()
                + 0.08 * (4.0 * PI * i as f32 / (taps - 1) as f32).cos();
            sinc * w
        })
        .collect();
    let sum: f32 = kernel.iter().sum();
    (0..x.len().div_ceil(factor))
        .map(|o| {
            let center = (o * factor) as isize;
            let mut acc = 0.0;
            for (i, k) in kernel.iter().enumerate() {
                let j = center + i as isize - half;
                if j >= 0 && (j as usize) < x.len() {
                    acc += k * x[j as usize];
                }
            }
            acc / sum
        })
        .collect()
}
