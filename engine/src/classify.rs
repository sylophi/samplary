//! Categories and subcategories from the sound, for samples whose names
//! say nothing.

use rayon::prelude::*;

use crate::analysis::Descriptor;
use crate::clap::Embedding;
use crate::index::{Point, Query, Scales, Weights};
use crate::library::Sample;

pub const EQUAL: Weights = [1.0; 4];
/// How much what CLAP hears counts in a guess, against the analysis: it
/// tells what a sound is apart best, and the analysis how long and how
/// high it rings.
pub const LISTENING: f32 = 1.0 / 3.0;
/// How much of the vote a category needs.
pub const CATEGORY_AGREEMENT: f32 = 0.5;
/// Subcategories split finer, and a wrong one is worse than none: they need
/// more of it.
pub const SUBCATEGORY_AGREEMENT: f32 = 0.7;
const K: usize = 7;
/// Comparing against every labeled sample gets slow on big libraries. This
/// many, spread across them, is plenty to vote.
const MAX_POOL: usize = 4000;

/// How far apart samples are, to guesses.
#[derive(Clone, Copy)]
pub struct Space<'a> {
    pub scales: &'a Scales,
    /// How far apart CLAP typically hears two samples.
    pub heard: f32,
}

/// A sample as guesses hear it.
#[derive(Clone, Copy)]
pub struct Sound<'a> {
    pub descriptor: &'a Descriptor,
    pub embedding: Option<&'a Embedding>,
}

impl<'a> Sound<'a> {
    pub fn of(s: &'a Sample) -> Self {
        Self {
            descriptor: &s.descriptor,
            embedding: s.embedding.as_ref(),
        }
    }
}

/// Labeled samples to vote with: where each is, its label, and its point.
struct Pool<'a> {
    positions: Vec<usize>,
    labels: Vec<u16>,
    points: Vec<(Point, Option<&'a Embedding>)>,
}

impl<'a> Pool<'a> {
    fn new(
        space: Space,
        sounds: &[Sound<'a>],
        labels: &[Option<u16>],
        positions: Vec<usize>,
    ) -> Self {
        Self {
            labels: positions.iter().map(|&i| labels[i].unwrap()).collect(),
            points: positions
                .par_iter()
                .map(|&i| {
                    (
                        space.scales.point(sounds[i].descriptor),
                        sounds[i].embedding,
                    )
                })
                .collect(),
            positions,
        }
    }
}

/// One sound to measure the pool against.
struct Probe<'a> {
    query: Query<'a>,
    embedding: Option<&'a Embedding>,
    heard: f32,
}

impl<'a> Probe<'a> {
    fn new(space: Space<'a>, sound: Sound<'a>) -> Self {
        Self {
            query: space.scales.query(sound.descriptor),
            embedding: sound.embedding,
            heard: space.heard,
        }
    }

    /// The distance to `other`, both as the analysis and as CLAP hear them
    /// when CLAP has heard both. Infinity once it's sure to be at least
    /// `limit`.
    fn distance(&self, other: &(Point, Option<&Embedding>), weights: &Weights, limit: f32) -> f32 {
        let (point, embedding) = other;
        match self.embedding.zip(*embedding) {
            Some((a, b)) => {
                // The analysis's part often rules it out alone.
                let analyzed = self
                    .query
                    .distance(point, weights, limit / (1.0 - LISTENING));
                if analyzed.is_infinite() {
                    return analyzed;
                }
                (1.0 - LISTENING) * analyzed + LISTENING * a.distance(b) / self.heard
            }
            None => self.query.distance(point, weights, limit),
        }
    }
}

/// The median distance CLAP hears between random pairs of `heard`.
pub fn typical_heard(heard: &[&Embedding]) -> f32 {
    let n = heard.len();
    if n < 2 {
        return 1.0;
    }
    let mut seed = 0x9e37_79b9u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % n as u32) as usize
    };
    let distances: Vec<f32> = (0..4000.min(n * (n - 1) / 2))
        .map(|_| {
            let a = next();
            let b = (a + 1 + next() % (n - 1)) % n;
            heard[a].distance(heard[b])
        })
        .collect();
    crate::dsp::median(&distances).max(1e-3)
}

