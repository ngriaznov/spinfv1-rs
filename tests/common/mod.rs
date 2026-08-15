//! Shared helpers for integration tests.
//!
//! Each test binary compiles this module separately and uses a different
//! subset of the helpers, so unused-function warnings are expected noise.
#![allow(dead_code)]

use spinfv1::{Fv1, Instruction, Program};

/// Build a VM with the given instructions loaded (padded with NOP).
pub fn vm(instructions: &[Instruction]) -> Fv1 {
    let program = Program::from_instructions(instructions).expect("test program fits in 128 slots");
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    fv1
}

/// Run `samples` mono samples (left channel), returning left outputs.
pub fn run_mono(fv1: &mut Fv1, input: impl IntoIterator<Item = f32>) -> Vec<f32> {
    input.into_iter().map(|x| fv1.process(x, 0.0).0).collect()
}

/// Count positive-going zero crossings, ignoring exact zeros.
pub fn positive_crossings(signal: &[f32]) -> usize {
    signal
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count()
}

/// Mean frequency in cycles/sample from positive-going zero crossings,
/// measured between the first and last crossing.
pub fn crossing_frequency(signal: &[f32]) -> f64 {
    let crossings: Vec<usize> = signal
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, _)| i)
        .collect();
    assert!(crossings.len() >= 2, "need at least two zero crossings");
    let cycles = crossings.len() - 1;
    let span = crossings[crossings.len() - 1] - crossings[0];
    cycles as f64 / span as f64
}
