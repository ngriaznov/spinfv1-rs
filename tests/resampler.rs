//! Boundary-resampling tests: the converter's fidelity and the
//! `HostedFv1` wrapper's rate authenticity.
//!
//! Run with `cargo test --features resampler`.
#![cfg(feature = "resampler")]

mod common;

use common::crossing_frequency;
use spinfv1::resampler::{CHIP_RATE, HostedFv1, StreamResampler};
use spinfv1::{Fv1, Instruction, Program, coeff, reg};

const HOST_RATE: f64 = 48_000.0;

fn passthrough() -> Fv1 {
    let program = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
        Instruction::ldax(reg::ADCR),
        Instruction::Wrax {
            reg: reg::DACR,
            c: 0,
        },
    ])
    .unwrap();
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    fv1
}

#[test]
fn converter_has_unity_dc_gain() {
    for (from, to) in [(HOST_RATE, CHIP_RATE), (CHIP_RATE, HOST_RATE)] {
        let mut rs = StreamResampler::new(from, to);
        let mut out = Vec::new();
        for _ in 0..2000 {
            rs.push((0.5, -0.25));
            while let Some(f) = rs.pull() {
                out.push(f);
            }
        }
        // Skip the kernel warmup, then the plateau must hold DC exactly.
        for &(l, r) in &out[200..] {
            assert!((l - 0.5).abs() < 1e-4, "left DC drifted: {l}");
            assert!((r + 0.25).abs() < 1e-4, "right DC drifted: {r}");
        }
    }
}

#[test]
fn hosted_chip_preserves_tone_frequency_and_level() {
    let mut hosted = HostedFv1::new(passthrough(), HOST_RATE);
    let n = 48_000;
    let freq = 440.0;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = (core::f64::consts::TAU * freq * i as f64 / HOST_RATE).sin() as f32 * 0.5;
        out.push(hosted.process(x, x).0);
    }
    let settled = &out[hosted.latency() + 2000..];
    let measured = crossing_frequency(settled) * HOST_RATE;
    assert!(
        (measured - freq).abs() < 1.0,
        "tone moved: {measured} Hz vs {freq} Hz"
    );
    let peak = settled.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    assert!(
        (peak - 0.5).abs() < 0.01,
        "passband level off: peak {peak}, expected 0.5"
    );
}

#[test]
fn hosted_chip_keeps_hardware_delay_times() {
    // A 3277-sample chip delay is 100 ms; at 48 kHz the echo must land
    // 4800 host samples after the impulse regardless of host rate.
    let chip_delay = 3277u16;
    let program = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: chip_delay,
            c: coeff::s1_9(1.0),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    let mut hosted = HostedFv1::new(fv1, HOST_RATE);

    let n = 8000;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = if i == 0 { 0.9 } else { 0.0 };
        out.push(hosted.process(x, 0.0).0);
    }
    let peak_at = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0 as f64;
    let expected = f64::from(chip_delay) / CHIP_RATE * HOST_RATE + hosted.latency() as f64;
    assert!(
        (peak_at - expected).abs() <= 3.0,
        "echo at host sample {peak_at}, expected ~{expected}"
    );
}

#[test]
fn latency_is_reported_and_small() {
    let hosted = HostedFv1::new(passthrough(), HOST_RATE);
    let latency = hosted.latency();
    // Two 32-tap kernels: tens of samples, well under 2 ms at 48 kHz.
    assert!(
        (16..96).contains(&latency),
        "unexpected latency: {latency} host samples"
    );
}

#[test]
fn converter_rejects_aliasing_above_chip_nyquist() {
    // A 30 kHz tone at a 96 kHz input rate sits far above the chip's
    // 16.384 kHz Nyquist; the down-converter's kernel must stop it.
    let mut rs = StreamResampler::new(96_000.0, CHIP_RATE);
    let mut out = Vec::new();
    for i in 0..48_000 {
        let x = (core::f64::consts::TAU * 30_000.0 * f64::from(i) / 96_000.0).sin() as f32;
        rs.push((x, x));
        while let Some(f) = rs.pull() {
            out.push(f.0);
        }
    }
    let settled = &out[1000..];
    let rms = (settled
        .iter()
        .map(|&v| f64::from(v) * f64::from(v))
        .sum::<f64>()
        / settled.len() as f64)
        .sqrt();
    let db = 20.0 * rms.log10();
    assert!(
        db < -80.0,
        "aliasing leak at {db:.1} dB (unit-amplitude tone)"
    );
}

#[test]
fn clock_mod_scales_delay_times() {
    // The crystal-swap: the same 3277-sample chip delay is 100 ms at
    // the stock 32,768 Hz clock, 200 ms underclocked to 16,384 Hz and
    // 50 ms overclocked to 65,536 Hz, measured in host samples.
    let chip_delay = 3277u16;
    let program = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: chip_delay,
            c: coeff::s1_9(1.0),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();

    for chip_rate in [16_384.0, 32_768.0, 65_536.0] {
        let mut fv1 = Fv1::new();
        fv1.load_program(&program);
        let mut hosted = HostedFv1::with_chip_rate(fv1, HOST_RATE, chip_rate);
        assert_eq!(hosted.chip_rate(), chip_rate);

        let n = (f64::from(chip_delay) / chip_rate * HOST_RATE) as usize + 2000;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let x = if i == 0 { 0.9 } else { 0.0 };
            out.push(hosted.process(x, 0.0).0);
        }
        let peak_at = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0 as f64;
        let expected = f64::from(chip_delay) / chip_rate * HOST_RATE + hosted.latency() as f64;
        assert!(
            (peak_at - expected).abs() <= 3.0,
            "chip_rate {chip_rate}: echo at {peak_at}, expected ~{expected}"
        );
    }
}

#[test]
fn clock_mod_switch_preserves_chip_state() {
    // Re-clocking a running chip keeps its delay RAM: an impulse
    // written before the switch still comes out afterwards, at the new
    // clock's timing.
    let chip_delay = 3277u16;
    let program = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: chip_delay,
            c: coeff::s1_9(1.0),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    let mut hosted = HostedFv1::new(fv1, HOST_RATE);

    // Feed the impulse at the stock clock, then swap the crystal right
    // away, well before the 100 ms echo is due.
    hosted.process(0.9, 0.0);
    for _ in 0..100 {
        hosted.process(0.0, 0.0);
    }
    hosted.set_chip_rate(16_384.0);
    assert_eq!(hosted.chip_rate(), 16_384.0);

    let mut peak = 0.0f32;
    for _ in 0..20_000 {
        let (l, _) = hosted.process(0.0, 0.0);
        peak = peak.max(l.abs());
    }
    assert!(peak > 0.5, "echo lost across the clock switch: peak {peak}");
}
