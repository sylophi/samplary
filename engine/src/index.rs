//! How alike two analyzed sounds are, aspect by aspect.

use std::ops::Range;

use rayon::prelude::*;

use crate::analysis::{Descriptor, ENV_DIMS, PITCH_DIMS, SPEC_DIMS, TIMBRE_DIMS};

/// Spectrum, timbre, pitch, amplitude: how much each counts in a search.
pub type Weights = [f32; 4];

const VECTOR_DIMS: usize = ENV_DIMS + SPEC_DIMS + TIMBRE_DIMS;
const ENV: Range<usize> = 0..ENV_DIMS;
const SPEC: Range<usize> = ENV_DIMS..ENV_DIMS + SPEC_DIMS;
const TIMBRE: Range<usize> = ENV_DIMS + SPEC_DIMS..VECTOR_DIMS;
// Within the pitch part: how much of the sound has a clear fundamental
// (0..1), its MIDI pitch, how it moves, and its chroma.
const VOICED: usize = 0;
const MIDI: usize = 1;
const CONTOUR: Range<usize> = 2..PITCH_DIMS - 12;
const CHROMA: Range<usize> = PITCH_DIMS - 12..PITCH_DIMS;

/// What distances within a library are measured in. Differences in each
/// dimension of the descriptors' vectors count in standard deviations
/// across the library, so aspects are comparable. Each aspect's distance
/// counts in how far apart two of its samples typically are.
pub struct Scales {
    inv_std: [f32; VECTOR_DIMS],
    /// Median distance between two random samples, per aspect.
    typical: [f32; 4],
}

/// A descriptor as distances see it: its vectors scaled by a library's
/// spread, its pitch part as analyzed.
#[derive(Clone)]
pub struct Point {
    z: [f32; VECTOR_DIMS],
    pitch: [f32; PITCH_DIMS],
}

impl Scales {
    pub fn new(descriptors: &[&Descriptor]) -> Self {
        let n = descriptors.len().max(1) as f64;
        // In fixed chunks, summed in order, so the same library always
        // comes out the same.
        let sum = |f: &(dyn Fn(usize, f32) -> f64 + Sync)| -> [f64; VECTOR_DIMS] {
            let parts: Vec<[f64; VECTOR_DIMS]> = descriptors
                .par_chunks(1024)
                .map(|chunk| {
                    let mut acc = [0f64; VECTOR_DIMS];
                    for d in chunk {
                        for (k, a) in acc.iter_mut().enumerate() {
                            *a += f(k, d.0[k]);
                        }
                    }
                    acc
                })
                .collect();
            let mut total = [0f64; VECTOR_DIMS];
            for p in parts {
                for (t, v) in total.iter_mut().zip(p) {
                    *t += v;
                }
            }
            total
        };
        let mean = sum(&|_, v| v as f64).map(|s| s / n);
        let var = sum(&|k, v| (v as f64 - mean[k]).powi(2)).map(|s| s / n);
        let mut scales = Self {
            inv_std: var.map(|v| (1.0 / v.sqrt().max(1e-3)) as f32),
            typical: [1.0; 4],
        };
        scales.typical = scales.typical_distances(descriptors);
        scales
    }

    pub fn point(&self, d: &Descriptor) -> Point {
        let mut z = [0f32; VECTOR_DIMS];
        for ((z, v), s) in z.iter_mut().zip(&d.0).zip(&self.inv_std) {
            *z = v * s;
        }
        let mut pitch = [0f32; PITCH_DIMS];
        pitch.copy_from_slice(d.pitch());
        Point { z, pitch }
    }

    pub fn query(&self, d: &Descriptor) -> Query<'_> {
        Query {
            scales: self,
            point: self.point(d),
        }
    }

    fn typical_distances(&self, descriptors: &[&Descriptor]) -> [f32; 4] {
        let n = descriptors.len();
        if n < 2 {
            return [1.0; 4];
        }
        let mut seed = 0x9e37_79b9u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % n as u32) as usize
        };
        let mut samples: [Vec<f32>; 4] = Default::default();
        for _ in 0..4000.min(n * (n - 1) / 2) {
            let a = next();
            let b = (a + 1 + next() % (n - 1)) % n;
            let d = raw_distances(&self.point(descriptors[a]), &self.point(descriptors[b]));
            for k in 0..4 {
                samples[k].push(d[k]);
            }
        }
        samples.map(|s| crate::dsp::median(&s).max(1e-3))
    }
}

/// One sound to measure others against.
pub struct Query<'a> {
    scales: &'a Scales,
    point: Point,
}

