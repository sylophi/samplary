use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub struct Decoded {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: Option<u32>,
    /// Length of the whole file in frames, even when `mono` was cut short.
    pub frames: u64,
    /// Mono mixdown, at most `max_seconds` long.
    pub mono: Vec<f32>,
}

impl Decoded {
    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }
}

/// Every channel, for playing.
pub struct Pcm {
    pub sample_rate: u32,
    /// One per channel, each at most `max_seconds` long.
    pub channels: Vec<Vec<f32>>,
}

struct Format {
    sample_rate: u32,
    channels: usize,
    bits: Option<u32>,
    frames: u64,
}

/// Decodes the file's audio track, handing each packet's interleaved
/// samples to `take` until it has `max_seconds` of them.
fn read(
    path: &Path,
    max_seconds: Option<f64>,
    mut take: impl FnMut(&[f32], usize),
) -> Result<Format, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| "It isn't in a format Samplary can read.")?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("It has no audio.")?;
    let track_id = track.id;
    let known_frames = track.num_frames;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("It has no audio.")?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|_| "Its audio is in a codec Samplary can't decode.")?;

    let mut sample_rate = params.sample_rate.unwrap_or(0);
    let mut channels = params.channels.as_ref().map_or(0, |c| c.count());
    let mut interleaved: Vec<f32> = Vec::new();
    let mut frames: u64 = 0;
    let mut taken: usize = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) | Err(Error::ResetRequired) => break,
            Err(Error::IoError(_)) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        sample_rate = buf.spec().rate();
        channels = buf.spec().channels().count().max(1);
        let n = buf.frames();
        frames += n as u64;
        let limit = max_seconds.map(|s| (s * sample_rate as f64) as usize);
        if limit.is_some_and(|m| taken >= m) {
            // Only the length is still needed, so stop once the container knows it.
            if known_frames.is_some() {
                break;
            }
            continue;
        }
        interleaved.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut interleaved);
        take(&interleaved, channels);
        taken += n;
    }

    if sample_rate == 0 {
        return Err("Its sample rate is missing.".into());
    }
    if taken == 0 {
        return Err("It has no audio.".into());
    }
    Ok(Format {
        sample_rate,
        channels,
        bits: params.bits_per_sample,
        frames: known_frames.unwrap_or(frames).max(taken as u64),
    })
}

pub fn decode(path: &Path, max_seconds: Option<f64>) -> Result<Decoded, String> {
    let mut mono: Vec<f32> = Vec::new();
    let f = read(path, max_seconds, |interleaved, channels| {
        let scale = 1.0 / channels as f32;
        mono.extend(
            interleaved
                .chunks_exact(channels)
                .map(|f| f.iter().sum::<f32>() * scale),
        );
    })?;
    if let Some(s) = max_seconds {
        mono.truncate((s * f.sample_rate as f64) as usize);
    }
    Ok(Decoded {
        sample_rate: f.sample_rate,
        channels: f.channels as u16,
        bits: f.bits,
        frames: f.frames.max(mono.len() as u64),
        mono,
    })
}

pub fn decode_channels(path: &Path, max_seconds: Option<f64>) -> Result<Pcm, String> {
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let f = read(path, max_seconds, |interleaved, channels| {
        planar.resize_with(channels, Vec::new);
        for frame in interleaved.chunks_exact(channels) {
            for (c, &v) in frame.iter().enumerate() {
                planar[c].push(v);
            }
        }
    })?;
    // A stream that changes its channel count partway leaves some short.
    let len = planar.iter().map(Vec::len).min().unwrap_or(0);
    let len = max_seconds.map_or(len, |s| len.min((s * f.sample_rate as f64) as usize));
    for c in &mut planar {
        c.truncate(len);
    }
    Ok(Pcm {
        sample_rate: f.sample_rate,
        channels: planar,
    })
}
