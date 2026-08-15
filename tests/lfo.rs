//! LFO behavior: SIN frequency and amplitude, RMP period and direction,
//! JAM, the NA cross-fade envelope, and CHO RDA interpolation plumbing.

mod common;

use common::{crossing_frequency, positive_crossings, run_mono, vm};
use spinfv1::{ChoFlags, Instruction, LfoSel, RampAmp, SkpCond, reg};

const OUT: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};

/// Program that streams an LFO value (via CHO RDAL) to DACL.
fn rdal_program(setup: Instruction, lfo: LfoSel, flags: ChoFlags) -> Vec<Instruction> {
    vec![
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        setup,
        Instruction::ChoRdal { lfo, flags },
        OUT,
    ]
}

#[test]
fn sin_lfo_frequency_matches_formula() {
    // f = rate * SR / (2^18 * pi) Hz, i.e. rate / 2^17 radians per sample.
    for rate in [50u16, 200, 511] {
        let mut fv1 = vm(&rdal_program(
            Instruction::Wlds {
                lfo: false,
                freq: rate,
                amp: 32767,
            },
            LfoSel::Sin0,
            ChoFlags::SIN,
        ));
        let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1 << 16));
        let measured = crossing_frequency(&out);
        let expected = f64::from(rate) / f64::from(1u32 << 17) / core::f64::consts::TAU;
        let rel = (measured - expected).abs() / expected;
        assert!(
            rel < 0.02,
            "rate {rate}: measured {measured}, expected {expected}"
        );
    }
}

#[test]
fn sin_lfo_amplitude_scales_with_range() {
    for amp in [32767u16, 8192, 1024] {
        let mut fv1 = vm(&rdal_program(
            Instruction::Wlds {
                lfo: false,
                freq: 100,
                amp,
            },
            LfoSel::Sin0,
            ChoFlags::SIN,
        ));
        let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1 << 15));
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        let expected = f64::from(amp) / 32768.0;
        let rel = (f64::from(peak) - expected).abs() / expected;
        assert!(rel < 0.02, "amp {amp}: peak {peak}, expected {expected}");
    }
}

#[test]
fn sin_lfo_cos_output_is_quadrature() {
    let make = |flags| {
        vm(&rdal_program(
            Instruction::Wlds {
                lfo: false,
                freq: 100,
                amp: 32767,
            },
            LfoSel::Sin0,
            flags,
        ))
    };
    let n = 1 << 15;
    let sin = run_mono(&mut make(ChoFlags::SIN), std::iter::repeat_n(0.0, n));
    let cos = run_mono(&mut make(ChoFlags::COS), std::iter::repeat_n(0.0, n));
    // sin^2 + cos^2 should be roughly constant at the amplitude squared.
    for i in (0..n).step_by(500) {
        let mag = f64::from(sin[i]).hypot(f64::from(cos[i]));
        assert!((mag - 1.0).abs() < 0.02, "at {i}: |(sin, cos)| = {mag}");
    }
}

#[test]
fn sin1_is_independent_of_sin0() {
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 2,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 100,
            amp: 32767,
        },
        Instruction::Wlds {
            lfo: true,
            freq: 400,
            amp: 32767,
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Sin1,
            flags: ChoFlags::SIN,
        },
        OUT,
    ]);
    let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1 << 16));
    let measured = crossing_frequency(&out);
    let expected = 400.0 / f64::from(1u32 << 17) / core::f64::consts::TAU;
    assert!((measured - expected).abs() / expected < 0.02);
}

#[test]
fn wlds_every_pass_does_not_freeze_the_lfo() {
    // No SKP RUN guard: WLDS runs every sample. The oscillator must still
    // advance (WLDS writes rate/range registers without resetting phase).
    let mut fv1 = vm(&[
        Instruction::Wlds {
            lfo: false,
            freq: 200,
            amp: 32767,
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
        },
        OUT,
    ]);
    let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1 << 15));
    assert!(
        positive_crossings(&out) >= 3,
        "LFO frozen by unguarded WLDS"
    );
}

