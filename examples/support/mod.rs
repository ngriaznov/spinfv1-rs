//! Shared helpers for the 48 kHz rendering examples: WAV output, a
//! cubic-Hermite resampler, and the dry test phrase.
//!
//! Not an example itself — pulled in via `#[path = "support/mod.rs"]`.

use std::fs::File;
use std::io::{BufWriter, Result, Write};
use std::path::Path;

/// The host sample rate the examples render at.
pub const HOST_RATE: f64 = 48_000.0;

/// Minimal stereo 16-bit PCM WAV writer.
pub fn write_wav_stereo(path: &Path, l: &[f32], r: &[f32], sample_rate: u32) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    let frames = l.len().min(r.len());
    let data_len = (frames * 4) as u32;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&2u16.to_le_bytes())?; // stereo
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&(sample_rate * 4).to_le_bytes())?;
    w.write_all(&4u16.to_le_bytes())?; // block align
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    for i in 0..frames {
        for s in [l[i], r[i]] {
            w.write_all(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
        }
    }
    Ok(())
}

/// 4-point cubic Hermite (Catmull-Rom) resampler. Plenty for a demo; a
/// production host would use a windowed-sinc polyphase instead.
pub fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
    let out_len = (input.len() as f64 * to_rate / from_rate) as usize;
    let step = from_rate / to_rate;
    let at = |i: isize| -> f64 {
        f64::from(
            *input
                .get(i.clamp(0, input.len() as isize - 1) as usize)
                .unwrap_or(&0.0),
        )
    };
    (0..out_len)
        .map(|n| {
            let pos = n as f64 * step;
            let i = pos.floor() as isize;
            let t = pos - pos.floor();
            let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let c1 = 0.5 * (y2 - y0);
            let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
            (((c3 * t + c2) * t + c1) * t + y1) as f32
        })
        .collect()
}

/// The dry test signal at 48 kHz, effects-friendly by construction:
/// six staccato plucks (transients for delays/reverbs), then a sustained
/// two-tone chord (steady-state for tremolo/chorus/flanger/phaser), then
/// silence so effect tails ring out. Every note edge is faded to zero so
/// the source itself is click-free.
pub fn dry_phrase() -> Vec<f32> {
    let sr = HOST_RATE;
    let total = (sr * 8.0) as usize;
    let mut out = vec![0.0f32; total];

    // Staccato plucks.
    let notes: [(f64, f64); 6] = [
        (392.0, 0.0),
        (523.25, 0.5),
        (659.25, 1.0),
        (783.99, 1.5),
        (659.25, 2.0),
        (523.25, 2.5),
    ];
    for &(freq, start) in &notes {
        let s0 = (start * sr) as usize;
        let len = (sr * 0.45) as usize;
        let fade = (sr * 0.08) as usize;
        for i in 0..len {
            let t = i as f64 / sr;
            let mut env = (-14.0 * t).exp() * (1.0 - (-800.0 * t).exp());
            if i >= len - fade {
                let x = (i - (len - fade)) as f64 / fade as f64;
                env *= 0.5 * (1.0 + (core::f64::consts::PI * x).cos());
            }
            let tone = (core::f64::consts::TAU * freq * t).sin()
                + 0.35 * (core::f64::consts::TAU * 2.0 * freq * t).sin();
            out[s0 + i] += (0.42 * env * tone) as f32;
        }
    }

    // Sustained two-tone chord, 3.3 s - 5.3 s, with cosine attack/release.
    let s0 = (3.3 * sr) as usize;
    let len = (2.0 * sr) as usize;
    let edge = (0.05 * sr) as usize;
    for i in 0..len {
        let t = i as f64 / sr;
        let mut env = 0.28;
        if i < edge {
            env *= 0.5 * (1.0 - (core::f64::consts::PI * (1.0 - i as f64 / edge as f64)).cos());
        }
        if i >= len - edge {
            let x = (i - (len - edge)) as f64 / edge as f64;
            env *= 0.5 * (1.0 + (core::f64::consts::PI * x).cos());
        }
        let tone = (core::f64::consts::TAU * 293.66 * t).sin()
            + 0.7 * (core::f64::consts::TAU * 440.0 * t).sin();
        out[s0 + i] += (env * tone) as f32;
    }
    out
}
