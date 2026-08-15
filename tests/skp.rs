//! SKP condition semantics: NEG/GEZ/ZRO, the PACC-based ZRC, first-run RUN,
//! AND-combination of conditions, and jumps past the end of the program.

mod common;

use common::vm;
use spinfv1::{Instruction, SkpCond, coeff, reg};

const OUT: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};

#[test]
fn neg_gez_zro_conditions() {
    let cases: &[(SkpCond, &[(i32, bool)])] = &[
        (SkpCond::NEG, &[(-1, true), (0, false), (1, false)]),
        (SkpCond::GEZ, &[(-1, false), (0, true), (1, true)]),
        (SkpCond::ZRO, &[(-1, false), (0, true), (1, false)]),
    ];
    for &(cond, inputs) in cases {
        let mut fv1 = vm(&[
            Instruction::ldax(reg::ADCL),
            Instruction::Skp { cond, n: 1 },
            Instruction::CLR,
            OUT,
        ]);
        for &(x, skips) in inputs {
            let out = fv1.process_raw(x, 0).0;
            let expected = if skips { x } else { 0 };
            assert_eq!(out, expected, "{cond:?} with ACC = {x}");
        }
    }
}

#[test]
fn zrc_detects_sign_change_against_pacc() {
    // Set ACC to -0.5 then +0.5; at the SKP, PACC (one instruction delayed)
    // is -0.5 while ACC is +0.5 -> zero crossing -> skip.
    let mut fv1 = vm(&[
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(-0.5),
        },
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.5),
        },
        Instruction::Skp {
            cond: SkpCond::ZRC,
            n: 1,
        },
        Instruction::CLR,
        OUT,
    ]);
    let half = i32::from(coeff::s_10(0.5)) << 13;
    assert_eq!(fv1.process_raw(0, 0).0, half, "sign flip skips the CLR");

    // Same signs: no skip.
    let mut fv1 = vm(&[
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.5),
        },
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.25),
        },
        Instruction::Skp {
            cond: SkpCond::ZRC,
            n: 1,
        },
        Instruction::CLR,
        OUT,
    ]);
    assert_eq!(fv1.process_raw(0, 0).0, 0, "no sign change, CLR executes");
}

#[test]
fn run_skips_after_first_pass() {
    let mut fv1 = vm(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 2,
        },
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.25),
        },
        OUT,
        Instruction::CLR,
    ]);
    let quarter = i32::from(coeff::s_10(0.25)) << 13;
    assert_eq!(
        fv1.process_raw(0, 0).0,
        quarter,
        "first pass executes init code"
    );
    assert_eq!(
        fv1.process_raw(0, 0).0,
        quarter,
        "later passes skip; DACL persists"
    );
    fv1.reset();
    assert_eq!(fv1.process_raw(0, 0).0, quarter, "reset restores first-run");
}

#[test]
fn multiple_conditions_are_anded() {
    // NEG|ZRC: skip only when ACC negative *and* sign changed.
    let cond = SkpCond::NEG | SkpCond::ZRC;
    // ACC goes +0.5 -> -0.5: both hold.
    let mut fv1 = vm(&[
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.5),
        },
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(-0.5),
        },
        Instruction::Skp { cond, n: 1 },
        Instruction::CLR,
        OUT,
    ]);
    let neg_half = i32::from(coeff::s_10(-0.5)) << 13;
    assert_eq!(fv1.process_raw(0, 0).0, neg_half);

    // ACC goes -0.5 -> +0.5: crossing but not negative -> no skip.
    let mut fv1 = vm(&[
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(-0.5),
        },
        Instruction::Sof {
            c: 0,
            d: coeff::s_10(0.5),
        },
        Instruction::Skp { cond, n: 1 },
        Instruction::CLR,
        OUT,
    ]);
    assert_eq!(fv1.process_raw(0, 0).0, 0);
}

#[test]
fn skp_no_conditions_skips_unconditionally() {
    // Per the instruction spec, "all set conditions hold" is vacuously
    // true for an empty mask: SKP 0,N is the chip's unconditional jump
    // (SpinASM's JMP), and SKP 0,0 remains a NOP.
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Skp {
            cond: SkpCond::NONE,
            n: 1,
        },
        Instruction::CLR,
        OUT,
    ]);
    assert_eq!(fv1.process_raw(123, 0).0, 123, "CLR is jumped over");
}

#[test]
fn skip_can_jump_past_program_end() {
    let mut fv1 = vm(&[
        Instruction::ldax(reg::ADCL),
        OUT,
        Instruction::Skp {
            cond: SkpCond::GEZ,
            n: 63,
        },
    ]);
    // Just needs to not panic and still produce output.
    assert_eq!(fv1.process_raw(55, 0).0, 55);
}

#[test]
fn skip_distance_is_exact() {
    // SKP over exactly two of three SOF increments.
    let mut fv1 = vm(&[
        Instruction::CLR,
        Instruction::Skp {
            cond: SkpCond::GEZ,
            n: 2,
        },
        Instruction::Sof {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.125),
        },
        Instruction::Sof {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.125),
        },
        Instruction::Sof {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.125),
        },
        OUT,
    ]);
    let eighth = i32::from(coeff::s_10(0.125)) << 13;
    assert_eq!(fv1.process_raw(0, 0).0, eighth, "only the third SOF runs");
}
