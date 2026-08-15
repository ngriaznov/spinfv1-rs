//! Per-instruction ALU semantics: exact fixed-point expectations computed
//! independently from the datasheet formulas.

mod common;

use common::vm;
use spinfv1::fixed::{ACC_MAX, ACC_MIN, ONE_S23};
use spinfv1::{Fv1, Instruction, coeff, reg};

/// Run one sample with raw S.23 left input and return raw DACL.
fn run_raw(fv1: &mut Fv1, input: i32) -> i32 {
    fv1.process_raw(input, 0).0
}

/// Programs below end with `WRAX DACL, 0`, so DACL captures ACC.
const OUT: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};

#[test]
fn sof_multiply_add_exact() {
    // ACC = ADCL * 0.5 + 0.25
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Sof {
            c: coeff::s1_14(0.5),
            d: coeff::s_10(0.25),
        },
        OUT,
    ]);
    for x in [0i32, 1, -1, 100_000, -100_000, ACC_MAX, ACC_MIN] {
        let expected = ((i64::from(x) * 8192) >> 14) + (256 << 13);
        assert_eq!(run_raw(&mut fv1, x), expected as i32, "SOF on {x}");
    }
}

#[test]
fn sof_saturates_at_both_rails() {
    // Two SOF 1.99993896 in a row overflow anything >= ~0.26.
    let c = coeff::s1_14(1.9999);
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Sof { c, d: 0 },
        Instruction::Sof { c, d: 0 },
        OUT,
    ]);
    let x = coeff::s_23(0.4);
    assert_eq!(run_raw(&mut fv1, x), ACC_MAX);
    assert_eq!(run_raw(&mut fv1, -x), ACC_MIN);
}

#[test]
fn bitwise_ops_sign_extend_from_bit_23() {
    // AND clearing the sign bit of a negative ACC yields a positive value.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::And { mask: 0x7F_FFFF },
        OUT,
    ]);
    assert_eq!(run_raw(&mut fv1, -1), 0x7F_FFFF, "-1 & 0x7FFFFF");
    assert_eq!(run_raw(&mut fv1, ACC_MIN), 0, "-1.0 & 0x7FFFFF");

    // OR setting the sign bit makes ACC negative.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Or { mask: 0x80_0000 },
        OUT,
    ]);
    assert_eq!(run_raw(&mut fv1, 0), ACC_MIN);
    assert_eq!(run_raw(&mut fv1, 5), ACC_MIN + 5);

    // NOT of -1 (0xFFFFFF) is 0; NOT of 0 is -1.
    let mut fv1 = vm(&[Instruction::ldax(reg::ADCL), Instruction::NOT, OUT]);
    assert_eq!(run_raw(&mut fv1, -1), 0);
    assert_eq!(run_raw(&mut fv1, 0), -1);

    // CLR always yields zero.
    let mut fv1 = vm(&[Instruction::ldax(reg::ADCL), Instruction::CLR, OUT]);
    assert_eq!(run_raw(&mut fv1, ACC_MIN), 0);
}

#[test]
fn xor_is_involution() {
    let mask = 0x5A_5A5A;
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Xor { mask },
        Instruction::Xor { mask },
        OUT,
    ]);
    for x in [0, 1, -1, 123_456, -123_456, ACC_MAX, ACC_MIN] {
        assert_eq!(run_raw(&mut fv1, x), x, "XOR twice on {x}");
    }
}

#[test]
fn rdax_accumulates_and_wrax_scales() {
    // ACC = ADCL * 1.0 + ADCL * 0.5, stored, then scaled by -1.0.
    let mut fv1 = vm(&[
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(1.0),
        },
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(0.5),
        },
        Instruction::Wrax {
            reg: reg::user(0),
            c: coeff::s1_14(-1.0),
        },
        OUT,
    ]);
    let x = 1_000_000;
    let sum = x + ((i64::from(x) * 8192) >> 14) as i32;
    assert_eq!(run_raw(&mut fv1, x), -sum);
    assert_eq!(fv1.register(reg::user(0)), sum, "WRAX stored pre-scale ACC");
}

#[test]
fn rdfx_crossfades_toward_register() {
    // REG0 = 0.25 (loaded first pass), then RDFX: acc = (x - r)*c + r.
    let c = coeff::s1_14(0.5);
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Rdfx {
            reg: reg::user(0),
            c,
        },
        Instruction::Wrax {
            reg: reg::user(0),
            c: coeff::s1_14(1.0),
        },
        OUT,
    ]);
    let x1 = coeff::s_23(0.5);
    // First pass: reg0 = 0 -> acc = x/2.
    assert_eq!(run_raw(&mut fv1, x1), x1 / 2);
    // Second pass: reg0 = x1/2 -> acc = (x1 - x1/2)/2 + x1/2 = 3*x1/4.
    assert_eq!(run_raw(&mut fv1, x1), x1 / 2 + x1 / 4);
}

