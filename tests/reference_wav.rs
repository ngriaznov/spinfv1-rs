//! Bit-comparison against reference-captured WAV files.
//!
//! The WAVs in `tests/data/` were captured by running the shared golden
//! test signal (`common::test_signal`, pots 0.5/0.25/1.0) through an
//! independent FV-1 virtual machine implementation and writing its raw
//! S.23 outputs as 24-bit PCM at the chip rate. Each covered program
//! exercises a datapath family on which this emulator and that
//! implementation agree bit-for-bit: SOF/saturation, register filters,
//! bit ops and SKP/PACC, delay/WRAP/LR/RMPA, and ADDR_PTR addressing.
//! The test replays the same signal through our VM and requires every
//! output sample to equal the captured file exactly — no tolerance.
//!
//! The LFO interpolation, LOG/EXP, and NA-crossfade families are
//! deliberately absent: there this emulator follows Spin's documentation
//! where the reference diverges from it (see README "Fidelity notes"),
//! so a bit-compare against that reference would pin the wrong behavior.
//! Those families are frozen by `tests/golden.rs` and
//! `tests/log_exp_values.rs` instead.
//!
//! The assembler is `std`-only, so this file is compiled out under
//! `--no-default-features --features libm`.
#![cfg(feature = "std")]

mod common;

use common::{MICRO, test_signal};
use spinfv1::{Fv1, assemble};

/// Programs with a captured reference WAV (subset of `common::MICRO`).
const CAPTURED: &[&str] = &[
    "gain_sof",
    "register_filters",
    "bitops_skp",
    "delay_feedback",
    "addr_ptr",
];

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

#[test]
fn outputs_bit_match_reference_captures() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    for stem in CAPTURED {
        let source = MICRO
            .iter()
            .find(|(s, _)| s == stem)
            .expect("captured program must be in MICRO")
            .1;
        let expected = read_wav_s23(&dir.join(format!("{stem}.wav")));

        let program = assemble(source).expect("program must assemble");
        let mut fv1 = Fv1::new();
        fv1.load_program(&program);
        fv1.set_pot(0, 0.5);
        fv1.set_pot(1, 0.25);
        fv1.set_pot(2, 1.0);

        let signal = test_signal(expected.len());
        for (n, ((l, r), (el, er))) in signal.iter().zip(&expected).enumerate() {
            let (ol, or) = fv1.process_raw(*l, *r);
            assert_eq!(
                (ol, or),
                (*el, *er),
                "{stem}: sample {n} diverges from the reference capture"
            );
        }
    }
}
