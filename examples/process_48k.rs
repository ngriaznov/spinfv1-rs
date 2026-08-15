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

#[path = "support/mod.rs"]
mod support;

use std::path::Path;

use spinfv1::{Fv1, SAMPLE_RATE, assemble};
use support::{HOST_RATE, dc_block, dry_phrase, resample, write_wav_stereo};

fn main() -> std::io::Result<()> {
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
    // AC-couple the output like a real board (see support::dc_block).
    let wet_l_48k = dc_block(&resample(&wet_l, SAMPLE_RATE, HOST_RATE));
    let wet_r_48k = dc_block(&resample(&wet_r, SAMPLE_RATE, HOST_RATE));

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
