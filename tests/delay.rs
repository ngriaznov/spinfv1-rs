//! Delay RAM semantics: the decrementing base pointer, RDA/WRA/WRAP timing,
//! the LR register, RMPA indirect addressing, and 32K wraparound.

mod common;

use common::vm;
use spinfv1::{Instruction, coeff, reg};

const OUT: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};

/// Write input at delay 0, read tap at `d`: out[n] = in[n - d].
fn echo_program(d: u16) -> Vec<Instruction> {
    vec![
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: d,
            c: coeff::s1_9(1.0),
        },
        OUT,
    ]
}

#[test]
fn delay_line_is_sample_exact() {
    let d = 100;
    let mut fv1 = vm(&echo_program(d));
    // Impulse of 0.5 at n = 0.
    for n in 0..300u32 {
        let x = if n == 0 { 4_194_304 } else { 0 };
        let (out, _) = fv1.process_raw(x, 0);
        let expected = if n == u32::from(d) { 4_194_304 } else { 0 };
        assert_eq!(out, expected, "echo tap at sample {n}");
    }
}

#[test]
fn delay_wraps_around_32k() {
    // A tap one sample short of the full memory still lines up exactly.
    let d = 32_767;
    let mut fv1 = vm(&echo_program(d));
    for n in 0..33_000u32 {
        let x = if n == 0 { 1_000_000 } else { 0 };
        let (out, _) = fv1.process_raw(x, 0);
        let expected = if n == 32_767 { 1_000_000 } else { 0 };
        assert_eq!(out, expected, "sample {n}");
    }
}

#[test]
fn rda_applies_coefficient_with_truncation() {
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: 10,
            c: coeff::s1_9(0.5),
        },
        OUT,
    ]);
    let x = 999_999; // odd, so truncation matters
    let mut last = 0;
    for n in 0..=10u32 {
        last = fv1.process_raw(if n == 0 { x } else { 0 }, 0).0;
    }
    assert_eq!(last, ((i64::from(x) * 256) >> 9) as i32);
}

#[test]
fn wra_scales_acc_after_write() {
    // WRA addr, 0.5 leaves ACC halved.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra {
            addr: 0,
            c: coeff::s1_9(0.5),
        },
        OUT,
    ]);
    let x = 2_000_000;
    assert_eq!(fv1.process_raw(x, 0).0, ((i64::from(x) * 256) >> 9) as i32);
}

#[test]
fn wrap_adds_last_read() {
    // RDA sets LR; WRAP computes ACC*C + LR.
    // Preload delay[5] (as seen later) by feeding an impulse through WRA.
    let c = coeff::s1_9(0.25);
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 }, // delay[0] = x, ACC = 0
        Instruction::Rda { addr: 5, c: 0 }, // LR = delay[5], ACC += 0
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.5),
        }, // ACC = 0.5
        Instruction::Wrap { addr: 100, c }, // delay[100] = 0.5; ACC = 0.5*0.25 + LR
        OUT,
    ]);
    let x = 3_000_000;
    let half = i32::from(coeff::s_10(0.5)) << 13;
    let mut outs = Vec::new();
    for n in 0..8u32 {
        outs.push(fv1.process_raw(if n == 0 { x } else { 0 }, 0).0);
    }
    let base = ((i64::from(half) * i64::from(c)) >> 9) as i32;
    // Before the impulse reaches tap 5, LR = 0.
    assert_eq!(outs[0], base);
    // At n = 5 the impulse sits at delay[5], so LR = x.
    assert_eq!(outs[5], base + x);
    assert_eq!(outs[6], base);
}

#[test]
fn rmpa_reads_via_addr_ptr() {
    // ADDR_PTR wants the delay address in bits 23..8.
    let target = 123i32;
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCR),
        Instruction::Wra { addr: 123, c: 0 }, // delay[123] = right input
        Instruction::ldax(reg::ADCL),
        Instruction::Wrax {
            reg: reg::ADDR_PTR,
            c: 0,
        },
        Instruction::Rmpa {
            c: coeff::s1_9(1.0),
        },
        OUT,
    ]);
    let value = 777_777;
    let (out, _) = fv1.process_raw(target << 8, value);
    assert_eq!(out, value, "RMPA fetched delay[ADDR_PTR >> 8]");
}

