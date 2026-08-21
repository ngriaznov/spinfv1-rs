//! Bit-comparison against committed WAV captures.
//!
//! Every WAV in `tests/data/` holds raw S.23 output as 24-bit stereo PCM
//! at the chip rate, produced by streaming the shared golden test signal
//! (`common::test_signal`, pots 0.5/0.25/1.0) through a VM. The tests
//! replay the identical signal through this emulator and require every
//! output sample to equal the file exactly — no tolerance. Two tiers:
//!
//! * `tests/data/*.wav` — captured from an **independent FV-1 virtual
//!   machine implementation**, one program per datapath family on which
//!   this emulator and that implementation agree bit-for-bit:
//!   SOF/saturation, register filters, bit ops and SKP/PACC,
//!   delay/WRAP/LR/RMPA, and ADDR_PTR addressing. The LFO
//!   interpolation, LOG/EXP, and NA-crossfade families are deliberately
//!   absent from this tier: there this emulator follows Spin's
//!   documentation where that reference diverges from it (see the
//!   crate-level fidelity notes), so a bit-compare would pin the wrong
//!   behavior.
//!   These files cannot be regenerated from this crate.
//!
//! * `tests/data/corpus/*.wav` — the full standard Spin factory-program
//!   corpus (`examples/programs/`), captured from **this emulator**
//!   after its instruction-level cross-validation, freezing
//!   whole-program behavior as inspectable, listenable artifacts.
//!   Regenerate deliberately-changed behavior with
//!   `SPINFV1_WRITE_WAVS=1 cargo test --test reference_wav`.
//!
//! The assembler is `std`-only, so this file is compiled out under
//! `--no-default-features --features libm`.
#![cfg(feature = "std")]

mod common;

use common::{MICRO, compare_frames, read_wav_s23, test_signal, write_wav_s23};
use spinfv1::{Fv1, assemble};

/// Micro-programs with an externally captured reference WAV (subset of
/// `common::MICRO`).
const REFERENCE_CAPTURED: &[&str] = &[
    "gain_sof",
    "register_filters",
    "bitops_skp",
    "delay_feedback",
    "addr_ptr",
];

/// The standard Spin factory corpus (plus this crate's octave_down),
/// self-captured under `tests/data/corpus/`.
const CORPUS: &[&str] = &[
    "GA_DEMO_CHORUS",
    "GA_DEMO_FLANGE",
    "GA_DEMO_PHASE",
    "GA_DEMO_TREM",
    "octave_down",
    "rom_chor_rev",
    "rom_fla_rev",
    "rom_pitch",
    "rom_pt_echo",
    "rom_rev1",
    "rom_rev2",
    "rom_trem_rev",
];

const FRAMES: usize = 32_768;

fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// Run `source` over the golden signal, returning raw S.23 frames.
///
/// `reference_mode` disables delay quantization to match the
/// conditions the external reference captures were made under
/// (full-precision delay storage); the corpus tier runs the
/// chip-faithful default.
fn run_program(source: &str, reference_mode: bool) -> Vec<(i32, i32)> {
    let program = assemble(source).expect("program must assemble");
    let mut fv1 = Fv1::new();
    if reference_mode {
        fv1.set_delay_quantization(false);
    }
    fv1.load_program(&program);
    fv1.set_pot(0, 0.5);
    fv1.set_pot(1, 0.25);
    fv1.set_pot(2, 1.0);
    test_signal(FRAMES)
        .into_iter()
        .map(|(l, r)| fv1.process_raw(l, r))
        .collect()
}

#[test]
fn outputs_bit_match_reference_captures() {
    for stem in REFERENCE_CAPTURED {
        let source = MICRO
            .iter()
            .find(|(s, _)| s == stem)
            .expect("captured program must be in MICRO")
            .1;
        let expected = read_wav_s23(&data_dir().join(format!("{stem}.wav")));
        let got = run_program(source, true);
        compare_frames(stem, &got[..expected.len().min(got.len())], &expected);
    }
}

#[test]
fn corpus_outputs_bit_match_committed_wavs() {
    let programs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/programs");
    let corpus_dir = data_dir().join("corpus");
    let write_mode = std::env::var_os("SPINFV1_WRITE_WAVS").is_some();
    if write_mode {
        std::fs::create_dir_all(&corpus_dir).unwrap();
    }
    for stem in CORPUS {
        let source = std::fs::read_to_string(programs.join(format!("{stem}.spn")))
            .unwrap_or_else(|e| panic!("{stem}: {e}"));
        let got = run_program(&source, false);
        let wav = corpus_dir.join(format!("{stem}.wav"));
        if write_mode {
            write_wav_s23(&wav, &got);
        } else {
            compare_frames(stem, &got, &read_wav_s23(&wav));
        }
    }
}