/// Labels each unlabeled sample after its nearest labeled neighbours, when
/// at least `agreement` of their vote goes to one label. Gives up as soon
/// as `cancelled` says so.
pub fn propagate(
    space: Space,
    sounds: &[Sound],
    labels: &[Option<u16>],
    agreement: f32,
    cancelled: impl Fn() -> bool + Sync,
) -> Option<Vec<Option<u16>>> {
    let labeled = labeled_in(labels);
    if labeled.len() < 2 * K {
        return Some(vec![None; labels.len()]);
    }
    let stride = labeled.len().div_ceil(MAX_POOL);
    let pool = Pool::new(
        space,
        sounds,
        labels,
        labeled.into_iter().step_by(stride).collect(),
    );
    (0..labels.len())
        .into_par_iter()
        .map(|i| {
            if cancelled() {
                return None;
            }
            Some(match labels[i] {
                Some(_) => None,
                None => vote(
                    &Probe::new(space, sounds[i]),
                    &pool,
                    |_| false,
                    &EQUAL,
                    agreement,
                ),
            })
        })
        .collect()
}

/// For each labeled sample, its label and what its neighbours outside its
/// group would have called it.
pub fn leave_one_out(
    space: Space,
    sounds: &[Sound],
    labels: &[Option<u16>],
    groups: &[usize],
    weights: &Weights,
    agreement: f32,
) -> Vec<(u16, Option<u16>)> {
    let pool = Pool::new(space, sounds, labels, labeled_in(labels));
    pool.positions
        .par_iter()
        .zip(&pool.labels)
        .map(|(&i, &label)| {
            let probe = Probe::new(space, sounds[i]);
            let apart = |j| groups[j] == groups[i];
            (label, vote(&probe, &pool, apart, weights, agreement))
        })
        .collect()
}

fn labeled_in(labels: &[Option<u16>]) -> Vec<usize> {
    (0..labels.len()).filter(|&i| labels[i].is_some()).collect()
}

