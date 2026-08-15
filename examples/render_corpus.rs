//! Render the dry test phrase through every SpinASM program in
//! `examples/programs/`, at 48 kHz, for by-ear regression testing.
//!
//! ```sh
//! cargo run --release --example render_corpus [output_dir]
//! ```
//!
//! Writes `dry_48k.wav` once, plus `<program>_48k.wav` for each `.spn`.
//! Comparing these renders against real-hardware recordings — or just
//! against your ears across emulator changes — is how scaling bugs like
//! the SIN LFO excursion get caught.
//!
//! Pots and dry/wet blend are set per program from each source's own pot
//! documentation: the GA_DEMO guitar-amp programs put their *built-in
//! reverb* on POT0 (turned fully off here so the headline effect is what
//! you hear) and mix dry internally (so no external dry is added), while
//! the rom_* reverbs output wet-only and get an external dry blend.
//! Unlisted programs default to all pots at 0.5 with a 0.6/0.6 mix.

#[path = "support/mod.rs"]
mod support;

use std::path::Path;

use spinfv1::{Fv1, SAMPLE_RATE, assemble};
use support::{HOST_RATE, dc_block, dry_phrase, resample, write_wav_stereo};

/// Per-program settings: `(stem, [pot0, pot1, pot2], dry_gain, wet_gain)`,
/// derived from the pot descriptions in each program's header comments.
const SETTINGS: &[(&str, [f32; 3], f32, f32)] = &[
    // GA_DEMO: POT0 = built-in reverb level (off), POT2 = effect level/width;
    // these programs mix dry internally, so no external dry is added.
    ("GA_DEMO_CHORUS", [0.0, 0.5, 0.8], 0.0, 0.9),
    ("GA_DEMO_FLANGE", [0.0, 0.5, 0.8], 0.0, 0.9),
    ("GA_DEMO_TREM", [0.0, 0.6, 0.9], 0.0, 0.9),
    ("GA_DEMO_PHASE", [0.0, 0.5, 0.8], 0.0, 0.9),
    // pot0 = pitch: the program's SOF maps the pot to +/-4 semitones
    // (rate +/-0.125); pot fully down renders the -4 semitone end.
    ("rom_pitch", [0.0, 0.5, 0.5], 0.0, 0.9),
    // pot0 = pitch, pot1 = echo delay, pot2 = echo mix.
    ("rom_pt_echo", [0.0, 0.5, 0.8], 0.0, 0.9),
    // Our own fixed octave-down transposer; no pots.
    ("octave_down", [0.0, 0.0, 0.0], 0.0, 0.9),
];

fn main() -> std::io::Result<()> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let out_dir = Path::new(&out_dir);
    std::fs::create_dir_all(out_dir)?;

    let programs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/programs");
    let mut sources: Vec<_> = std::fs::read_dir(&programs_dir)?
        .filter_map(|e| {
            let path = e.ok()?.path();
            (path.extension()? == "spn").then_some(path)
        })
        .collect();
    sources.sort();

    let dry_48k = dry_phrase();
    let dry_chip = resample(&dry_48k, HOST_RATE, SAMPLE_RATE);
    write_wav_stereo(
        &out_dir.join("dry_48k.wav"),
        &dry_48k,
        &dry_48k,
        HOST_RATE as u32,
    )?;
    println!("{:24} {:>10} {:>10}", "program", "wet rms", "peak");

    for path in sources {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path)?;
        let program = match assemble(&source) {
            Ok(p) => p,
            Err(e) => {
                println!("{stem:24} ASSEMBLY ERROR: {e}");
                continue;
            }
        };
        let (pots, dry_gain, wet_gain) = SETTINGS
            .iter()
            .find(|(name, ..)| *name == stem)
            .map_or(([0.5; 3], 0.6, 0.6), |&(_, p, d, w)| (p, d, w));
        let mut fv1 = Fv1::new();
        fv1.load_program(&program);
        for (pot, &v) in pots.iter().enumerate() {
            fv1.set_pot(pot, v);
        }
        let (mut wet_l, mut wet_r) = (Vec::new(), Vec::new());
        for &x in &dry_chip {
            let (l, r) = fv1.process(x, x);
            wet_l.push(l);
            wet_r.push(r);
        }
        let wet_l = dc_block(&resample(&wet_l, SAMPLE_RATE, HOST_RATE));
        let wet_r = dc_block(&resample(&wet_r, SAMPLE_RATE, HOST_RATE));

        let frames = dry_48k.len().min(wet_l.len());
        let mix = |wet: &[f32]| -> Vec<f32> {
            (0..frames)
                .map(|i| dry_gain * dry_48k[i] + wet_gain * wet[i])
                .collect()
        };
        let (mix_l, mix_r) = (mix(&wet_l), mix(&wet_r));

        let rms = (wet_l
            .iter()
            .map(|&v| f64::from(v) * f64::from(v))
            .sum::<f64>()
            / wet_l.len() as f64)
            .sqrt();
        let peak = mix_l
            .iter()
            .chain(&mix_r)
            .fold(0.0f32, |m, &v| m.max(v.abs()));
        write_wav_stereo(
            &out_dir.join(format!("{stem}_48k.wav")),
            &mix_l,
            &mix_r,
            HOST_RATE as u32,
        )?;
        println!("{stem:24} {rms:10.4} {peak:10.3}");
    }
    Ok(())
}
