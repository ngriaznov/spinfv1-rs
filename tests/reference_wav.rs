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
//!   documentation where that reference diverges from it (see README
//!   "Fidelity notes"), so a bit-compare would pin the wrong behavior.
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

use common::{MICRO, test_signal};
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
fn run_program(source: &str) -> Vec<(i32, i32)> {
    let program = assemble(source).expect("program must assemble");
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    fv1.set_pot(0, 0.5);
    fv1.set_pot(1, 0.25);
    fv1.set_pot(2, 1.0);
    test_signal(FRAMES)
        .into_iter()
        .map(|(l, r)| fv1.process_raw(l, r))
        .collect()
}

/// Minimal reader for the exact WAV shape we commit: RIFF/WAVE, PCM
/// `fmt ` chunk (24-bit stereo), then a `data` chunk of little-endian
/// signed 24-bit frames, returned as raw S.23 (l, r) pairs.
fn read_wav_s23(path: &std::path::Path) -> Vec<(i32, i32)> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(&b[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&b[8..12], b"WAVE", "not a WAVE file");
    let mut pos = 12;
    let mut fmt_ok = false;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..pos + 8 + len];
        match id {
            b"fmt " => {
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                assert_eq!((channels, bits), (2, 24), "expected 24-bit stereo PCM");
                fmt_ok = true;
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    assert!(fmt_ok, "missing fmt chunk");
    let data = data.expect("missing data chunk");
    data.chunks_exact(6)
        .map(|f| {
            let s24 = |c: &[u8]| (i32::from_le_bytes([0, c[0], c[1], c[2]])) >> 8;
            (s24(&f[0..3]), s24(&f[3..6]))
        })
        .collect()
}

/// Writer mirroring `read_wav_s23`: 24-bit stereo PCM at the chip rate.
fn write_wav_s23(path: &std::path::Path, frames: &[(i32, i32)]) {
    let mut data = Vec::with_capacity(frames.len() * 6);
    for &(l, r) in frames {
        data.extend_from_slice(&l.to_le_bytes()[0..3]);
        data.extend_from_slice(&r.to_le_bytes()[0..3]);
    }
    let mut b = Vec::with_capacity(44 + data.len());
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&2u16.to_le_bytes()); // stereo
    b.extend_from_slice(&32_768u32.to_le_bytes()); // chip rate
    b.extend_from_slice(&(32_768u32 * 6).to_le_bytes());
    b.extend_from_slice(&6u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(path, b).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn compare(stem: &str, got: &[(i32, i32)], expected: &[(i32, i32)]) {
    assert_eq!(got.len(), expected.len(), "{stem}: frame count");
    for (n, (g, e)) in got.iter().zip(expected).enumerate() {
        assert_eq!(g, e, "{stem}: sample {n} diverges from the committed WAV");
    }
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
        let got = run_program(source);
        compare(stem, &got[..expected.len().min(got.len())], &expected);
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
        let got = run_program(&source);
        let wav = corpus_dir.join(format!("{stem}.wav"));
        if write_mode {
            write_wav_s23(&wav, &got);
        } else {
            compare(stem, &got, &read_wav_s23(&wav));
        }
    }
}