/// The label most of the `K` nearest in `pool` share, if it has at least
/// `agreement` of their vote. Ties go to the nearer: the earlier in the
/// pool, then the label met first.
fn vote(
    probe: &Probe,
    pool: &Pool,
    exclude: impl Fn(usize) -> bool,
    weights: &Weights,
    agreement: f32,
) -> Option<u16> {
    // The nearest so far, nearest first, as labels and distances.
    let mut near = [(0u16, 0f32); K];
    let mut found = 0;
    for ((&j, &label), point) in pool.positions.iter().zip(&pool.labels).zip(&pool.points) {
        if exclude(j) {
            continue;
        }
        let limit = if found == K {
            near[K - 1].1
        } else {
            f32::INFINITY
        };
        let d = probe.distance(point, weights, limit);
        if found == K && d >= limit {
            continue;
        }
        let mut at = if found < K {
            found += 1;
            found - 1
        } else {
            K - 1
        };
        while at > 0 && near[at - 1].1 > d {
            near[at] = near[at - 1];
            at -= 1;
        }
        near[at] = (label, d);
    }
    let mut votes: Vec<(u16, f32)> = Vec::with_capacity(K);
    let mut total = 0.0;
    for &(label, d) in &near[..found] {
        let weight = 1.0 / (d + 0.05);
        match votes.iter_mut().find(|v| v.0 == label) {
            Some(v) => v.1 += weight,
            None => votes.push((label, weight)),
        }
        total += weight;
    }
    let mut best: Option<(u16, f32)> = None;
    for v in votes {
        if best.is_none_or(|b| v.1 > b.1) {
            best = Some(v);
        }
    }
    let (label, score) = best?;
    (score / total >= agreement).then_some(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DESCRIPTOR_DIMS;
    use crate::clap::DIMS;

    fn random(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed as f32 / u32::MAX as f32
    }

    fn sounds<'a>(descriptors: &'a [Descriptor], embeddings: &'a [Embedding]) -> Vec<Sound<'a>> {
        descriptors
            .iter()
            .enumerate()
            .map(|(i, descriptor)| Sound {
                descriptor,
                embedding: embeddings.get(i),
            })
            .collect()
    }

    #[test]
    fn labels_after_neighbours() {
        let mut seed = 3;
        let mut centre = || Descriptor((0..DESCRIPTOR_DIMS).map(|_| random(&mut seed)).collect());
        let (kick, hat) = (centre(), centre());
        let mut seed = 5;
        let mut near = |d: &Descriptor| {
            Descriptor(
                d.0.iter()
                    .map(|v| v + (random(&mut seed) - 0.5) * 0.01)
                    .collect(),
            )
        };
        let mut all = Vec::new();
        let mut labels = Vec::new();
        for _ in 0..8 {
            all.push(near(&kick));
            labels.push(Some(0));
            all.push(near(&hat));
            labels.push(Some(3));
        }
        all.push(near(&hat));
        labels.push(None);
        let refs: Vec<&Descriptor> = all.iter().collect();
        let scales = Scales::new(&refs);
        let space = Space {
            scales: &scales,
            heard: 1.0,
        };
        let sounds = sounds(&all, &[]);
        let guesses = propagate(space, &sounds, &labels, 0.5, || false).unwrap();
        assert_eq!(guesses.last(), Some(&Some(3)));
        assert!(guesses[..16].iter().all(Option::is_none));
        assert_eq!(propagate(space, &sounds, &labels, 0.5, || true), None);

        let groups: Vec<usize> = (0..labels.len()).collect();
        let right = leave_one_out(space, &sounds, &labels, &groups, &EQUAL, 0.5)
            .iter()
            .filter(|(label, guess)| Some(*label) == *guess)
            .count();
        assert_eq!(right, 16);
    }

    #[test]
    fn hears_what_the_analysis_cant_tell_apart() {
        // Two kinds the analysis hears alike, and CLAP apart.
        let mut seed = 7;
        let base = Descriptor((0..DESCRIPTOR_DIMS).map(|_| random(&mut seed)).collect());
        let mut point = |centre: u32| -> Embedding {
            let mut s = centre;
            let p: Vec<f32> = (0..DIMS)
                .map(|_| random(&mut s) - 0.5 + (random(&mut seed) - 0.5) * 0.1)
                .collect();
            Embedding::new(&p)
        };
        let mut descriptors = Vec::new();
        let mut embeddings = Vec::new();
        let mut labels = Vec::new();
        let d = || base.clone();
        for i in 0..16 {
            descriptors.push(d());
            embeddings.push(point(if i % 2 == 0 { 1 } else { 2 }));
            labels.push(Some(i % 2));
        }
        descriptors.push(d());
        embeddings.push(point(2));
        labels.push(None);
        let refs: Vec<&Descriptor> = descriptors.iter().collect();
        let scales = Scales::new(&refs);
        let space = Space {
            scales: &scales,
            heard: 1.0,
        };
        let guesses = propagate(
            space,
            &sounds(&descriptors, &embeddings),
            &labels,
            0.5,
            || false,
        )
        .unwrap();
        assert_eq!(guesses.last(), Some(&Some(1)));
    }

    #[test]
    fn too_few_labels_guess_nothing() {
        let all = vec![Descriptor(vec![0.5; DESCRIPTOR_DIMS]); 5];
        let refs: Vec<&Descriptor> = all.iter().collect();
        let scales = Scales::new(&refs);
        let space = Space {
            scales: &scales,
            heard: 1.0,
        };
        let labels = [Some(1), None, None, None, None];
        assert_eq!(
            propagate(space, &sounds(&all, &[]), &labels, 0.5, || false),
            Some(vec![None; 5])
        );
    }
}