impl Query<'_> {
    /// Per-aspect distances to `other`, where 1.0 is as far apart as two
    /// random samples from the library typically are.
    pub fn aspects(&self, other: &Point) -> [f32; 4] {
        let raw = raw_distances(&self.point, other);
        std::array::from_fn(|k| raw[k] / self.scales.typical[k])
    }

    /// The weighted distance to `other`, or infinity as soon as it's sure
    /// to be at least `limit`: the cheaper aspects alone often rule it out.
    pub fn distance(&self, other: &Point, weights: &Weights, limit: f32) -> f32 {
        let total: f32 = weights.iter().sum();
        if total <= 0.0 {
            return 0.0;
        }
        let (a, b, t) = (&self.point, other, &self.scales.typical);
        let mut d = pitch_distance(&a.pitch, &b.pitch) / t[2] * weights[2];
        if d / total >= limit {
            return f32::INFINITY;
        }
        d += rms_diff(&a.z[ENV], &b.z[ENV]) / t[3] * weights[3];
        if d / total >= limit {
            return f32::INFINITY;
        }
        d += rms_diff(&a.z[SPEC], &b.z[SPEC]) / t[0] * weights[0];
        if d / total >= limit {
            return f32::INFINITY;
        }
        d += rms_diff(&a.z[TIMBRE], &b.z[TIMBRE]) / t[1] * weights[1];
        d / total
    }

    pub fn distance_to(&self, d: &Descriptor, weights: &Weights) -> f32 {
        self.distance(&self.scales.point(d), weights, f32::INFINITY)
    }
}

/// The root mean square difference, summed in lanes so it vectorizes.
fn rms_diff(a: &[f32], b: &[f32]) -> f32 {
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    let mut lanes = [0f32; 8];
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            let d = x[k] - y[k];
            lanes[k] += d * d;
        }
    }
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| (x - y) * (x - y)).sum();
    ((lanes.iter().sum::<f32>() + tail) / a.len() as f32).sqrt()
}

/// Spectrum, timbre, pitch and amplitude distances.
fn raw_distances(a: &Point, b: &Point) -> [f32; 4] {
    [
        rms_diff(&a.z[SPEC], &b.z[SPEC]),
        rms_diff(&a.z[TIMBRE], &b.z[TIMBRE]),
        pitch_distance(&a.pitch, &b.pitch),
        rms_diff(&a.z[ENV], &b.z[ENV]),
    ]
}

/// Pitch classes always count. When both sounds have a clear fundamental,
/// so do the semitones between them and how each one moves.
fn pitch_distance(a: &[f32; PITCH_DIMS], b: &[f32; PITCH_DIMS]) -> f32 {
    let (mut dot, mut aa, mut bb) = (0f32, 0f32, 0f32);
    for (x, y) in a[CHROMA].iter().zip(&b[CHROMA]) {
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    let norm = (aa * bb).sqrt();
    let mut d = 1.0 - if norm > 0.0 { dot / norm } else { 1.0 };
    d += (a[VOICED] - b[VOICED]).abs();
    match (a[MIDI] > 0.0, b[MIDI] > 0.0) {
        (true, true) => {
            d += (a[MIDI] - b[MIDI]).abs().min(24.0) / 12.0;
            d += rms_diff(&a[CONTOUR], &b[CONTOUR]) / 12.0;
        }
        (false, false) => {}
        _ => d += 1.0,
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DESCRIPTOR_DIMS;

    pub fn random(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed as f32 / u32::MAX as f32
    }

    fn descriptors(n: usize) -> Vec<Descriptor> {
        let mut seed = 7;
        (0..n)
            .map(|_| Descriptor((0..DESCRIPTOR_DIMS).map(|_| random(&mut seed)).collect()))
            .collect()
    }

    #[test]
    fn typical_samples_are_about_one_apart() {
        let all = descriptors(200);
        let refs: Vec<&Descriptor> = all.iter().collect();
        let scales = Scales::new(&refs);
        let query = scales.query(&all[0]);
        assert_eq!(query.distance_to(&all[0], &[1.0; 4]), 0.0);
        let mut far: Vec<f32> = all[1..]
            .iter()
            .map(|d| query.distance_to(d, &[1.0; 4]))
            .collect();
        far.sort_by(f32::total_cmp);
        let median = far[far.len() / 2];
        assert!((0.7..1.3).contains(&median), "{median}");
    }

    #[test]
    fn stops_early_past_the_limit() {
        let all = descriptors(50);
        let refs: Vec<&Descriptor> = all.iter().collect();
        let scales = Scales::new(&refs);
        let query = scales.query(&all[0]);
        let other = scales.point(&all[1]);
        let full = query.distance(&other, &[1.0; 4], f32::INFINITY);
        assert_eq!(query.distance(&other, &[1.0; 4], full * 2.0), full);
        assert_eq!(
            query.distance(&other, &[1.0; 4], full / 100.0),
            f32::INFINITY
        );
    }
}