#[test]
fn lfo_rate_is_live_register() {
    // Drive SIN0_RATE from POT0 while measuring: frequency must follow.
    let program = vec![
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 0,
            amp: 32767,
        },
        Instruction::ldax(reg::POT0),
        Instruction::Wrax {
            reg: reg::SIN0_RATE,
            c: 0,
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
        },
        OUT,
    ];
    let mut fv1 = vm(&program);
    // POT value equal to what WLDS f=400 would store: 400 << 14 raw.
    let pot = (400i32 << 14) as f32 / 8_388_608.0;
    fv1.set_pot(0, pot);
    let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1 << 16));
    let measured = crossing_frequency(&out);
    let expected = 400.0 / f64::from(1u32 << 17) / core::f64::consts::TAU;
    assert!((measured - expected).abs() / expected < 0.02);
}

#[test]
fn ramp_lfo_period_and_range() {
    // Phase steps freq/16 in a (0x3FFFFF >> amp_shift) space.
    for (freq, amp, expected_period) in [
        (8192i16, RampAmp::Amp4096, 8192.0f64),
        (16384, RampAmp::Amp4096, 4096.0),
        (8192, RampAmp::Amp1024, 2048.0),
    ] {
        let mut fv1 = vm(&rdal_program(
            Instruction::Wldr {
                lfo: false,
                freq,
                amp,
            },
            LfoSel::Rmp0,
            ChoFlags::SIN,
        ));
        let n = 1 << 16;
        let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, n));
        // Sawtooth counting down: count wraps (big upward jumps).
        let wraps = out.windows(2).filter(|w| w[1] - w[0] > 0.1).count();
        let period = n as f64 / wraps as f64;
        let rel = (period - expected_period).abs() / expected_period;
        assert!(
            rel < 0.02,
            "freq {freq}: period {period}, expected {expected_period}"
        );
        // Peak matches the window: range/2^23 = (0x3FFFFF >> shift)/2^23.
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v));
        let expected_peak = (0x3F_FFFF >> (amp as u32)) as f32 / 8_388_608.0;
        assert!(
            (peak / expected_peak - 1.0).abs() < 0.01,
            "peak {peak} vs {expected_peak}"
        );
    }
}

#[test]
fn negative_ramp_rate_runs_upward() {
    let mut fv1 = vm(&rdal_program(
        Instruction::Wldr {
            lfo: false,
            freq: -8192,
            amp: RampAmp::Amp4096,
        },
        LfoSel::Rmp0,
        ChoFlags::SIN,
    ));
    let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1 << 14));
    // Rising saw: mostly increasing, with sharp downward wraps.
    let rises = out.windows(2).filter(|w| w[1] > w[0]).count();
    assert!(rises > out.len() * 9 / 10, "expected a rising ramp");
}

#[test]
fn jam_holds_ramp_at_zero() {
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wldr {
            lfo: false,
            freq: 8192,
            amp: RampAmp::Amp4096,
        },
        Instruction::Jam { lfo: false },
        Instruction::ChoRdal {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::SIN,
        },
        OUT,
    ]);
    let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, 1000));
    // JAM executes before the tick each sample, so the value stays at
    // exactly one tick from zero at most.
    let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    assert!(peak < 1e-3, "JAM failed to pin the ramp (peak {peak})");
}

#[test]
fn cho_rda_pair_with_zero_amplitude_is_transparent() {
    // With amp = 0 the LFO offset and fraction are 0, so the classic
    // interpolation pair reduces to delay[100]*(1-0) + delay[101]*0.
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 100,
            amp: 0,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::COMPC,
            addr: 100,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: 101,
        },
        OUT,
    ]);
    for n in 0..300u32 {
        let x = if n == 0 { 4_194_304 } else { 0 };
        let out = fv1.process_raw(x, 0).0;
        if n == 100 {
            // Coefficient is (1.0 - 1 LSB), so allow a 1-LSB shortfall.
            assert!((out - 4_194_304).abs() <= 1, "echo at 100, got {out}");
        } else {
            assert!(out.abs() <= 1, "unexpected output {out} at {n}");
        }
    }
}

