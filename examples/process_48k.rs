//! Run a real Spin reverb program on a dry 48 kHz signal.
//!
//! ```sh
//! cargo run --example process_48k [output_dir]
//! ```
//!
//! The FV-1 is a 32,768 Hz chip. To process audio living at 48 kHz with
//! authentic behavior (delay lengths, LFO rates and reverb decay exactly as
//! on hardware), this example resamples 48 kHz -> 32,768 Hz, runs the chip,
//! and resamples the stereo output back to 48 kHz — the same structure a
//! plugin host wrapper would use. (The alternative — calling `process` once
//! per 48 kHz sample — also works, but stretches every delay time and LFO
//! rate by 48000/32768.)
//!
//! The program is `examples/programs/rom_rev1.spn`, Spin's classic ROM
//! reverb 1, assembled from source at build time. The dry input is a
//! synthesized plucked phrase with silent gaps so the reverb tail is
//! clearly audible. Two stereo WAVs are written: the dry source and the
//! processed dry+wet mix.

use std::fs::File;
use std::io::{BufWriter, Result, Write};
use std::path::Path;

use spinfv1::{Fv1, SAMPLE_RATE, assemble};

const HOST_RATE: f64 = 48_000.0;

/// Minimal stereo 16-bit PCM WAV writer.
fn write_wav_stereo(path: &Path, l: &[f32], r: &[f32], sample_rate: u32) -> Result<()> {
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
fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
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

/// A dry plucked phrase at 48 kHz: staccato notes with gaps, then silence
/// so the reverb tail rings out. No effects — this is the "before" signal.
fn dry_phrase() -> Vec<f32> {
    let notes: [(f64, f64); 6] = [
        (392.0, 0.0),
        (523.25, 0.5),
        (659.25, 1.0),
        (783.99, 1.5),
        (659.25, 2.0),
        (523.25, 2.5),
    ];
    let total = (HOST_RATE * 5.5) as usize; // phrase + 3 s of tail room
    let mut out = vec![0.0f32; total];
    for &(freq, start) in &notes {
        let s0 = (start * HOST_RATE) as usize;
        let len = (HOST_RATE * 0.25) as usize; // short staccato pluck
        for i in 0..len {
            let t = i as f64 / HOST_RATE;
            let env = (-14.0 * t).exp() * (1.0 - (-800.0 * t).exp());
            let tone = (core::f64::consts::TAU * freq * t).sin()
                + 0.35 * (core::f64::consts::TAU * 2.0 * freq * t).sin();
            out[s0 + i] += (0.42 * env * tone) as f32;
        }
    }
    out
}

fn main() -> Result<()> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir)?;

    // Assemble the genuine Spin ROM reverb from its SpinASM source.
    let program = assemble(include_str!("programs/rom_rev1.spn")).expect("rom_rev1.spn assembles");

    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    fv1.set_pot(0, 0.8); // reverb time
    fv1.set_pot(1, 0.5); // low-frequency response
    fv1.set_pot(2, 0.5); // high-frequency response

    // Dry 48 kHz source -> chip rate -> FV-1 -> back to 48 kHz.
    let dry_48k = dry_phrase();
    let dry_chip = resample(&dry_48k, HOST_RATE, SAMPLE_RATE);
    let mut wet_l = Vec::with_capacity(dry_chip.len());
    let mut wet_r = Vec::with_capacity(dry_chip.len());
    for &x in &dry_chip {
        let (l, r) = fv1.process(x, x);
        wet_l.push(l);
        wet_r.push(r);
    }
    let wet_l_48k = resample(&wet_l, SAMPLE_RATE, HOST_RATE);
    let wet_r_48k = resample(&wet_r, SAMPLE_RATE, HOST_RATE);

    // rom_rev1 outputs wet only; mix like a pedal would.
    let frames = dry_48k.len().min(wet_l_48k.len());
    let mix = |wet: &[f32]| -> Vec<f32> {
        (0..frames)
            .map(|i| 0.8 * dry_48k[i] + 0.55 * wet[i])
            .collect()
    };
    let (mix_l, mix_r) = (mix(&wet_l_48k), mix(&wet_r_48k));

    let dry_path = dir.join("dry_48k.wav");
    let wet_path = dir.join("reverb_48k.wav");
    write_wav_stereo(&dry_path, &dry_48k, &dry_48k, HOST_RATE as u32)?;
    write_wav_stereo(&wet_path, &mix_l, &mix_r, HOST_RATE as u32)?;
    println!(
        "wrote {} ({} frames, dry source)",
        dry_path.display(),
        frames
    );
    println!(
        "wrote {} ({} frames, rom_rev1 at pots 0.8/0.5/0.5)",
        wet_path.display(),
        frames
    );
    Ok(())
}
