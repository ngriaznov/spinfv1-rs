//! Extended community-program corpus: bit-comparison against committed
//! WAV captures, directory-driven.
//!
//! `tests/programs/extended/` holds third-party FV-1 programs (original
//! comment headers and attributions preserved; see the license note in
//! the README). Each has a committed 24-bit stereo WAV of its output
//! over the first 16,384 frames of the shared golden test signal
//! (`common::test_signal`, pots 0.5/0.25/1.0), in one of two tiers:
//!
//! * `tests/data/extended/ref/` — captured from an independent FV-1
//!   virtual machine implementation and bit-identical to this emulator
//!   in reference mode (full-precision delay storage). An external
//!   cross-check; these files cannot be regenerated from this crate.
//! * `tests/data/extended/self/` — programs exercising datapaths where
//!   this emulator follows Spin's documentation and diverges from that
//!   reference (see the crate-level fidelity notes); captured from this
//!   emulator, freezing behavior as regression artifacts. Regenerate
//!   after a deliberate behavior change with
//!   `SPINFV1_WRITE_WAVS=1 cargo test --test extended_wav`.
//!
//! Tests iterate the WAV directories, so extending the corpus is just
//! adding a program and its capture. Both tiers compare every sample
//! exactly, no tolerance.
#![cfg(feature = "std")]

mod common;

use std::path::{Path, PathBuf};

use common::{compare_frames, read_wav_s23, test_signal, write_wav_s23};
use spinfv1::{Fv1, assemble};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn wav_stems(dir: &Path) -> Vec<String> {
    let mut stems: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "wav").then(|| path.file_stem()?.to_str().map(String::from))?
        })
        .collect();
    stems.sort();
    stems
}

fn run_program(stem: &str, frames: usize, reference_mode: bool) -> Vec<(i32, i32)> {
    let source = std::fs::read_to_string(repo(&format!("tests/programs/extended/{stem}.spn")))
        .unwrap_or_else(|e| panic!("{stem}: {e}"));
    let program = assemble(&source).unwrap_or_else(|e| panic!("{stem}: {e}"));
    let mut fv1 = Fv1::new();
    if reference_mode {
        fv1.set_delay_quantization(false);
    }
    fv1.load_program(&program);
    fv1.set_pot(0, 0.5);
    fv1.set_pot(1, 0.25);
    fv1.set_pot(2, 1.0);
    test_signal(frames)
        .into_iter()
        .map(|(l, r)| fv1.process_raw(l, r))
        .collect()
}

#[test]
fn extended_corpus_matches_reference_captures() {
    let dir = repo("tests/data/extended/ref");
    let stems = wav_stems(&dir);
    assert!(!stems.is_empty(), "reference tier must not be empty");
    for stem in &stems {
        let expected = read_wav_s23(&dir.join(format!("{stem}.wav")));
        let got = run_program(stem, expected.len(), true);
        compare_frames(stem, &got, &expected);
    }
}

#[test]
fn extended_corpus_matches_self_captures() {
    let dir = repo("tests/data/extended/self");
    let write_mode = std::env::var_os("SPINFV1_WRITE_WAVS").is_some();
    let stems = wav_stems(&dir);
    assert!(!stems.is_empty(), "self tier must not be empty");
    for stem in &stems {
        let wav = dir.join(format!("{stem}.wav"));
        if write_mode {
            let frames = read_wav_s23(&wav).len();
            write_wav_s23(&wav, &run_program(stem, frames, false));
        } else {
            let expected = read_wav_s23(&wav);
            compare_frames(stem, &run_program(stem, expected.len(), false), &expected);
        }
    }
}

#[test]
fn extended_corpus_is_at_least_a_hundred_programs() {
    let total = wav_stems(&repo("tests/data/extended/ref")).len()
        + wav_stems(&repo("tests/data/extended/self")).len();
    assert!(total >= 100, "extended corpus shrank to {total} programs");
}