#[test]
fn cho_sof_na_produces_crossfade_envelope() {
    // ACC = ACC_MAX * xfade: a clamped 0 -> 1 -> 0 trapezoid over the ramp
    // period (flat 0 across the wrap, slope-4 ramps, flat 1.0 in the middle).
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wldr {
            lfo: false,
            freq: 8192,
            amp: RampAmp::Amp4096,
        },
        Instruction::CLR,
        Instruction::Or { mask: 0x7F_FFFF },
        Instruction::ChoSof {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::NA,
            d: 0,
        },
        OUT,
    ]);
    let n = 1 << 15;
    let out = run_mono(&mut fv1, std::iter::repeat_n(0.0, n));
    let peak = out.iter().fold(0.0f32, |m, &v| m.max(v));
    let floor = out[100..].iter().fold(1.0f32, |m, &v| m.min(v));
    assert!(peak > 0.99, "envelope should reach ~1.0, got {peak}");
    assert!(floor < 0.01, "envelope should reach ~0.0, got {floor}");
    // Integer phase arithmetic makes the envelope exactly periodic at the
    // ramp period (8192 samples).
    for i in (0..n - 8192).step_by(999) {
        assert!(
            (out[i] - out[i + 8192]).abs() < 0.01,
            "envelope not periodic at {i}"
        );
    }
}

#[test]
fn cho_compc_pair_sums_to_unity() {
    // coeff + (1 - coeff) applied to the same delay slot returns the input.
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 300,
            amp: 1000,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::COMPC,
            addr: 2000,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: 2000,
        },
        OUT,
    ]);
    // DC input: every delay slot holds the same value, so the offset does
    // not matter and coefficients must sum to ~1. Warm up long enough for
    // the whole modulated region (2000 ± 1000 samples) to be written.
    let x = 2_000_000;
    let mut last = 0;
    for _ in 0..5000 {
        last = fv1.process_raw(x, 0).0;
    }
    assert!(
        (last - x).abs() <= 2,
        "coefficient pair not complementary: {last} vs {x}"
    );
}

#[test]
fn rptr2_reads_half_window_ahead() {
    // With a frozen ramp (rate 0 after jam), RPTR2's offset differs from the
    // base pointer by half the window: 2048 samples for Amp4096.
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wldr {
            lfo: false,
            freq: 0,
            amp: RampAmp::Amp4096,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::RPTR2,
            addr: 0,
        },
        OUT,
    ]);
    // phase = 0 -> rptr2 phase = (0 + range/2) & range = 0x1FFFFF, which is
    // offset 2047 with fraction 1023/1024 (the coefficient of this tap).
    let frac = 0x3FFi64 << 13;
    let at2047 = ((4_000_000 * frac) >> 23) as i32;
    for n in 0..2500u32 {
        let x = if n == 0 { 4_000_000 } else { 0 };
        let out = fv1.process_raw(x, 0).0;
        let expected = if n == 2047 { at2047 } else { 0 };
        assert_eq!(out, expected, "RPTR2 impulse timing at {n}");
    }
}

#[test]
fn sin_lfo_never_exceeds_the_24_bit_rails() {
    // At the maximum rate and full range the magic-circle oscillator
    // would overshoot ±1.0 without state clamping; every architectural
    // value must stay inside the 24-bit accumulator range.
    let mut fv1 = vm(&rdal_program(
        Instruction::Wlds {
            lfo: false,
            freq: 511,
            amp: 32767,
        },
        LfoSel::Sin0,
        ChoFlags::SIN,
    ));
    for n in 0..2_000_000u32 {
        let (out, _) = fv1.process_raw(0, 0);
        assert!(
            (-0x80_0000..=0x7F_FFFF).contains(&out),
            "ACC escaped the rails at sample {n}: {out}"
        );
    }
}
