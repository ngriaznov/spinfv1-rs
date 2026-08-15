//! End-to-end audio tests: complete FV-1 programs processing real signals,
//! verified by signal analysis (timing, spectra via zero crossings, filter
//! responses) rather than by peeking at emulator internals.

mod common;

use common::{crossing_frequency, vm};
use spinfv1::{ChoFlags, Fv1, Instruction, LfoSel, Program, RampAmp, SkpCond, coeff, reg};

const OUT_L: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};

fn sine(freq_hz: f64, n: usize, amplitude: f64) -> Vec<f32> {
    (0..n)
        .map(|i| (amplitude * (core::f64::consts::TAU * freq_hz * i as f64 / 32768.0).sin()) as f32)
        .collect()
}

#[test]
fn passthrough_is_bit_transparent() {
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        OUT_L,
        Instruction::ldax(reg::ADCR),
        Instruction::Wrax {
            reg: reg::DACR,
            c: 0,
        },
    ]);
    let mut lcg: u64 = 42;
    for _ in 0..10_000 {
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let l = (lcg >> 40) as i32 - (1 << 23);
        let r = ((lcg >> 16) & 0xFF_FFFF) as i32 - (1 << 23);
        assert_eq!(
            fv1.process_raw(l, r),
            (l, r),
            "stereo pass-through must be exact"
        );
    }
}

#[test]
fn stereo_channels_are_independent() {
    // Swap channels: DACL <- ADCR, DACR <- ADCL.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCR),
        OUT_L,
        Instruction::ldax(reg::ADCL),
        Instruction::Wrax {
            reg: reg::DACR,
            c: 0,
        },
    ]);
    assert_eq!(fv1.process_raw(111, -222), (-222, 111));
}

#[test]
fn echo_with_pot_controlled_feedback() {
    // Feedback echo where POT0 sets the feedback amount via MULX.
    let d = 400u16;
    let mut fv1 = vm(&[
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
    ]);
    fv1.set_pot(0, 0.5);
    let x = 4_000_000;
    let mut peaks = Vec::new();
    for n in 0..(usize::from(d) * 4 + 1) {
        let out = fv1.process_raw(if n == 0 { x } else { 0 }, 0).0;
        if n > 0 && n % usize::from(d) == 0 {
            peaks.push(out);
        }
    }
    // Geometric decay by exactly 1/2 per repeat.
    assert_eq!(peaks, vec![x, x / 2, x / 4, x / 8]);
}

#[test]
fn one_pole_lowpass_matches_reference_model() {
    // y[n] = y[n-1] + k (x[n] - y[n-1]) via RDFX/WRAX, k = 0.25.
    let k = 0.25;
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Rdfx {
            reg: reg::user(0),
            c: coeff::s1_14(k),
        },
        Instruction::Wrax {
            reg: reg::user(0),
            c: coeff::s1_14(1.0),
        },
        OUT_L,
    ]);
    // Mixed step + sine input, compared against a double-precision model.
    let input: Vec<f32> = sine(1000.0, 4096, 0.4)
        .into_iter()
        .enumerate()
        .map(|(i, s)| s + if i >= 100 { 0.3 } else { -0.2 })
        .collect();
    let mut y_ref = 0.0f64;
    for (n, &x) in input.iter().enumerate() {
        let out = f64::from(fv1.process(x, 0.0).0);
        // Quantize the input exactly like the ADC does.
        let xq = f64::from(spinfv1::fixed::f32_to_s23(x)) / 8_388_608.0;
        y_ref += k * (xq - y_ref);
        assert!(
            (out - y_ref).abs() < 1e-4,
            "lowpass diverged at sample {n}: {out} vs {y_ref}"
        );
    }
}