#[test]
fn ldax_is_rdfx_with_zero_coefficient() {
    let mut fv1 = vm(&[Instruction::ldax(reg::ADCL), OUT]);
    for x in [0, 42, -42, ACC_MAX, ACC_MIN] {
        assert_eq!(run_raw(&mut fv1, x), x);
    }
}

#[test]
fn maxx_and_absa() {
    // MAXX REG0, 1.0 with REG0 = -0.5: ACC = max(|-0.5|, |ACC|).
    let half = coeff::s_23(0.5);
    let mut fv1 = vm(&[
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(-0.5),
        }, // ACC = -0.5
        Instruction::Wrax {
            reg: reg::user(0),
            c: 0,
        }, // REG0 = -0.5, ACC = 0
        Instruction::ldax(reg::ADCL),
        Instruction::Maxx {
            reg: reg::user(0),
            c: coeff::s1_14(1.0),
        },
        OUT,
    ]);
    assert_eq!(run_raw(&mut fv1, coeff::s_23(0.25)), half, "|reg| wins");
    assert_eq!(
        run_raw(&mut fv1, coeff::s_23(-0.75)),
        coeff::s_23(0.75),
        "|acc| wins"
    );

    // ABSA = MAXX 0,0 (REG0 here is SIN0_RATE=0, value 0).
    let mut fv1 = vm(&[Instruction::ldax(reg::ADCL), Instruction::ABSA, OUT]);
    assert_eq!(run_raw(&mut fv1, -12345), 12345);
    assert_eq!(run_raw(&mut fv1, 12345), 12345);
    assert_eq!(
        run_raw(&mut fv1, ACC_MIN),
        ACC_MAX,
        "|-1.0| saturates to +0.99999988"
    );
}

#[test]
fn mulx_squares_via_register() {
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wrax {
            reg: reg::user(0),
            c: coeff::s1_14(1.0),
        },
        Instruction::Mulx { reg: reg::user(0) },
        OUT,
    ]);
    for x in [0, half(), -half(), coeff::s_23(0.1)] {
        let expected = ((i64::from(x) * i64::from(x)) >> 23) as i32;
        assert_eq!(run_raw(&mut fv1, x), expected, "x^2 for {x}");
    }
}

fn half() -> i32 {
    coeff::s_23(0.5)
}

#[test]
fn pacc_lags_by_one_instruction_wrhx() {
    // WRHX: REG <- ACC; ACC <- ACC*C + PACC, where PACC is ACC as of the
    // start of the *previous* instruction.
    let c = coeff::s1_14(0.5);
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL), // ACC = x
        Instruction::Sof {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.125),
        }, // ACC = x + 0.125
        Instruction::Wrhx {
            reg: reg::user(0),
            c,
        }, // PACC here = x
        OUT,
    ]);
    let x = coeff::s_23(0.25);
    let after_sof = ((i64::from(x) * 16384) >> 14) as i32 + (i32::from(coeff::s_10(0.125)) << 13);
    let expected = ((i64::from(after_sof) * i64::from(c)) >> 14) as i32 + x;
    assert_eq!(run_raw(&mut fv1, x), expected);
    assert_eq!(fv1.register(reg::user(0)), after_sof);
}

#[test]
fn pacc_lags_by_one_instruction_wrlx() {
    // WRLX: REG <- ACC; ACC <- (PACC - ACC)*C + PACC.
    let c = coeff::s1_14(-1.0);
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL), // ACC = x
        Instruction::Sof {
            c: coeff::s1_14(0.5),
            d: 0,
        }, // ACC = x/2
        Instruction::Wrlx {
            reg: reg::user(0),
            c,
        }, // PACC = x
        OUT,
    ]);
    let x = coeff::s_23(0.5);
    let acc = ((i64::from(x) * 8192) >> 14) as i32; // x/2
    let expected = (((i64::from(x) - i64::from(acc)) * i64::from(c)) >> 14) as i32 + x;
    assert_eq!(run_raw(&mut fv1, x), expected);
    assert_eq!(fv1.register(reg::user(0)), acc);
}

#[test]
fn log_exact_powers_of_two() {
    // LOG 1.0, 0: ACC = log2(|ACC|)/16 in S.23 terms (S4.19 raw).
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Log {
            c: coeff::s1_14(1.0),
            d: 0,
        },
        OUT,
    ]);
    // log2(0.25) = -2 -> raw -2 * 2^19 = -1048576.
    assert_eq!(run_raw(&mut fv1, coeff::s_23(0.25)), -2 << 19);
    // log2(0.5) = -1.
    assert_eq!(run_raw(&mut fv1, coeff::s_23(0.5)), -1 << 19);
    // Sign is ignored: LOG uses |ACC|.
    assert_eq!(run_raw(&mut fv1, coeff::s_23(-0.5)), -1 << 19);
    // log2(0) saturates to the most negative S4.19 value (-16.0).
    assert_eq!(run_raw(&mut fv1, 0), ACC_MIN);
}

