//! Tests for the SpinASM assembler (`spinfv1::assemble`): golden programs
//! checked word-for-word against `Program::from_instructions`, every
//! instruction/pseudo-op, directive semantics (`EQU`, `MEM`, labels), the
//! coefficient shorthand rule, error cases (each pinned to its line number),
//! and an audio end-to-end echo assembled straight from source text.
//!
//! The assembler is `std`-only (it needs `HashMap`/`String`), so this whole
//! file is compiled out under `--no-default-features --features libm`.
#![cfg(feature = "std")]

use spinfv1::{
    AsmError, ChoFlags, Fv1, Instruction, LfoSel, Program, RampAmp, SkpCond, assemble, coeff, reg,
};

/// Assert two programs encode identically, word for word.
fn assert_same_program(source: &str, expected: &Program) {
    let got = assemble(source).unwrap_or_else(|e| panic!("assembly failed: {e}"));
    assert_eq!(
        got.words(),
        expected.words(),
        "assembled words differ from the golden program"
    );
}

// ---------------------------------------------------------------------
// Golden programs: real effects, checked word-for-word.
// ---------------------------------------------------------------------

#[test]
fn golden_echo_matches_hand_built_program() {
    let expected = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: 100,
            c: coeff::s1_9(0.5),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();

    let source = "
        ; 100-sample echo at half gain
        DELAY   MEM 200

                LDAX  ADCL
                WRA   DELAY, 0
                RDA   DELAY^, 0.5
                WRAX  DACL, 0
    ";
    assert_same_program(source, &expected);
}

#[test]
fn golden_feedback_echo_with_pot_and_labels_matches_hand_built_program() {
    // Mirrors audio_e2e.rs's `echo_with_pot_controlled_feedback`, plus an
    // unconditional SKP-to-label loop wrap to exercise forward label refs.
    let d = 400i64;
    let expected = Program::from_instructions(&[
        Instruction::Rda {
            addr: d as u16,
            c: coeff::s1_9(1.0),
        },
        Instruction::Mulx { reg: reg::POT0 },
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(1.0),
        },
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: d as u16,
            c: coeff::s1_9(1.0),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
        Instruction::Skp {
            cond: SkpCond::NONE,
            n: 0,
        },
    ])
    .unwrap();

    let source = "
        TAP     EQU 400

        START:  RDA   TAP, 1.0
                MULX  POT0
                RDAX  ADCL, 1.0
                WRA   0, 0
                RDA   TAP, 1.0
                WRAX  DACL, 0
                SKP   0, END
        END:
    ";
    assert_same_program(source, &expected);
}

