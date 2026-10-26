/// YIN fundamental-frequency estimation (de Cheveigné & Kawahara, 2002).
pub struct Yin {
    sr: f32,
    window: usize,
    tau_min: usize,
    tau_max: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct PitchFrame {
    pub hz: f32,
    /// 1 minus the normalized difference at the chosen lag: near 1 for a
    /// clean periodic signal, low for noise.
    pub clarity: f32,
}

impl Yin {
    pub fn new(sr: f32, fmin: f32, fmax: f32, window: usize) -> Self {
        Self {
            sr,
            window,
            tau_min: ((sr / fmax).floor() as usize).max(2),
            tau_max: (sr / fmin).ceil() as usize,
        }
    }

    /// Samples one estimate reads, starting at its offset.
    pub fn span(&self) -> usize {
        self.window + self.tau_max + 1
    }

    pub fn estimate(&self, x: &[f32]) -> Option<PitchFrame> {
        if x.len() < self.span() {
            return None;
        }
        let w = self.window;
        let mut d = vec![0f32; self.tau_max + 1];
        for (tau, slot) in d.iter_mut().enumerate().skip(1) {
            *slot = x[..w]
                .iter()
                .zip(&x[tau..tau + w])
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
        }
        // Cumulative mean normalized difference.
        let mut running = 0.0;
        let mut cmnd = vec![1f32; self.tau_max + 1];
        for tau in 1..=self.tau_max {
            running += d[tau];
            cmnd[tau] = if running > 0.0 {
                d[tau] * tau as f32 / running
            } else {
                1.0
            };
        }
        let threshold = 0.15;
        let mut best = None;
        let mut tau = self.tau_min;
        while tau < self.tau_max {
            if cmnd[tau] < threshold {
                while tau + 1 < self.tau_max && cmnd[tau + 1] < cmnd[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let tau = best.unwrap_or_else(|| {
            (self.tau_min..self.tau_max)
                .min_by(|&a, &b| cmnd[a].total_cmp(&cmnd[b]))
                .unwrap_or(self.tau_min)
        });
        // Parabolic interpolation around the chosen lag.
        let refined = if tau > 1 && tau < self.tau_max {
            let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 {
                tau as f32 + 0.5 * (a - c) / denom
            } else {
                tau as f32
            }
        } else {
            tau as f32
        };
        Some(PitchFrame {
            hz: self.sr / refined,
            clarity: (1.0 - cmnd[tau]).clamp(0.0, 1.0),
        })
    }
}

pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn pitch_class_name(pc: usize) -> &'static str {
    NOTE_NAMES[pc % 12]
}

/// "A4", "C#2", ... for a (fractional) MIDI note, rounded to the nearest.
pub fn note_name(midi: f32) -> String {
    let n = midi.round() as i32;
    format!(
        "{}{}",
        NOTE_NAMES[n.rem_euclid(12) as usize],
        n.div_euclid(12) - 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, sr: f32, len: usize, harmonics: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                (1..=harmonics)
                    .map(|h| {
                        (2.0 * std::f32::consts::PI * hz * h as f32 * i as f32 / sr).sin()
                            / h as f32
                    })
                    .sum()
            })
            .collect()
    }

    #[test]
    fn finds_fundamentals_across_the_range() {
        let sr = 11025.0;
        let yin = Yin::new(sr, 30.0, 2000.0, 1024);
        for hz in [41.2, 55.0, 110.0, 261.6, 440.0, 1046.5] {
            let p = yin.estimate(&tone(hz, sr, yin.span(), 6)).unwrap();
            assert!(
                (hz_to_midi(p.hz) - hz_to_midi(hz)).abs() < 0.2,
                "{hz} -> {}",
                p.hz
            );
            assert!(p.clarity > 0.8);
        }
    }

    #[test]
    fn noise_is_unclear() {
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..4000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
            })
            .collect();
        let yin = Yin::new(11025.0, 30.0, 2000.0, 1024);
        assert!(yin.estimate(&noise).unwrap().clarity < 0.6);
    }

    #[test]
    fn names_notes() {
        assert_eq!(note_name(69.0), "A4");
        assert_eq!(note_name(60.4), "C4");
        assert_eq!(note_name(25.0), "C#1");
    }
}
