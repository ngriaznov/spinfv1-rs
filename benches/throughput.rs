//! Throughput benchmarks, no `criterion` (no dev-dependency needed to keep
//! the crate's zero-dependency default build intact): hand-rolled timing
//! with [`std::time::Instant`] over six representative FV-1 programs.
//!
//! Run with `cargo bench`. Each program processes ~2,000,000 samples of a
//! deterministic pseudo-random stereo signal; we print samples/sec, the
//! ratio against the chip's native 32,768 Hz rate, and a checksum of the
//! raw output stream so a run is directly comparable to a previous one
//! (same numbers in, same numbers out) as the implementation changes.

use std::hint::black_box;
use std::time::{Duration, Instant};

use spinfv1::{ChoFlags, Fv1, Instruction, LfoSel, Program, RampAmp, SkpCond, coeff, reg};

/// Samples processed per timed run. ~2e6 samples is about a minute of audio
/// at the chip's native rate, plenty to average out call overhead.
const SAMPLES: usize = 2_000_000;
/// Untimed samples run once before timing, to warm up branch predictors and
/// caches without counting the effect of any allocator warm-up.
const WARMUP_SAMPLES: usize = 20_000;
/// Timed repetitions per program; we report the best (least noisy) of these.
const RUNS: u32 = 3;

const OUT_L: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};
const OUT_R: Instruction = Instruction::Wrax {
    reg: reg::DACR,
    c: 0,
};

/// A small linear congruential generator producing a deterministic stereo
/// stream of raw S.23 samples, split from one 64-bit draw per sample (same
/// technique the crate's own end-to-end tests use for pseudo-random input).
struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_pair(&mut self) -> (i32, i32) {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let l = (self.0 >> 40) as i32 - (1 << 23);
        let r = ((self.0 >> 16) & 0xFF_FFFF) as i32 - (1 << 23);
        (l, r)
    }
}

/// Passthrough: `DACL = ADCL`, `DACR = ADCR`.
fn passthrough_program() -> Program {
    Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        OUT_L,
        Instruction::ldax(reg::ADCR),
        OUT_R,
    ])
    .expect("4 instructions fit in 128 slots")
}

/// Feedback echo with a pot-controlled feedback gain (mirrors
/// `tests/audio_e2e.rs::echo_with_pot_controlled_feedback`).
fn feedback_echo_program() -> Program {
    let d = 400u16;
    Program::from_instructions(&[
        Instruction::Rda {
            addr: d,
            c: coeff::s1_9(1.0),
        },
        Instruction::Mulx { reg: reg::POT0 },
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(1.0),
        },
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: d,
            c: coeff::s1_9(1.0),
        },
        OUT_L,
    ])
    .expect("6 instructions fit in 128 slots")
}

/// Sine-LFO chorus using the `CHO RDA` interpolation pair (mirrors
/// `tests/audio_e2e.rs::chorus_delay_tracks_the_sine_lfo`).
fn chorus_program() -> Program {
    let base = 1000u16;
    let amp = 256u16;
    let rate = 80u16;
    Program::from_instructions(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wlds {
            lfo: false,
            freq: rate,
            amp,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::COMPC,
            addr: base,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: base + 1,
        },
        OUT_L,
    ])
    .expect("7 instructions fit in 128 slots")
}

/// Dual-tap crossfaded pitch shifter (mirrors
/// `tests/audio_e2e.rs::pitch_shifter`).
fn pitch_shifter_program() -> Program {
    let temp = 16384u16;
    let rate = 4915i16;
    Program::from_instructions(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wldr {
            lfo: false,
            freq: rate,
            amp: RampAmp::Amp4096,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        // Tap 1 at the ramp pointer, linearly interpolated.
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::REG | ChoFlags::COMPC,
            addr: 0,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::SIN,
            addr: 1,
        },
        Instruction::Wra { addr: temp, c: 0 },
        // Tap 2 half a window away.
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::RPTR2 | ChoFlags::COMPC,
            addr: 0,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::RPTR2,
            addr: 1,
        },
        // Crossfade the two taps with the triangle envelope.
        Instruction::ChoSof {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::NA | ChoFlags::COMPC,
            d: 0,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::NA,
            addr: temp,
        },
        OUT_L,
    ])
    .expect("11 instructions fit in 128 slots")
}

