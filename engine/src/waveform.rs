use crate::dsp::Stft;

pub const THUMB_COLUMNS: usize = 256;
/// min, max, then low / mid / high band balance.
pub const BYTES_PER_COLUMN: usize = 5;

const LOW_HZ: f32 = 250.0;
const MID_HZ: f32 = 2500.0;

/// Peak extents plus spectral balance per column, for drawing a waveform
/// colored by frequency content (red lows, green mids, blue highs). Peaks
/// are scaled to the loudest one, so quiet sounds keep their shape, short
/// of making room tone or dither look loud.
pub fn render(mono: &[f32], sr: u32, columns: usize) -> Vec<u8> {
    let mut out = vec![0u8; columns * BYTES_PER_COLUMN];
    if mono.is_empty() || columns == 0 {
        return out;
    }
    let n_fft = if sr > 50_000 { 1024 } else { 512 };
    let hop = n_fft / 4;
    let stft = Stft::new(n_fft, hop);
    let bin_hz = stft.bin_hz(sr as f32);
    let low_end = ((LOW_HZ / bin_hz).round() as usize).min(stft.bins());
    let mid_end = ((MID_HZ / bin_hz).round() as usize).min(stft.bins());
    let mut bands: Vec<[f32; 3]> = Vec::with_capacity(stft.frame_count(mono.len()));
    stft.for_each_frame(mono, |_, power| {
        let sum = |r: std::ops::Range<usize>| power[r].iter().sum::<f32>();
        bands.push([
            sum(1..low_end),
            sum(low_end..mid_end),
            sum(mid_end..power.len()),
        ]);
    });
    // Equal power per octave reads as neutral, so each band is divided by
    // the octaves it spans.
    let octaves = [
        (LOW_HZ / 20.0).log2(),
        (MID_HZ / LOW_HZ).log2(),
        (sr as f32 / 2.0 / MID_HZ).log2().max(0.5),
    ];

    let peak = mono.iter().fold(0f32, |p, v| p.max(v.abs()));
    let gain = 127.0 / peak.max(0.01);
    let len = mono.len();
    for (c, col) in out.chunks_exact_mut(BYTES_PER_COLUMN).enumerate() {
        let s0 = c * len / columns;
        let s1 = ((c + 1) * len / columns).max(s0 + 1).min(len);
        let (lo, hi) = mono[s0.min(len - 1)..s1]
            .iter()
            .fold((0f32, 0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        col[0] = (lo * gain).round() as i8 as u8;
        col[1] = (hi * gain).round() as i8 as u8;

        let f0 = (s0 + hop / 2) / hop;
        let f1 = ((s1 + hop / 2) / hop).max(f0 + 1).min(bands.len());
        let mut acc = [0f32; 3];
        for b in &bands[f0.min(bands.len() - 1)..f1] {
            for i in 0..3 {
                acc[i] += b[i];
            }
        }
        let level: Vec<f32> = (0..3).map(|i| (acc[i] / octaves[i]).sqrt()).collect();
        let top = level.iter().cloned().fold(1e-9, f32::max);
        for i in 0..3 {
            col[2 + i] = (level[i] / top * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(hz: f32) -> Vec<f32> {
        (0..44_100)
            .map(|i| (2.0 * PI * hz * i as f32 / 44_100.0).sin() * 0.5)
            .collect()
    }

    #[test]
    fn colors_follow_frequency() {
        let low = render(&sine(80.0), 44_100, 16);
        let high = render(&sine(8000.0), 44_100, 16);
        let col = |w: &[u8]| w[8 * BYTES_PER_COLUMN..9 * BYTES_PER_COLUMN].to_vec();
        assert_eq!(col(&low)[2], 255);
        assert!(col(&low)[4] < 40);
        assert_eq!(col(&high)[4], 255);
        assert!(col(&high)[2] < 40);
        assert_eq!(col(&low)[1] as i8, 127);
        assert_eq!(col(&low)[0] as i8, -127);
    }

    #[test]
    fn low_sample_rates() {
        let x: Vec<f32> = (0..4_000).map(|i| (i as f32 * 0.3).sin()).collect();
        assert_eq!(render(&x, 4_000, 16).len(), 16 * BYTES_PER_COLUMN);
    }
}
