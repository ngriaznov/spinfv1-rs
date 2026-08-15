//! Render the dry test phrase through every SpinASM program in
//! `examples/programs/`, at 48 kHz, for by-ear regression testing.
//!
//! ```sh
//! cargo run --release --example render_corpus [output_dir]
//! ```
//!
//! Writes `dry_48k.wav` once, plus `<program>_48k.wav` for each `.spn`
//! (dry mixed with the program's wet output). Comparing these renders
//! against real-hardware recordings — or just against your ears across
//! emulator changes — is how scaling bugs like the SIN LFO excursion get
//! caught. All pots sit at 0.5.

#[path = "support/mod.rs"]
mod support;

use std::path::Path;

use spinfv1::{Fv1, SAMPLE_RATE, assemble};
use support::{HOST_RATE, dry_phrase, resample, write_wav_stereo};

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
        let mut fv1 = Fv1::new();
        fv1.load_program(&program);
        for pot in 0..3 {
            fv1.set_pot(pot, 0.5);
        }
        let (mut wet_l, mut wet_r) = (Vec::new(), Vec::new());
        for &x in &dry_chip {
            let (l, r) = fv1.process(x, x);
            wet_l.push(l);
            wet_r.push(r);
        }
        let wet_l = resample(&wet_l, SAMPLE_RATE, HOST_RATE);
        let wet_r = resample(&wet_r, SAMPLE_RATE, HOST_RATE);

        // Effects differ in whether they output wet-only or a full mix; a
        // moderate dry blend keeps every render comparable by ear.
        let frames = dry_48k.len().min(wet_l.len());
        let mix = |wet: &[f32]| -> Vec<f32> {
            (0..frames)
                .map(|i| 0.6 * dry_48k[i] + 0.6 * wet[i])
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