#[test]
fn shelving_lowpass_wrlx_attenuates_highs() {
    // Classic WRLX shelf: RDFX + WRLX forms a low shelf; measure that a high
    // frequency is attenuated more than a low frequency.
    let program = vec![
        Instruction::ldax(reg::ADCL),
        Instruction::Rdfx {
            reg: reg::user(0),
            c: coeff::s1_14(0.05),
        },
        Instruction::Wrlx {
            reg: reg::user(0),
            c: coeff::s1_14(-1.0),
        },
        OUT_L,
    ];
    let gain_at = |freq: f64| {
        let mut fv1 = vm(&program);
        let input = sine(freq, 16384, 0.4);
        let out: Vec<f32> = input.iter().map(|&x| fv1.process(x, 0.0).0).collect();
        let rms_in: f64 = input[8192..]
            .iter()
            .map(|&v| f64::from(v).powi(2))
            .sum::<f64>()
            .sqrt();
        let rms_out: f64 = out[8192..]
            .iter()
            .map(|&v| f64::from(v).powi(2))
            .sum::<f64>()
            .sqrt();
        rms_out / rms_in
    };
    let low = gain_at(50.0);
    let high = gain_at(8000.0);
    assert!(low > 0.9, "shelf should pass lows (gain {low})");
    assert!(high < 0.2, "shelf should cut highs (gain {high})");
}

#[test]
fn chorus_delay_tracks_the_sine_lfo() {
    // Feed a linear ramp; an interpolated modulated tap of a linear signal
    // recovers the instantaneous delay exactly, so we can verify the CHO
    // interpolation path end to end.
    let base = 1000u16;
    let amp = 256u16;
    let rate = 80u16;
    let mut fv1 = vm(&[
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
    ]);

    let k = 1.0e-5f64; // ramp slope per sample, stays < 0.5 over the run
    let n = 40_000usize;
    let lfo_period = core::f64::consts::TAU * f64::from(1u32 << 17) / f64::from(rate);
    let mut delays = Vec::new();
    for i in 0..n {
        let x = (k * i as f64) as f32;
        let y = f64::from(fv1.process(x, 0.0).0);
        if i > 4000 {
            // y = k (i - d) => d = i - y/k.
            delays.push(i as f64 - y / k);
        }
    }
    // Average over a whole number of LFO periods so the sine integrates out.
    let whole = (delays.len() as f64 / lfo_period).floor() * lfo_period;
    let mean = delays[..whole as usize].iter().sum::<f64>() / whole.floor();
    let max = delays.iter().cloned().fold(f64::MIN, f64::max);
    let min = delays.iter().cloned().fold(f64::MAX, f64::min);
    // Mean delay = base + mean fractional offset (~0.5).
    assert!(
        (mean - f64::from(base)).abs() < 3.0,
        "mean modulated delay {mean}, expected ~{base}"
    );
    // Excursion = LFO amplitude in samples.
    let excursion = (max - min) / 2.0;
    assert!(
        (excursion - f64::from(amp)).abs() < f64::from(amp) * 0.03,
        "delay excursion {excursion}, expected ~{amp}"
    );
    // Modulation period matches the LFO formula 2*pi*2^17/rate.
    let centered: Vec<f32> = delays.iter().map(|&d| (d - mean) as f32).collect();
    let measured_period = 1.0 / crossing_frequency(&centered);
    let expected_period = core::f64::consts::TAU * f64::from(1u32 << 17) / f64::from(rate);
    assert!(
        (measured_period - expected_period).abs() / expected_period < 0.02,
        "modulation period {measured_period}, expected {expected_period}"
    );
}