#[test]
fn exp_exact_powers_of_two() {
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Exp {
            c: coeff::s1_14(1.0),
            d: 0,
        },
        OUT,
    ]);
    // 2^(-1) = 0.5 (ACC read as S4.19: raw -2^19).
    assert_eq!(run_raw(&mut fv1, -1 << 19), coeff::s_23(0.5));
    // 2^(-2) = 0.25.
    assert_eq!(run_raw(&mut fv1, -2 << 19), coeff::s_23(0.25));
    // ACC >= 0 saturates the exponential at ~1.0.
    assert_eq!(run_raw(&mut fv1, 0), ACC_MAX);
    assert_eq!(run_raw(&mut fv1, 12345), ACC_MAX);
}

#[test]
fn exp_undoes_log() {
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Log {
            c: coeff::s1_14(1.0),
            d: 0,
        },
        Instruction::Exp {
            c: coeff::s1_14(1.0),
            d: 0,
        },
        OUT,
    ]);
    for x in [0.9, 0.5, 0.25, 0.1, 0.01, 0.001] {
        let raw = coeff::s_23(x);
        let out = run_raw(&mut fv1, raw);
        // The S4.19 log-domain quantum maps to ~|x|*2^23*ln(2)/2^19 ≈ 10 LSB
        // at full scale after EXP, so a few LSB of drift is inherent.
        let err = (out - raw).abs();
        assert!(err <= 8, "EXP(LOG({x})) drifted by {err} LSB");
    }
}

#[test]
fn log_c_and_d_are_applied() {
    // LOG 0.5, 2.0: acc = 0.5*log2|acc| + 2.0 (S4.19 domain).
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Log {
            c: coeff::s1_14(0.5),
            d: coeff::s4_6(2.0),
        },
        OUT,
    ]);
    // log2(0.25) = -2; 0.5*-2 + 2 = 1.0 in S4.19 -> raw 2^19.
    assert_eq!(run_raw(&mut fv1, coeff::s_23(0.25)), 1 << 19);
}

#[test]
fn writes_to_input_registers_are_overwritten_each_sample() {
    // A program that trashes ADCL still sees fresh input next sample.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        OUT,
        Instruction::CLR,
        Instruction::Wrax {
            reg: reg::ADCL,
            c: 0,
        },
    ]);
    assert_eq!(run_raw(&mut fv1, 111), 111);
    assert_eq!(run_raw(&mut fv1, 222), 222);
}

#[test]
fn pot_registers_track_set_pot() {
    let mut fv1 = vm(&[Instruction::ldax(reg::POT1), OUT]);
    fv1.set_pot(1, 0.5);
    assert_eq!(run_raw(&mut fv1, 0), coeff::s_23(0.5));
    fv1.set_pot(1, 1.0);
    assert_eq!(run_raw(&mut fv1, 0), ACC_MAX, "pot 1.0 clamps to ACC_MAX");
    fv1.set_pot(1, -3.0);
    assert_eq!(run_raw(&mut fv1, 0), 0, "pots clamp to [0,1]");
}

#[test]
fn acc_persists_across_samples() {
    // No clear between passes: ACC carries over.
    let mut fv1 = vm(&[
        Instruction::Sof {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.125),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: coeff::s1_14(1.0),
        },
    ]);
    let step = i32::from(coeff::s_10(0.125)) << 13;
    assert_eq!(run_raw(&mut fv1, 0), step);
    assert_eq!(run_raw(&mut fv1, 0), 2 * step);
    assert_eq!(run_raw(&mut fv1, 0), 3 * step);
}

#[test]
fn raw_words_execute_as_nop() {
    let program = spinfv1::Program::from_words(&[
        Instruction::ldax(reg::ADCL).encode(),
        0xDEAD_BE1F, // undefined opcode
        OUT.encode(),
    ])
    .unwrap();
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    assert_eq!(fv1.process_raw(4242, 0).0, 4242);
}

#[test]
fn dac_register_holds_last_value() {
    // DACL written on the first pass only (guarded by SKP RUN) persists.
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: spinfv1::SkpCond::RUN,
            n: 2,
        },
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.25),
        },
        OUT,
    ]);
    let first = fv1.process_raw(0, 0).0;
    let second = fv1.process_raw(0, 0).0;
    assert_eq!(first, i32::from(coeff::s_10(0.25)) << 13);
    assert_eq!(second, first, "DACL persists when not rewritten");
}

#[test]
fn full_scale_input_is_clamped() {
    let mut fv1 = vm(&[Instruction::ldax(reg::ADCL), OUT]);
    assert_eq!(fv1.process(2.0, 0.0).0, ACC_MAX as f32 / ONE_S23 as f32);
    assert_eq!(fv1.process(-2.0, 0.0).0, -1.0);
}