#[test]
fn feedback_echo_decays_geometrically() {
    // Classic feedback delay: y = delay[d]; delay[0] = x + y*g.
    let d = 50;
    let g = coeff::s1_9(0.5);
    let mut fv1 = vm(&[
        Instruction::Rda { addr: d, c: g }, // ACC = fb * 0.5
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(1.0),
        },
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: d,
            c: coeff::s1_9(1.0),
        },
        OUT,
    ]);
    let x = 4_000_000; // ~0.477
    let mut taps = Vec::new();
    for n in 0..(usize::from(d) * 4 + 1) {
        let out = fv1.process_raw(if n == 0 { x } else { 0 }, 0).0;
        if n % usize::from(d) == 0 && n > 0 {
            taps.push(out);
        }
    }
    // Exact halving each round trip (x chosen to divide cleanly).
    assert_eq!(taps, vec![x, x / 2, x / 4, x / 8]);
}

#[test]
fn delay_quantization_models_compressed_float() {
    // The 14-bit delay word is a compressed float: the magnitude keeps its
    // top 10 significant bits, so the quantization step scales with level.
    let round_trip = |x: i32| {
        let mut fv1 = vm(&echo_program(1));
        fv1.set_delay_quantization(true);
        fv1.process_raw(x, 0);
        fv1.process_raw(0, 0).0
    };

    // Loud signal (21 significant bits): step 2^11, top 10 bits survive.
    assert_eq!(round_trip(0x0012_3457), 0x0012_3000);
    // Quiet signal (13 significant bits): step is the 2^6 floor — much
    // finer than a linear 14-bit store's fixed 2^10 step.
    assert_eq!(round_trip(0x1234), 0x1200);
    // Negative values quantize symmetrically.
    assert_eq!(round_trip(-0x0012_3457), -0x0012_3000);

    // Fresh VM without quantization keeps every bit.
    let mut fv1 = vm(&echo_program(1));
    fv1.process_raw(0x0012_3457, 0);
    assert_eq!(fv1.process_raw(0, 0).0, 0x0012_3457);
}

#[test]
fn reset_clears_delay_ram() {
    let mut fv1 = vm(&echo_program(10));
    fv1.process_raw(1_000_000, 0);
    fv1.reset();
    for n in 0..50u32 {
        assert_eq!(fv1.process_raw(0, 0).0, 0, "stale echo after reset at {n}");
    }
}

#[test]
fn randomize_delay_ram_is_deterministic_and_readable() {
    let mut a = spinfv1::Fv1::new();
    let mut b = spinfv1::Fv1::new();
    a.randomize_delay_ram(7);
    b.randomize_delay_ram(7);
    assert_eq!(a.delay_ram(), b.delay_ram());
    assert!(a.delay_ram().iter().any(|&v| v != 0));
    assert!(
        a.delay_ram()
            .iter()
            .all(|&v| (-(1 << 23)..1 << 23).contains(&v))
    );

    // A program that only reads never-written RAM hears the pattern.
    let program = spinfv1::assemble("rda 1000, 1.0\nwrax DACL, 0.0\n").unwrap();
    a.load_program(&program);
    a.randomize_delay_ram(7);
    let mut heard = false;
    for _ in 0..64 {
        let (l, _r) = a.process_raw(0, 0);
        heard |= l != 0;
    }
    assert!(heard);

    // Different seed, different pattern.
    b.randomize_delay_ram(8);
    assert_ne!(a.delay_ram(), b.delay_ram());
}

#[test]
fn randomize_registers_covers_general_purpose_only() {
    let mut chip = spinfv1::Fv1::new();
    chip.randomize_registers(3);
    assert!((0x20..0x40).any(|r| chip.register(r) != 0));
    for r in 0..0x20u8 {
        assert_eq!(chip.register(r), 0);
    }
    let mut again = spinfv1::Fv1::new();
    again.randomize_registers(3);
    for r in 0x20..0x40u8 {
        assert_eq!(chip.register(r), again.register(r));
    }
}