#[test]
fn golden_one_pole_lowpass_matches_hand_built_program() {
    let k = 0.25;
    let expected = Program::from_instructions(&[
        Instruction::ldax(reg::ADCL),
        Instruction::Rdfx {
            reg: reg::user(0),
            c: coeff::s1_14(k),
        },
        Instruction::Wrax {
            reg: reg::user(0),
            c: coeff::s1_14(1.0),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();

    let source = "
        LDAX  ADCL
        RDFX  REG0, 0.25
        WRAX  REG0, 1.0
        WRAX  DACL, 0
    ";
    assert_same_program(source, &expected);
}

#[test]
fn golden_chorus_matches_hand_built_program() {
    let base = 1000u16;
    let amp = 256u16;
    let rate = 80u16;
    let expected = Program::from_instructions(&[
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
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();

    let source = "
        BASE    EQU 1000
        AMP     EQU 256
        RATE    EQU 80

                SKP   RUN, 1
                WLDS  SIN0, RATE, AMP
                LDAX  ADCL
                WRA   0, 0
                CHO RDA, SIN0, COMPC, BASE
                CHO RDA, SIN0, SIN, BASE+1
                WRAX  DACL, 0
    ";
    assert_same_program(source, &expected);
}

#[test]
fn golden_pitch_shifter_matches_hand_built_program() {
    let rate = 4915i16;
    let temp = 16384u16;
    let expected = Program::from_instructions(&[
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
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .unwrap();

    let source = "
        TEMP    EQU 16384

                SKP   RUN, 1
                WLDR  RMP0, 4915, 4096
                LDAX  ADCL
                WRA   0, 0
                CHO RDA, RMP0, REG|COMPC, 0
                CHO RDA, RMP0, SIN, 1
                WRA   TEMP, 0
                CHO RDA, RMP0, RPTR2|COMPC, 0
                CHO RDA, RMP0, RPTR2, 1
                CHO SOF, RMP0, NA|COMPC, 0
                CHO RDA, RMP0, NA, TEMP
                WRAX  DACL, 0
    ";
    assert_same_program(source, &expected);
}

// ---------------------------------------------------------------------
// Every instruction and pseudo-op assembles.
// ---------------------------------------------------------------------

#[test]
fn every_instruction_and_pseudo_op_assembles() {
    let source = "
        RDA    100, 0.5
        RMPA   0.25
        WRA    50, -0.5
        WRAP   50, 0.5
        RDAX   REG0, 1.0
        RDFX   REG1, -1.0
        WRAX   REG2, 0.5
        WRHX   REG3, -0.25
        WRLX   REG4, 0.75
        MAXX   REG5, 1.0
        MULX   REG6
        LOG    1.0, 2.0
        EXP    1.0, 0.5
        SOF    1.0, -0.5
        AND    $7FFFFF
        OR     $000001
        XOR    $FFFFFF
        SKP    RUN|ZRC, 2
        WLDS   SIN1, 100, 200
        WLDR   RMP1, -1000, 1024
        JAM    RMP0
        CHO RDA, SIN0, SIN, 10
        CHO SOF, SIN0, COMPC, 0.5
        CHO RDAL, RMP1
        CHO RDAL, RMP1, COS
        NOP
        CLR
        NOT
        ABSA
        LDAX   REG7
        RAW    $12345678
    ";
    let program = assemble(source).unwrap_or_else(|e| panic!("assembly failed: {e}"));
    let decoded = program.instructions();
    let expected = [
        Instruction::Rda {
            addr: 100,
            c: coeff::s1_9(0.5),
        },
        Instruction::Rmpa {
            c: coeff::s1_9(0.25),
        },
        Instruction::Wra {
            addr: 50,
            c: coeff::s1_9(-0.5),
        },
        Instruction::Wrap {
            addr: 50,
            c: coeff::s1_9(0.5),
        },
        Instruction::Rdax {
            reg: reg::user(0),
            c: coeff::s1_14(1.0),
        },
        Instruction::Rdfx {
            reg: reg::user(1),
            c: coeff::s1_14(-1.0),
        },
        Instruction::Wrax {
            reg: reg::user(2),
            c: coeff::s1_14(0.5),
        },
        Instruction::Wrhx {
            reg: reg::user(3),
            c: coeff::s1_14(-0.25),
        },
        Instruction::Wrlx {
            reg: reg::user(4),
            c: coeff::s1_14(0.75),
        },
        Instruction::Maxx {
            reg: reg::user(5),
            c: coeff::s1_14(1.0),
        },
        Instruction::Mulx { reg: reg::user(6) },
        Instruction::Log {
            c: coeff::s1_14(1.0),
            d: coeff::s4_6(2.0),
        },
        Instruction::Exp {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(0.5),
        },
        Instruction::Sof {
            c: coeff::s1_14(1.0),
            d: coeff::s_10(-0.5),
        },
        Instruction::And { mask: 0x7F_FFFF },
        Instruction::Or { mask: 0x00_0001 },
        Instruction::Xor { mask: 0xFF_FFFF },
        Instruction::Skp {
            cond: SkpCond::RUN | SkpCond::ZRC,
            n: 2,
        },
        Instruction::Wlds {
            lfo: true,
            freq: 100,
            amp: 200,
        },
        Instruction::Wldr {
            lfo: true,
            freq: -1000,
            amp: RampAmp::Amp1024,
        },
        Instruction::Jam { lfo: false },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: 10,
        },
        Instruction::ChoSof {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::COMPC,
            d: coeff::s_15(0.5),
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Rmp1,
            flags: ChoFlags(0),
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Rmp1,
            flags: ChoFlags::COS,
        },
        Instruction::NOP,
        Instruction::CLR,
        Instruction::NOT,
        Instruction::ABSA,
        Instruction::ldax(reg::user(7)),
        Instruction::Raw(0x1234_5678),
    ];
    for (i, (got, want)) in decoded.iter().zip(expected.iter()).enumerate() {
        assert_eq!(got, want, "instruction {i}: got {got}, want {want}");
    }
    for ins in &decoded[expected.len()..] {
        assert_eq!(*ins, Instruction::NOP, "padding must be NOP");
    }
}

// ---------------------------------------------------------------------
// Case-insensitivity, comments, whitespace.
// ---------------------------------------------------------------------

#[test]
fn mnemonics_registers_and_symbols_are_case_insensitive() {
    let a = assemble(
        "
        delay mem 10
        ldax adcl
        wra delay,0
        rda delay,0.5
        wrax dacl,0
    ",
    )
    .unwrap();
    let b = assemble(
        "
        DELAY MEM 10
        LDAX ADCL
        WRA DELAY,0
        RDA DELAY,0.5
        WRAX DACL,0
    ",
    )
    .unwrap();
    assert_eq!(a.words(), b.words());
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let source = "
        ; a comment on its own line

        LDAX ADCL   ; trailing comment
                            ; another blank-ish comment line
        WRAX DACL, 0
    ";
    let program = assemble(source).unwrap();
    assert_eq!(program.instructions()[0], Instruction::ldax(reg::ADCL));
    assert_eq!(
        program.instructions()[1],
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0
        }
    );
}

#[test]
fn numeric_literal_forms() {
    // Decimal, hex ($ and 0x), and binary all agree for AND's mask field.
    let hex_dollar = assemble("AND $ABCDEF").unwrap();
    let hex_0x = assemble("AND 0xABCDEF").unwrap();
    let decimal = assemble("AND 11259375").unwrap();
    assert_eq!(hex_dollar.words(), hex_0x.words());
    assert_eq!(hex_dollar.words(), decimal.words());
    assert_eq!(
        hex_dollar.instructions()[0],
        Instruction::And { mask: 0xAB_CDEF }
    );

    let binary = assemble("AND %1010_1011").unwrap();
    assert_eq!(
        binary.instructions()[0],
        Instruction::And { mask: 0b1010_1011 }
    );
}

// ---------------------------------------------------------------------
// Coefficient literal semantics and range edges.
// ---------------------------------------------------------------------

#[test]
fn coefficient_integer_literal_means_the_real_value() {
    // `SOF -2,0` means C = -2.0, not the raw bit pattern 2.
    let program = assemble("SOF -2, 0").unwrap();
    assert_eq!(
        program.instructions()[0],
        Instruction::Sof {
            c: coeff::s1_14(-2.0),
            d: coeff::s_10(0.0),
        }
    );
}

#[test]
fn s1_14_negative_two_is_exactly_representable() {
    let program = assemble("RDAX REG0, -2.0").unwrap();
    let Instruction::Rdax { c, .. } = program.instructions()[0] else {
        panic!("expected RDAX")
    };
    assert_eq!(c, -32768);
}

#[test]
fn s1_14_shorthand_plus_two_maps_to_largest_representable_value() {
    // SpinASM's conventional shorthand: +2.0 for an S1.14 field is accepted
    // and clamped to the largest representable coefficient (32767, i.e.
    // 1.99993896484375), matching real SpinASM behavior.
    let program = assemble("RDAX REG0, 2.0").unwrap();
    let Instruction::Rdax { c, .. } = program.instructions()[0] else {
        panic!("expected RDAX")
    };
    assert_eq!(c, 32767);
}

#[test]
fn s1_14_values_strictly_between_max_and_shorthand_are_errors() {
    let err = assemble("RDAX REG0, 1.99999999").unwrap_err();
    assert!(err.message().contains("out of range"), "{err}");
    let err = assemble("RDAX REG0, 2.5").unwrap_err();
    assert!(err.message().contains("out of range"), "{err}");
}

#[test]
fn s_10_and_s4_6_shorthands() {
    let p = assemble("SOF 1.0, 1.0").unwrap();
    let Instruction::Sof { c, d } = p.instructions()[0] else {
        panic!("expected SOF")
    };
    assert_eq!(c, 16384); // S1.14: 1.0 is exactly representable.
    assert_eq!(d, 1023); // S.10 shorthand: 1.0 -> max representable.

    let p = assemble("LOG 1.0, 16.0").unwrap();
    let Instruction::Log { d, .. } = p.instructions()[0] else {
        panic!("expected LOG")
    };
    assert_eq!(d, 1023); // S4.6 shorthand: 16.0 -> max representable.
}

#[test]
fn cho_sof_s_15_shorthand() {
    let p = assemble("CHO SOF, SIN0, SIN, 1.0").unwrap();
    let Instruction::ChoSof { d, .. } = p.instructions()[0] else {
        panic!("expected CHO SOF")
    };
    assert_eq!(d, 32767);
}

// ---------------------------------------------------------------------
// MEM allocation, NAME/NAME#/NAME^, overflow.
// ---------------------------------------------------------------------

#[test]
fn mem_allocates_sequentially_and_defines_start_end_mid() {
    let source = "
        A MEM 100
        B MEM 50

        EQU_A_START EQU A
        EQU_A_END   EQU A#
        EQU_A_MID   EQU A^
        EQU_B_START EQU B
        EQU_B_END   EQU B#
        EQU_B_MID   EQU B^

        RDA EQU_A_START, 0
        RDA EQU_A_END, 0
        RDA EQU_A_MID, 0
        RDA EQU_B_START, 0
        RDA EQU_B_END, 0
        RDA EQU_B_MID, 0
    ";
    let program = assemble(source).unwrap();
    let addrs: Vec<u16> = program.instructions()[..6]
        .iter()
        .map(|ins| match ins {
            Instruction::Rda { addr, .. } => *addr,
            other => panic!("expected RDA, got {other}"),
        })
        .collect();
    assert_eq!(addrs, vec![0, 100, 50, 100, 150, 125]);
}

#[test]
fn mem_offset_arithmetic() {
    let source = "
        DELAY MEM 1000

        RDA DELAY+100, 0
        RDA DELAY#-1, 0
        RDA DELAY^+3, 0
    ";
    let program = assemble(source).unwrap();
    let addrs: Vec<u16> = program.instructions()[..3]
        .iter()
        .map(|ins| match ins {
            Instruction::Rda { addr, .. } => *addr,
            other => panic!("expected RDA, got {other}"),
        })
        .collect();
    assert_eq!(addrs, vec![100, 999, 503]);
}

#[test]
fn mem_overflow_is_an_error() {
    let source = "
        A MEM 32000
        B MEM 1000
    ";
    let err = assemble(source).unwrap_err();
    assert_eq!(err.line(), 3);
    assert!(err.message().to_lowercase().contains("overflow"), "{err}");
}

#[test]
fn mem_exactly_fills_delay_memory_without_error() {
    let source = "
        A MEM 32768
        RDA A, 0
    ";
    assert!(assemble(source).is_ok());
}

// ---------------------------------------------------------------------
// An instruction whose operand token is literally `MEM`/`EQU` must never
// be silently reinterpreted as a `MEM`/`EQU` directive line: those
// keywords only start a directive when the line isn't already a real
// instruction (an instruction's first operand sits at exactly the token
// position a name-first `NAME MEM/EQU ...` directive would put its
// keyword).
// ---------------------------------------------------------------------

#[test]
fn mnemonic_first_operand_named_mem_is_not_a_directive() {
    // `MEM` is never defined here, so this must fail as an unknown symbol,
    // not as a bogus "MEM requires a size" directive complaint.
    let err = assert_err_at("MULX MEM\n", 1);
    assert!(err.message().contains("unknown symbol"), "{err}");
}

#[test]
fn mnemonic_first_operand_named_equ_is_not_a_directive() {
    let err = assert_err_at("MULX EQU\n", 1);
    assert!(err.message().contains("unknown symbol"), "{err}");
}

#[test]
fn mnemonic_second_operand_named_mem_is_not_a_directive() {
    let err = assert_err_at("RDA MEM, 0.5\n", 1);
    assert!(err.message().contains("unknown symbol"), "{err}");
}

#[test]
fn lowercase_mnemonic_operand_named_mem_is_not_a_directive() {
    let err = assert_err_at("mulx mem\n", 1);
    assert!(err.message().contains("unknown symbol"), "{err}");
}

#[test]
fn instruction_colliding_with_mem_keyword_is_never_silently_dropped() {
    // Without the comma, `JAM MEM 5` used to parse as a valid `MEM`
    // allocation directive and silently swallow the whole `JAM`
    // instruction. It must now always be an error, and in particular must
    // never shrink the instruction count.
    let source = "NOP\nJAM MEM 5\nNOP\n";
    let err = assemble(source).expect_err("colliding JAM line must be rejected, not dropped");
    assert_eq!(err.line(), 2, "{err}");
}

#[test]
fn instruction_colliding_with_mem_keyword_errors_even_if_mem_is_predefined() {
    // Same collision, but with `MEM` predefined as a real symbol first so
    // the directive-shaped tail would otherwise parse "successfully".
    let source = "MEM EQU 5\nJAM MEM 5\n";
    assert!(assemble(source).is_err());
}

// ---------------------------------------------------------------------
// Labels and SKP distances.
// ---------------------------------------------------------------------

#[test]
fn forward_skp_distance_is_exact() {
    // SKP over exactly two of three SOF instructions (mirrors
    // tests/skp.rs's `skip_distance_is_exact`).
    let source = "
        CLR
        SKP GEZ, THIRD
        SOF 1.0, 0.125
        SOF 1.0, 0.125
THIRD:  SOF 1.0, 0.125
        WRAX DACL, 0
    ";
    let program = assemble(source).unwrap();
    assert_eq!(
        program.instructions()[1],
        Instruction::Skp {
            cond: SkpCond::GEZ,
            n: 2
        }
    );
}

#[test]
fn skp_run_zrc_combined_flags() {
    let program = assemble("SKP RUN|ZRC, 0").unwrap();
    assert_eq!(
        program.instructions()[0],
        Instruction::Skp {
            cond: SkpCond::RUN | SkpCond::ZRC,
            n: 0
        }
    );
}

#[test]
fn skp_backward_label_target_is_an_error() {
    let source = "
LOOP:   NOP
        SKP RUN, LOOP
    ";
    let err = assemble(source).unwrap_err();
    assert_eq!(err.line(), 3);
    assert!(err.message().contains("after"), "{err}");
}

#[test]
fn skp_distance_over_63_is_an_error() {
    let mut source = String::from("SKP RUN, FAR\n");
    for _ in 0..70 {
        source.push_str("NOP\n");
    }
    source.push_str("FAR: NOP\n");
    let err = assemble(&source).unwrap_err();
    assert!(
        err.message().contains("63") || err.message().to_lowercase().contains("maximum"),
        "{err}"
    );
}

#[test]
fn skp_literal_distance_does_not_need_a_label() {
    let program = assemble("SKP GEZ, 63").unwrap();
    assert_eq!(
        program.instructions()[0],
        Instruction::Skp {
            cond: SkpCond::GEZ,
            n: 63
        }
    );
}

// ---------------------------------------------------------------------
// Error cases, each pinned to the reporting line number.
// ---------------------------------------------------------------------

fn assert_err_at(source: &str, expected_line: usize) -> AsmError {
    let err = assemble(source).expect_err("expected an assembly error");
    assert_eq!(err.line(), expected_line, "wrong line for error: {err}");
    err
}

#[test]
fn unknown_mnemonic_error() {
    let err = assert_err_at("NOP\nFROBNICATE REG0, 1.0\n", 2);
    assert!(
        err.message().to_lowercase().contains("mnemonic") || err.message().contains("unknown"),
        "{err}"
    );
}

#[test]
fn unknown_symbol_error() {
    let err = assert_err_at("RDA NOSUCHSYMBOL, 0.5\n", 1);
    assert!(err.message().contains("unknown symbol"), "{err}");
}

#[test]
fn wrong_operand_count_error() {
    let err = assert_err_at("RDAX REG0\n", 1);
    assert!(err.message().contains("operand"), "{err}");
    let err = assert_err_at("NOP 1\n", 1);
    assert!(err.message().contains("operand"), "{err}");
}

#[test]
fn value_out_of_range_error() {
    let err = assert_err_at("RDA 40000, 0\n", 1);
    assert!(err.message().contains("range"), "{err}");
}

#[test]
fn duplicate_symbol_error() {
    let err = assert_err_at("A EQU 1\nA EQU 2\n", 2);
    assert!(err.message().contains("duplicate"), "{err}");
}

#[test]
fn duplicate_label_error() {
    let err = assert_err_at("LOOP: NOP\nLOOP: NOP\n", 2);
    assert!(err.message().contains("duplicate"), "{err}");
}

#[test]
fn redefining_a_predefined_symbol_is_an_error() {
    let err = assert_err_at("ADCL EQU 5\n", 1);
    assert!(err.message().contains("duplicate"), "{err}");
}

#[test]
fn program_too_long_error() {
    let mut source = String::new();
    for _ in 0..129 {
        source.push_str("NOP\n");
    }
    let err = assert_err_at(&source, 129);
    assert!(
        err.message().contains("128") || err.message().to_lowercase().contains("exceeds"),
        "{err}"
    );
}

#[test]
fn mem_overflow_error_reports_the_mem_line() {
    let err = assert_err_at("A MEM 40000\n", 1);
    assert!(err.message().to_lowercase().contains("overflow"), "{err}");
}

#[test]
fn bad_number_format_error() {
    let err = assert_err_at("AND $\n", 1);
    assert!(err.message().contains("hex"), "{err}");
}

#[test]
fn skp_target_out_of_range_error() {
    let mut source = String::from("SKP RUN, FAR\n");
    for _ in 0..70 {
        source.push_str("NOP\n");
    }
    source.push_str("FAR: NOP\n");
    let err = assert_err_at(&source, 1);
    assert!(err.message().contains("63"), "{err}");
}

#[test]
fn mask_field_rejects_real_literals() {
    let err = assert_err_at("AND 0.5\n", 1);
    assert!(err.message().contains("real"), "{err}");
}

#[test]
fn error_display_includes_line_number_and_text() {
    let err = assemble("RDA NOSUCHSYMBOL, 0.5\n").unwrap_err();
    let s = err.to_string();
    assert!(s.contains("line 1"), "{s}");
    assert!(s.contains("NOSUCHSYMBOL"), "{s}");
}

// ---------------------------------------------------------------------
// Audio end-to-end: assemble from text, run an impulse through Fv1.
// ---------------------------------------------------------------------

#[test]
fn assembled_echo_program_produces_a_sample_exact_delay() {
    let source = "
        DELAY   MEM 200

                LDAX  ADCL
                WRA   DELAY, 0
                RDA   DELAY^, 1.0
                WRAX  DACL, 0
    ";
    let program = assemble(source).unwrap();
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);

    let impulse = 1_000_000;
    for n in 0..200u32 {
        let out = fv1.process_raw(if n == 0 { impulse } else { 0 }, 0).0;
        assert_eq!(out, if n == 100 { impulse } else { 0 }, "sample {n}");
    }
}

#[test]
fn assembled_feedback_echo_decays_geometrically() {
    let d = 400u16;
    let source = format!(
        "
        RDA   {d}, 1.0
        MULX  POT0
        RDAX  ADCL, 1.0
        WRA   0, 0
        RDA   {d}, 1.0
        WRAX  DACL, 0
        "
    );
    let program = assemble(&source).unwrap();
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    fv1.set_pot(0, 0.5);

    let x = 4_000_000;
    let mut peaks = Vec::new();
    for n in 0..(usize::from(d) * 4 + 1) {
        let out = fv1.process_raw(if n == 0 { x } else { 0 }, 0).0;
        if n > 0 && n % usize::from(d) == 0 {
            peaks.push(out);
        }
    }
    assert_eq!(peaks, vec![x, x / 2, x / 4, x / 8]);
}

#[test]
fn expressions_from_spin_factory_programs() {
    // The constructs Spin's own free programs use, which broke assembly
    // until the evaluator learned full SpinASM expressions.
    // 1. Unary minus on an EQU'd real (GA_DEMO_* allpass coefficient).
    let p = assemble("equ kap 0.6\n rda 100, kap\n wrap 100, -kap\n").unwrap();
    assert_eq!(
        p.instructions()[1],
        Instruction::Wrap {
            addr: 100,
            c: coeff::s1_9(-0.6)
        }
    );
    // 2. Division producing a real coefficient (GA_DEMO_PHASE `1/64`).
    let p = assemble("rdax reg0, 1/64\n").unwrap();
    assert_eq!(
        p.instructions()[0],
        Instruction::Rdax {
            reg: reg::user(0),
            c: coeff::s1_14(1.0 / 64.0)
        }
    );
    // 3. Left shift building an ADDR_PTR value (rom_fla_rev `sym < 8`).
    let p = assemble("mem buf 100\nequ mid buf+38\nor mid < 8\n").unwrap();
    assert_eq!(p.instructions()[0], Instruction::Or { mask: 38 << 8 });
    // 4. Multiplication and precedence: 2+3*4 = 14.
    let p = assemble("rda 2+3*4, 0\n").unwrap();
    assert_eq!(p.instructions()[0], Instruction::Rda { addr: 14, c: 0 });
    // 5. Exact integer division stays an integer address.
    let p = assemble("rda 4096/2, 0\n").unwrap();
    assert_eq!(p.instructions()[0], Instruction::Rda { addr: 2048, c: 0 });
    // 6. Division by zero is a line-numbered error.
    let err = assemble("nop\nrda 5/0, 0\n").unwrap_err();
    assert!(err.to_string().contains("line 2"), "{err}");
}

#[test]
fn full_expression_grammar() {
    let one = |src: &str| {
        assemble(src)
            .unwrap_or_else(|e| panic!("`{src}` failed: {e}"))
            .instructions()[0]
    };
    // Parentheses override precedence.
    assert_eq!(
        one("rda (100+2)*3, 0\n"),
        Instruction::Rda { addr: 306, c: 0 }
    );
    // Bitwise & | ^ with correct relative precedence (| loosest).
    assert_eq!(one("or $f0 & $3c | $01\n"), Instruction::Or { mask: 0x31 });
    assert_eq!(one("or $ff ^ $0f\n"), Instruction::Or { mask: 0xF0 });
    // ~ is a 24-bit complement.
    assert_eq!(one("and ~$ff\n"), Instruction::And { mask: 0xFF_FF00 });
    // ^ glued to an identifier is still the MEM midpoint suffix.
    assert_eq!(
        one("mem buf 100\nrda buf^ + 1, 0\n"),
        Instruction::Rda { addr: 51, c: 0 }
    );
    // ...while a spaced ^ between values is XOR.
    assert_eq!(
        one("equ a $0f\nequ b $ff\nor a ^ b\n"),
        Instruction::Or { mask: 0xF0 }
    );
    // Exponent-notation reals.
    assert_eq!(
        one("rdax reg0, 1e-3\n"),
        Instruction::Rdax {
            reg: reg::user(0),
            c: coeff::s1_14(0.001)
        }
    );
    assert_eq!(
        one("rdax reg0, 1.5E-2\n"),
        Instruction::Rdax {
            reg: reg::user(0),
            c: coeff::s1_14(0.015)
        }
    );
    // Power operator, right-associative, integer-exact.
    assert_eq!(one("rda 2**10, 0\n"), Instruction::Rda { addr: 1024, c: 0 });
    assert_eq!(
        one("rda 2**2**3, 0\n"),
        Instruction::Rda { addr: 256, c: 0 }
    );
    // Errors carry line numbers.
    let err = assemble("nop\nrda (5, 0\n").unwrap_err();
    assert!(err.to_string().contains("line 2"), "{err}");
    let err = assemble("or 0.5 & 1\n").unwrap_err();
    assert!(err.to_string().contains("real"), "{err}");
}