/// The classic Spin dual-tap crossfaded pitch shifter.
fn pitch_shifter(rate: i16) -> Fv1 {
    let temp = 16384u16; // scratch delay slot well outside the 4096 window
    vm(&[
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
}

#[test]
fn pitch_shifter_shifts_by_the_ramp_ratio() {
    // Output frequency = input * (1 + rate/16384).
    for (rate, ratio) in [(-8192i16, 0.5f64), (4915, 1.3), (16384, 2.0)] {
        let mut fv1 = pitch_shifter(rate);
        let f_in = 440.0;
        let input = sine(f_in, 60_000, 0.5);
        let out: Vec<f32> = input.iter().map(|&x| fv1.process(x, 0.0).0).collect();
        let measured = crossing_frequency(&out[20_000..]) * 32768.0;
        let expected = f_in * ratio;
        let rel = (measured - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "rate {rate}: measured {measured:.1} Hz, expected {expected:.1} Hz"
        );
        // The shifter must actually pass signal, not just noise.
        let rms: f64 = out[20_000..]
            .iter()
            .map(|&v| f64::from(v).powi(2))
            .sum::<f64>()
            .sqrt()
            / ((out.len() - 20_000) as f64).sqrt();
        assert!(rms > 0.1, "rate {rate}: output too quiet (rms {rms})");
    }
}

#[test]
fn pitch_shifter_unity_rate_is_clean() {
    // rate = 0: both taps read a fixed delay; output = delayed input.
    let mut fv1 = pitch_shifter(0);
    let input = sine(440.0, 30_000, 0.5);
    let out: Vec<f32> = input.iter().map(|&x| fv1.process(x, 0.0).0).collect();
    let measured = crossing_frequency(&out[10_000..]) * 32768.0;
    assert!(
        (measured - 440.0).abs() < 5.0,
        "unity shift moved pitch: {measured}"
    );
}

#[test]
fn tremolo_via_cho_rdal_and_mulx() {
    // Amplitude modulation the way Spin's example programs do it: read the
    // raw LFO with CHO RDAL, park it in a register, MULX the input by it.
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 400,
            amp: 32767,
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
        },
        Instruction::Wrax {
            reg: reg::user(0),
            c: 0,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Mulx { reg: reg::user(0) },
        OUT_L,
    ]);
    let input = sine(1000.0, 1 << 16, 0.5);
    let out: Vec<f32> = input.iter().map(|&x| fv1.process(x, 0.0).0).collect();
    // Envelope: RMS over windows short against the ~2059-sample LFO period.
    let env: Vec<f64> = out
        .chunks(64)
        .map(|w| (w.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / w.len() as f64).sqrt())
        .collect();
    let peak = env.iter().cloned().fold(f64::MIN, f64::max);
    let trough = env.iter().cloned().fold(f64::MAX, f64::min);
    assert!(peak > 0.2, "tremolo peak too low: {peak}");
    assert!(trough < 0.05, "tremolo trough too high: {trough}");
}

#[test]
fn program_survives_arbitrary_words() {
    // Any 128 words must load and run without panicking, and the output must
    // stay inside the saturated range.
    let mut lcg: u64 = 0xFEED_FACE_CAFE_BEEF;
    for round in 0..200 {
        let words: Vec<u32> = (0..128)
            .map(|_| {
                lcg = lcg
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (lcg >> 20) as u32
            })
            .collect();
        let program = Program::from_words(&words).unwrap();
        let mut fv1 = Fv1::new();
        fv1.load_program(&program);
        fv1.set_pot(0, 0.3);
        fv1.set_pot(1, 0.7);
        fv1.set_pot(2, 1.0);
        for i in 0..100 {
            let x = if i % 2 == 0 { 0.9 } else { -0.9 };
            let (l, r) = fv1.process(x, -x);
            assert!(
                (-1.0..=1.0).contains(&l) && (-1.0..=1.0).contains(&r),
                "round {round}: output out of range ({l}, {r})"
            );
        }
    }
}

#[test]
fn bank_bytes_roundtrip_and_run() {
    // Serialize an echo program to the 512-byte EEPROM format and run it
    // from the deserialized copy.
    let source = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: 64,
            c: coeff::s1_9(1.0),
        },
        OUT_L,
    ])
    .unwrap();
    let bytes = source.to_bytes();
    assert_eq!(bytes.len(), 512);
    let restored = Program::from_bytes(&bytes).unwrap();
    assert_eq!(restored, source);

    let mut fv1 = Fv1::new();
    fv1.load_program(&restored);
    for n in 0..100u32 {
        let out = fv1.process_raw(if n == 0 { 1_000_000 } else { 0 }, 0).0;
        assert_eq!(out, if n == 64 { 1_000_000 } else { 0 }, "sample {n}");
    }
}

#[test]
fn dc_blocking_highpass_via_wrhx() {
    // RDFX/WRHX high shelf: DC input should be strongly attenuated.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Rdfx {
            reg: reg::user(0),
            c: coeff::s1_14(0.02),
        },
        Instruction::Wrhx {
            reg: reg::user(0),
            c: coeff::s1_14(-1.0),
        },
        OUT_L,
    ]);
    let mut last = 0.0f32;
    for _ in 0..20_000 {
        last = fv1.process(0.5, 0.0).0;
    }
    assert!(last.abs() < 0.01, "DC leaked through the highpass: {last}");
}