/// A dense 128-instruction program that exercises every opcode (and every
/// `WLDS`/`WLDR`, `CHO RDA`/`SOF`/`RDAL` sub-type) at least once, to measure
/// worst-case per-pass dispatch cost rather than a light effect's.
fn dense_program() -> Program {
    use Instruction::{
        And, ChoRda, ChoRdal, ChoSof, Exp, Jam, Log, Maxx, Mulx, Or, Rda, Rdax, Rdfx, Rmpa, Skp,
        Sof, Wldr, Wlds, Wra, Wrap, Wrax, Wrhx, Wrlx, Xor,
    };
    // Every opcode/sub-type at least once (24 variants covering all 21
    // FV-1 opcodes, since WLDS/WLDR and the three CHO forms share an
    // opcode each). Coefficients and addresses are arbitrary but valid.
    let cycle = [
        Rda {
            addr: 1000,
            c: coeff::s1_9(0.4),
        },
        Rmpa {
            c: coeff::s1_9(0.3),
        },
        Wra {
            addr: 1000,
            c: coeff::s1_9(0.2),
        },
        Wrap {
            addr: 2000,
            c: coeff::s1_9(-0.3),
        },
        Rdax {
            reg: reg::user(0),
            c: coeff::s1_14(0.5),
        },
        Rdfx {
            reg: reg::user(1),
            c: coeff::s1_14(0.25),
        },
        Wrax {
            reg: reg::user(2),
            c: coeff::s1_14(0.9),
        },
        Wrhx {
            reg: reg::user(3),
            c: coeff::s1_14(0.1),
        },
        Wrlx {
            reg: reg::user(4),
            c: coeff::s1_14(-0.2),
        },
        Maxx {
            reg: reg::user(5),
            c: coeff::s1_14(0.7),
        },
        Mulx { reg: reg::user(6) },
        Log {
            c: coeff::s1_14(1.0),
            d: coeff::s4_6(0.0),
        },
        Exp {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.0),
        },
        Sof {
            c: coeff::s1_14(0.8),
            d: coeff::s_10(0.1),
        },
        And { mask: 0x7F_FFFF },
        Or { mask: 0x00_0001 },
        Xor { mask: 0x00_00FF },
        Skp {
            cond: SkpCond::NEG,
            n: 1,
        },
        Wlds {
            lfo: false,
            freq: 100,
            amp: 400,
        },
        Wldr {
            lfo: true,
            freq: 300,
            amp: RampAmp::Amp2048,
        },
        Jam { lfo: true },
        ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: 500,
        },
        ChoSof {
            lfo: LfoSel::Rmp1,
            flags: ChoFlags::NA,
            d: coeff::s_15(0.3),
        },
        ChoRdal {
            lfo: LfoSel::Sin1,
            flags: ChoFlags::COS,
        },
    ];
    let mut ins = vec![Instruction::ldax(reg::ADCL)];
    for c in cycle.into_iter().cycle() {
        if ins.len() >= 126 {
            break;
        }
        ins.push(c);
    }
    ins.push(OUT_L);
    ins.push(OUT_R);
    assert_eq!(ins.len(), 128, "dense program must fill all 128 slots");
    Program::from_instructions(&ins).expect("exactly 128 instructions")
}

/// Time `samples` calls to [`Fv1::process_raw`] against `program`, returning
/// the best of [`RUNS`] timed repetitions as `(samples/sec, checksum)`. The
/// checksum is a wrapping sum of every raw output word from the winning run,
/// so it also catches accuracy regressions between benchmark runs.
fn bench(program: &Program, samples: usize) -> (f64, u64) {
    // One untimed warm-up pass on a throwaway VM instance.
    {
        let mut fv1 = Fv1::new();
        fv1.load_program(program);
        let mut lcg = Lcg::new(1);
        for _ in 0..WARMUP_SAMPLES {
            let (l, r) = lcg.next_pair();
            black_box(fv1.process_raw(black_box(l), black_box(r)));
        }
    }

    let mut best_elapsed = Duration::MAX;
    let mut best_checksum = 0u64;
    for _ in 0..RUNS {
        let mut fv1 = Fv1::new();
        fv1.load_program(program);
        let mut lcg = Lcg::new(0xC0FF_EE12_3456_789A);
        let mut checksum = 0u64;

        let start = Instant::now();
        for _ in 0..samples {
            let (l, r) = lcg.next_pair();
            let (out_l, out_r) = fv1.process_raw(black_box(l), black_box(r));
            checksum = checksum
                .wrapping_add(u64::from(out_l as u32))
                .wrapping_add(u64::from(out_r as u32));
        }
        let elapsed = start.elapsed();
        black_box(checksum);

        if elapsed < best_elapsed {
            best_elapsed = elapsed;
            best_checksum = checksum;
        }
    }

    (samples as f64 / best_elapsed.as_secs_f64(), best_checksum)
}

fn main() {
    let cases: [(&str, Program); 6] = [
        ("all-NOP (dispatch floor)", Program::default()),
        ("passthrough", passthrough_program()),
        ("feedback echo", feedback_echo_program()),
        ("sine-LFO chorus (CHO RDA pair)", chorus_program()),
        ("dual-tap RMP pitch shifter", pitch_shifter_program()),
        ("dense 128-op (every opcode)", dense_program()),
    ];

    println!(
        "{:<32} {:>14} {:>14} {:>20}",
        "program", "samples/sec", "vs real-time", "checksum"
    );
    println!("{}", "-".repeat(32 + 1 + 14 + 1 + 14 + 1 + 20));
    for (name, program) in &cases {
        let (samples_per_sec, checksum) = bench(program, SAMPLES);
        let ratio = samples_per_sec / spinfv1::SAMPLE_RATE;
        println!("{name:<32} {samples_per_sec:>14.0} {ratio:>13.0}x {checksum:>20}",);
    }
}
