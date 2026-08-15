//! Deliberate per-instruction conformance matrix against the FV-1
//! instruction-set specification.
//!
//! Every [`Instruction`] variant has a case here whose expected value is
//! computed from the *specification formula* — audited against Spin
//! Semiconductor's official instruction sheet
//! (spinsemi.com/knowledge_base/cheat.html) and the FV-1 datasheet
//! (SPN1001-DS-170829) — using the independent spec-math helpers below,
//! never the crate's own arithmetic.
//!
//! Completeness is enforced twice:
//! * [`variant_name`] is an exhaustive `match` with no wildcard arm, so
//!   adding an `Instruction` variant fails compilation until it is named;
//! * [`spec_matrix`] asserts that every named variant has at least one
//!   registered case, so naming it without testing it fails the test.

mod common;

use common::vm;
use spinfv1::{ChoFlags, Fv1, Instruction, LfoSel, RampAmp, SkpCond, reg};

// ---------------------------------------------------------------------
// Spec math: an independent model of the datasheet's arithmetic.
// Written from the spec ("24-bit S.23 accumulator, saturating adds,
// truncating multiplies, coefficients in S1.14/S1.9/S.10/S4.6/S.15"),
// deliberately not importing spinfv1::fixed.
// ---------------------------------------------------------------------

const ONE: i64 = 1 << 23;
const MAX: i64 = ONE - 1;
const MIN: i64 = -ONE;

/// Saturate to the 24-bit accumulator range.
fn sat(v: i64) -> i64 {
    v.clamp(MIN, MAX)
}

/// Multiply an S.23 value by a coefficient with `frac` fractional bits,
/// truncating toward negative infinity (the hardware multiplier drops
/// low bits of the product).
fn mulq(x: i64, c: i64, frac: u32) -> i64 {
    let p = (x as i128) * (c as i128);
    (p >> frac) as i64
}

/// Quantize a real coefficient to a raw field value (round to nearest).
fn q(v: f64, frac: u32) -> i64 {
    (v * f64::from(1u32 << frac)).round() as i64
}

const OUT: Instruction = Instruction::Wrax {
    reg: reg::DACL,
    c: 0,
};

/// Run one sample with raw ACC preset via ADCL + LDAX and return raw DACL.
fn eval1(body: &[Instruction], acc_in: i64) -> i64 {
    let mut program = vec![Instruction::ldax(reg::ADCL)];
    program.extend_from_slice(body);
    program.push(OUT);
    let mut fv1 = vm(&program);
    i64::from(fv1.process_raw(acc_in as i32, 0).0)
}

struct Case {
    variant: &'static str,
    /// The behavior formula, per Spin's instruction sheet.
    spec: &'static str,
    run: fn(),
}

/// Exhaustive variant naming: adding an `Instruction` variant will not
/// compile until it appears here — and then `spec_matrix` will fail until
/// a spec case covers it.
fn variant_name(i: &Instruction) -> &'static str {
    match i {
        Instruction::Rda { .. } => "RDA",
        Instruction::Rmpa { .. } => "RMPA",
        Instruction::Wra { .. } => "WRA",
        Instruction::Wrap { .. } => "WRAP",
        Instruction::Rdax { .. } => "RDAX",
        Instruction::Rdfx { .. } => "RDFX",
        Instruction::Wrax { .. } => "WRAX",
        Instruction::Wrhx { .. } => "WRHX",
        Instruction::Wrlx { .. } => "WRLX",
        Instruction::Maxx { .. } => "MAXX",
        Instruction::Mulx { .. } => "MULX",
        Instruction::Log { .. } => "LOG",
        Instruction::Exp { .. } => "EXP",
        Instruction::Sof { .. } => "SOF",
        Instruction::And { .. } => "AND",
        Instruction::Or { .. } => "OR",
        Instruction::Xor { .. } => "XOR",
        Instruction::Skp { .. } => "SKP",
        Instruction::Wlds { .. } => "WLDS",
        Instruction::Wldr { .. } => "WLDR",
        Instruction::Jam { .. } => "JAM",
        Instruction::ChoRda { .. } => "CHO RDA",
        Instruction::ChoSof { .. } => "CHO SOF",
        Instruction::ChoRdal { .. } => "CHO RDAL",
        Instruction::Raw(_) => "RAW",
    }
}

/// One sample instruction per variant, used to enumerate what must be
/// covered. `variant_name`'s exhaustive match guards this list: a new
/// variant breaks compilation there, prompting an entry here and a case
/// below.
fn all_variants() -> Vec<Instruction> {
    vec![
        Instruction::Rda { addr: 0, c: 0 },
        Instruction::Rmpa { c: 0 },
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Wrap { addr: 0, c: 0 },
        Instruction::Rdax { reg: 0, c: 0 },
        Instruction::Rdfx { reg: 0, c: 0 },
        Instruction::Wrax { reg: 0, c: 0 },
        Instruction::Wrhx { reg: 0, c: 0 },
        Instruction::Wrlx { reg: 0, c: 0 },
        Instruction::Maxx { reg: 0, c: 0 },
        Instruction::Mulx { reg: 0 },
        Instruction::Log { c: 0, d: 0 },
        Instruction::Exp { c: 0, d: 0 },
        Instruction::Sof { c: 0, d: 0 },
        Instruction::And { mask: 0 },
        Instruction::Or { mask: 0 },
        Instruction::Xor { mask: 0 },
        Instruction::Skp {
            cond: SkpCond::NONE,
            n: 0,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 0,
            amp: 0,
        },
        Instruction::Wldr {
            lfo: false,
            freq: 0,
            amp: RampAmp::Amp4096,
        },
        Instruction::Jam { lfo: false },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: 0,
        },
        Instruction::ChoSof {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            d: 0,
        },
        Instruction::ChoRdal {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
        },
        Instruction::Raw(0xFFFF_FFFF),
    ]
}

// ---------------------------------------------------------------------
// The spec cases.
// ---------------------------------------------------------------------

const CASES: &[Case] = &[
    Case {
        variant: "SOF",
        spec: "SOF C,D: ACC = C * ACC + D  (C: S1.14, D: S.10)",
        run: || {
            let (c, d) = (q(-1.5, 14), q(0.25, 10));
            let acc = 3_211_009; // odd, exercises truncation
            let expected = sat(mulq(acc, c, 14) + (d << 13));
            let got = eval1(
                &[Instruction::Sof {
                    c: c as i16,
                    d: d as i16,
                }],
                acc,
            );
            assert_eq!(got, expected);
            // Saturation at both rails is part of the spec ("the
            // accumulator saturates like an analog circuit").
            let big = q(1.9999, 14) as i16;
            assert_eq!(eval1(&[Instruction::Sof { c: big, d: 0 }], 7_000_000), MAX);
            assert_eq!(eval1(&[Instruction::Sof { c: big, d: 0 }], -7_000_000), MIN);
        },
    },
    Case {
        variant: "AND",
        spec: "AND M: ACC = ACC & M  (24-bit mask, bit 23 is the sign)",
        run: || {
            let acc = -2_054_847; // 0xFF..E0A241 sign-extended
            let mask = 0x0F_F0F0u32;
            let expected = acc & i64::from(mask);
            let got = eval1(&[Instruction::And { mask }], acc);
            assert_eq!(got, expected);
        },
    },
    Case {
        variant: "OR",
        spec: "OR M: ACC = ACC | M (result sign-extended from bit 23)",
        run: || {
            let acc = 5;
            let mask = 0x80_0001u32; // sets the sign bit
            let expected = ((acc | i64::from(mask)) << 40) >> 40; // sx24
            let got = eval1(&[Instruction::Or { mask }], acc);
            assert_eq!(got, expected);
            assert!(got < 0, "bit 23 set must read back negative");
        },
    },
    Case {
        variant: "XOR",
        spec: "XOR M: ACC = ACC ^ M; XOR $FFFFFF is the NOT pseudo-op",
        run: || {
            let acc = 0x12_3456;
            let mask = 0xFF_FFFFu32;
            let expected = ((acc ^ i64::from(mask)) << 40) >> 40;
            assert_eq!(eval1(&[Instruction::Xor { mask }], acc), expected);
            assert_eq!(eval1(&[Instruction::NOT], -1), 0, "NOT(-1) == 0");
        },
    },
    Case {
        variant: "LOG",
        spec: "LOG K1,K2: ACC = (LOG2(ACC)/16)*K1 + K2 \
               (K1: 16b S1.14; K2: 11b S4.6 per the SPINAsm manual, an \
               offset in the /16-normalized log domain, -16 to +15.98); \
               LOG(0) pins to the domain floor",
        run: || {
            // log2(0.25) = -2.0 exactly: raw S4.19 result -2 << 19.
            let acc = q(0.25, 23);
            let (c, d) = (q(0.5, 14), q(2.0, 6));
            let expected = sat(mulq(-2 << 19, c, 14) + (d << 13));
            let got = eval1(
                &[Instruction::Log {
                    c: c as i16,
                    d: d as i16,
                }],
                acc,
            );
            assert_eq!(got, expected);
            // |ACC| per spec: negative input gives the same log.
            assert_eq!(
                eval1(
                    &[Instruction::Log {
                        c: c as i16,
                        d: d as i16
                    }],
                    -acc
                ),
                expected
            );
            // LOG of zero: most negative S4.19 value (-16.0).
            assert_eq!(eval1(&[Instruction::Log { c: 1 << 14, d: 0 }], 0), MIN);
        },
    },
    Case {
        variant: "EXP",
        spec: "EXP K1,K2: ACC = (2^(ACC*16))*K1 + K2 \
               (K1: 16b S1.14; K2: 11b, -1 to +0.999023); the exponential \
               saturates at 1.0 for ACC >= 0",
        run: || {
            // 2^(-2) = 0.25 exactly.
            let acc = -2 << 19;
            let (c, d) = (q(0.5, 14), q(-0.125, 10));
            let expected = sat(mulq(q(0.25, 23), c, 14) + (d << 13));
            let got = eval1(
                &[Instruction::Exp {
                    c: c as i16,
                    d: d as i16,
                }],
                acc,
            );
            assert_eq!(got, expected);
            // ACC >= 0: 2^ACC >= 1.0 saturates to the max S.23 value.
            let got = eval1(&[Instruction::Exp { c: 1 << 14, d: 0 }], 12345);
            assert_eq!(got, MAX);
        },
    },
    Case {
        variant: "RDAX",
        spec: "RDAX REG,C: ACC = ACC + C * REG  (C: S1.14)",
        run: || {
            let (r, c) = (1_234_567i64, q(0.6, 14));
            // Preload REG0 with r, set ACC, then RDAX.
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCR),
                Instruction::Wrax {
                    reg: reg::user(0),
                    c: 0,
                },
                Instruction::ldax(reg::ADCL),
                Instruction::Rdax {
                    reg: reg::user(0),
                    c: c as i16,
                },
                OUT,
            ]);
            let acc = -777_777i64;
            let expected = sat(acc + mulq(r, c, 14));
            assert_eq!(i64::from(fv1.process_raw(acc as i32, r as i32).0), expected);
        },
    },
    Case {
        variant: "RDFX",
        spec: "RDFX REG,C: ACC = REG + C * (ACC - REG); RDFX REG,0 is LDAX",
        run: || {
            let (r, c) = (2_000_001i64, q(0.3, 14));
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCR),
                Instruction::Wrax {
                    reg: reg::user(1),
                    c: 0,
                },
                Instruction::ldax(reg::ADCL),
                Instruction::Rdfx {
                    reg: reg::user(1),
                    c: c as i16,
                },
                OUT,
            ]);
            let acc = -3_000_003i64;
            let expected = sat(r + mulq(acc - r, c, 14));
            assert_eq!(i64::from(fv1.process_raw(acc as i32, r as i32).0), expected);
            // LDAX: plain register load.
            assert_eq!(eval1(&[], 424_242), 424_242);
        },
    },
    Case {
        variant: "WRAX",
        spec: "WRAX REG,C: REG = ACC, then ACC = C * ACC",
        run: || {
            let c = q(-0.5, 14);
            let acc = 1_048_577i64;
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCL),
                Instruction::Wrax {
                    reg: reg::user(2),
                    c: c as i16,
                },
                OUT,
            ]);
            let got = i64::from(fv1.process_raw(acc as i32, 0).0);
            assert_eq!(got, sat(mulq(acc, c, 14)));
            assert_eq!(i64::from(fv1.register(reg::user(2))), acc, "REG = ACC");
        },
    },
    Case {
        variant: "WRHX",
        spec: "WRHX REG,C: REG = ACC, ACC = ACC*C + PACC \
               (PACC = ACC as of the start of the previous instruction)",
        run: || {
            let c = q(-0.75, 14);
            let acc0 = 900_001i64; // ACC entering the instruction before WRHX
            let delta = q(0.1, 10) << 13;
            // ldax; sof(+0.1); wrhx: PACC at WRHX = acc0.
            let got = eval1(
                &[
                    Instruction::Sof {
                        c: 1 << 14,
                        d: q(0.1, 10) as i16,
                    },
                    Instruction::Wrhx {
                        reg: reg::user(3),
                        c: c as i16,
                    },
                ],
                acc0,
            );
            let acc1 = sat(acc0 + delta);
            assert_eq!(got, sat(mulq(acc1, c, 14) + acc0));
        },
    },
    Case {
        variant: "WRLX",
        spec: "WRLX REG,C: REG = ACC, ACC = (PACC - ACC)*C + PACC",
        run: || {
            let c = q(0.5, 14);
            let acc0 = -640_001i64;
            let half = q(0.5, 14);
            let got = eval1(
                &[
                    Instruction::Sof {
                        c: half as i16,
                        d: 0,
                    },
                    Instruction::Wrlx {
                        reg: reg::user(4),
                        c: c as i16,
                    },
                ],
                acc0,
            );
            let acc1 = mulq(acc0, half, 14);
            assert_eq!(got, sat(mulq(acc0 - acc1, c, 14) + acc0));
        },
    },
    Case {
        variant: "MAXX",
        spec: "MAXX REG,C: ACC = MAX(|REG * C|, |ACC|); MAXX 0,0 is ABSA",
        run: || {
            let (r, c) = (-4_000_000i64, q(0.5, 14));
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCR),
                Instruction::Wrax {
                    reg: reg::user(5),
                    c: 0,
                },
                Instruction::ldax(reg::ADCL),
                Instruction::Maxx {
                    reg: reg::user(5),
                    c: c as i16,
                },
                OUT,
            ]);
            let acc = 1_500_000i64;
            let expected = sat(mulq(r, c, 14).abs().max(acc.abs()));
            assert_eq!(i64::from(fv1.process_raw(acc as i32, r as i32).0), expected);
            // ABSA pseudo-op.
            assert_eq!(eval1(&[Instruction::ABSA], -98_765), 98_765);
        },
    },
    Case {
        variant: "MULX",
        spec: "MULX REG: ACC = ACC * REG  (S.23 x S.23)",
        run: || {
            let r = q(0.5, 23);
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCR),
                Instruction::Wrax {
                    reg: reg::user(6),
                    c: 0,
                },
                Instruction::ldax(reg::ADCL),
                Instruction::Mulx { reg: reg::user(6) },
                OUT,
            ]);
            let acc = -333_333i64;
            let expected = sat(mulq(acc, r, 23));
            assert_eq!(i64::from(fv1.process_raw(acc as i32, r as i32).0), expected);
        },
    },
    Case {
        variant: "SKP",
        spec: "SKP CMASK,N: skip N instructions when every set condition \
               holds (NEG: ACC<0; GEZ: ACC>=0; ZRO: ACC==0; ZRC: sign of \
               ACC differs from PACC; RUN: not the first pass). An empty \
               mask holds vacuously: SKP 0,N is an unconditional jump",
        run: || {
            let probe = |cond: SkpCond, acc: i64| -> bool {
                // If the skip is taken the sentinel SOF is jumped over and
                // ACC survives; otherwise ACC becomes the sentinel.
                let sentinel = Instruction::Sof {
                    c: 0,
                    d: q(0.999, 10) as i16,
                };
                eval1(&[Instruction::Skp { cond, n: 1 }, sentinel], acc) == acc
            };
            assert!(probe(SkpCond::NEG, -1) && !probe(SkpCond::NEG, 0));
            assert!(probe(SkpCond::GEZ, 0) && !probe(SkpCond::GEZ, -1));
            assert!(probe(SkpCond::ZRO, 0) && !probe(SkpCond::ZRO, 1));
            // ZRC: PACC positive, ACC negative -> crossing.
            let crossed = eval1(
                &[
                    Instruction::Sof {
                        c: 0,
                        d: q(-0.5, 10) as i16,
                    },
                    Instruction::Skp {
                        cond: SkpCond::ZRC,
                        n: 1,
                    },
                    Instruction::CLR,
                ],
                100, // positive PACC source
            );
            assert_eq!(crossed, q(-0.5, 10) << 13, "sign change skips CLR");
            // Empty mask: unconditional jump (SpinASM's JMP).
            assert!(probe(SkpCond::NONE, 1), "SKP 0,N must always skip");
            assert!(probe(SkpCond::NONE, -1), "SKP 0,N must always skip");
            // RUN: false only on the very first pass after load.
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCL),
                Instruction::Skp {
                    cond: SkpCond::RUN,
                    n: 1,
                },
                Instruction::CLR,
                OUT,
            ]);
            assert_eq!(fv1.process_raw(55, 0).0, 0, "first pass: no skip");
            assert_eq!(fv1.process_raw(55, 0).0, 55, "later passes skip");
        },
    },
    Case {
        variant: "RDA",
        spec: "RDA ADDR,C: ACC = ACC + C * SRAM[ADDR]; delay addresses slide \
               one position per sample (decrementing base pointer)",
        run: || {
            let x = 4_100_001i64;
            let c = q(-1.25, 9);
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCL),
                Instruction::Wra { addr: 0, c: 0 },
                Instruction::Sof {
                    c: 0,
                    d: q(0.125, 10) as i16,
                },
                Instruction::Rda {
                    addr: 3,
                    c: c as i16,
                },
                OUT,
            ]);
            // Impulse, then zeros; the tap reads it exactly 3 samples later.
            let mut out = Vec::new();
            for n in 0..5 {
                out.push(i64::from(
                    fv1.process_raw(if n == 0 { x as i32 } else { 0 }, 0).0,
                ));
            }
            let base = q(0.125, 10) << 13;
            assert_eq!(out[2], base, "before the tap arrives");
            assert_eq!(out[3], sat(base + mulq(x, c, 9)), "tap at 3 samples");
            assert_eq!(out[4], base, "after it passes");
        },
    },
    Case {
        variant: "WRA",
        spec: "WRA ADDR,C: SRAM[ADDR] = ACC, then ACC = C * ACC",
        run: || {
            let acc = 2_222_223i64;
            let c = q(0.5, 9);
            let got = eval1(
                &[Instruction::Wra {
                    addr: 20,
                    c: c as i16,
                }],
                acc,
            );
            assert_eq!(got, sat(mulq(acc, c, 9)));
        },
    },
    Case {
        variant: "WRAP",
        spec: "WRAP ADDR,C: SRAM[ADDR] = ACC, then ACC = C*ACC + LR \
               (LR = value of the most recent delay read)",
        run: || {
            let x = 1_999_999i64;
            let c = q(0.25, 9);
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCL),
                Instruction::Wra { addr: 0, c: 0 },
                Instruction::Rda { addr: 2, c: 0 }, // LR = delay[2], ACC += 0
                Instruction::Sof {
                    c: 0,
                    d: q(0.5, 10) as i16,
                },
                Instruction::Wrap {
                    addr: 100,
                    c: c as i16,
                },
                OUT,
            ]);
            let mut out = Vec::new();
            for n in 0..4 {
                out.push(i64::from(
                    fv1.process_raw(if n == 0 { x as i32 } else { 0 }, 0).0,
                ));
            }
            let half = q(0.5, 10) << 13;
            assert_eq!(out[1], sat(mulq(half, c, 9)), "LR still zero");
            assert_eq!(out[2], sat(mulq(half, c, 9) + x), "LR = impulse");
        },
    },
    Case {
        variant: "RMPA",
        spec: "RMPA C: ACC = ACC + C * SRAM[ADDR_PTR], with the delay \
               address taken from ADDR_PTR bits 23..8",
        run: || {
            let v = 3_000_005i64;
            let c = q(1.5, 9);
            let target = 200i64;
            let mut fv1 = vm(&[
                Instruction::ldax(reg::ADCR),
                Instruction::Wra { addr: 200, c: 0 },
                Instruction::ldax(reg::ADCL),
                Instruction::Wrax {
                    reg: reg::ADDR_PTR,
                    c: 0,
                },
                Instruction::Rmpa { c: c as i16 },
                OUT,
            ]);
            let got = i64::from(fv1.process_raw((target << 8) as i32, v as i32).0);
            // WRAX ADDR_PTR,0 cleared ACC, so the result is the read alone.
            assert_eq!(got, sat(mulq(v, c, 9)));
        },
    },
    Case {
        variant: "WLDS",
        spec: "WLDS N,F,A: load SIN LFO N with frequency F (0..511) and \
               amplitude A (0..32767); f_Hz = F * SR / (2^17 * 2*pi)",
        run: || {
            let f = 300u16;
            let mut fv1 = vm(&[
                Instruction::Skp {
                    cond: SkpCond::RUN,
                    n: 1,
                },
                Instruction::Wlds {
                    lfo: true,
                    freq: f,
                    amp: 32767,
                },
                Instruction::ChoRdal {
                    lfo: LfoSel::Sin1,
                    flags: ChoFlags::SIN,
                },
                OUT,
            ]);
            let n = 1 << 15;
            let out: Vec<f32> = (0..n).map(|_| fv1.process(0.0, 0.0).0).collect();
            let measured = common::crossing_frequency(&out);
            let expected = f64::from(f) / f64::from(1u32 << 17) / core::f64::consts::TAU;
            assert!(
                (measured - expected).abs() / expected < 0.05,
                "SIN LFO frequency {measured} vs spec {expected}"
            );
        },
    },
    Case {
        variant: "WLDR",
        spec: "WLDR N,F,A: load RMP LFO N with signed frequency F and \
               amplitude A in {512,1024,2048,4096} samples; the tap slides \
               F/16384 samples per sample (F=+16384 is +1 octave)",
        run: || {
            // Deterministic phase check: phase counts down F/16 units per
            // sample in a 22-bit space (1024 units = 1 delay sample).
            let f = 1024i16;
            let steps = 100i64;
            let mut fv1 = vm(&[
                Instruction::Skp {
                    cond: SkpCond::RUN,
                    n: 1,
                },
                Instruction::Wldr {
                    lfo: false,
                    freq: f,
                    amp: RampAmp::Amp4096,
                },
                Instruction::ChoRdal {
                    lfo: LfoSel::Rmp0,
                    flags: ChoFlags::SIN,
                },
                OUT,
            ]);
            let mut last = 0i64;
            for _ in 0..steps {
                last = i64::from(fv1.process_raw(0, 0).0);
            }
            // After k full ticks the phase is (-(k)*(F/16)) mod 2^22; the
            // RDAL in pass k sees k-1 ticks.
            let expected = (-(steps - 1) * i64::from(f) / 16).rem_euclid(1 << 22);
            assert_eq!(last, expected, "ramp phase after {steps} samples");
        },
    },
    Case {
        variant: "JAM",
        spec: "JAM N: reset RMP LFO N to zero phase",
        run: || {
            let mut fv1 = vm(&[
                Instruction::Skp {
                    cond: SkpCond::RUN,
                    n: 1,
                },
                Instruction::Wldr {
                    lfo: true,
                    freq: 8192,
                    amp: RampAmp::Amp4096,
                },
                Instruction::Jam { lfo: true },
                Instruction::ChoRdal {
                    lfo: LfoSel::Rmp1,
                    flags: ChoFlags::SIN,
                },
                OUT,
            ]);
            for _ in 0..50 {
                assert_eq!(fv1.process_raw(0, 0).0, 0, "JAM pins phase at 0");
            }
        },
    },
    Case {
        variant: "CHO RDA",
        spec: "CHO RDA,N,FLAGS,ADDR: ACC = ACC + coeff * SRAM[ADDR+offset], \
               offset and interpolation coeff derived from LFO N; COMPC \
               gives 1-coeff (the classic two-instruction interpolated tap)",
        run: || {
            // Frozen SIN LFO (amp 0): offset 0, fraction 0, so the COMPC
            // read is (1.0 - 1 LSB) * SRAM[addr] and the plain read is 0.
            let x = 5_100_003i64;
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
                    addr: 4,
                },
                Instruction::ChoRda {
                    lfo: LfoSel::Sin0,
                    flags: ChoFlags::SIN,
                    addr: 5,
                },
                OUT,
            ]);
            let mut out = Vec::new();
            for n in 0..6 {
                out.push(i64::from(
                    fv1.process_raw(if n == 0 { x as i32 } else { 0 }, 0).0,
                ));
            }
            assert_eq!(
                out[4],
                mulq(x, MAX, 23),
                "(1-0)*x at the tap, 0*x beside it"
            );
            assert_eq!(out[3], 0);
            assert_eq!(out[5], 0);
        },
    },
    Case {
        variant: "CHO SOF",
        spec: "CHO SOF,N,FLAGS,D: ACC = coeff * ACC + D  (D: S.15); with \
               NA the coeff is the ramp crossfade envelope",
        run: || {
            // Ramp frozen at phase 0: crossfade = 0, so NA|COMPC gives
            // coeff = 1 - 0 and plain NA gives 0.
            let d = q(0.25, 15);
            let acc = 2_000_001i64;
            let got = eval1(
                &[
                    Instruction::Skp {
                        cond: SkpCond::RUN,
                        n: 1,
                    },
                    Instruction::Wldr {
                        lfo: false,
                        freq: 0,
                        amp: RampAmp::Amp4096,
                    },
                    Instruction::ChoSof {
                        lfo: LfoSel::Rmp0,
                        flags: ChoFlags::NA,
                        d: d as i16,
                    },
                ],
                acc,
            );
            assert_eq!(got, sat(mulq(acc, 0, 23) + (d << 8)), "xfade 0: ACC*0 + D");
            let got = eval1(
                &[
                    Instruction::Skp {
                        cond: SkpCond::RUN,
                        n: 1,
                    },
                    Instruction::Wldr {
                        lfo: false,
                        freq: 0,
                        amp: RampAmp::Amp4096,
                    },
                    Instruction::ChoSof {
                        lfo: LfoSel::Rmp0,
                        flags: ChoFlags::NA | ChoFlags::COMPC,
                        d: 0,
                    },
                ],
                acc,
            );
            assert_eq!(got, mulq(acc, MAX, 23), "xfade complement: ACC*(1-0)");
        },
    },
    Case {
        variant: "CHO RDAL",
        spec: "CHO RDAL,N: ACC = LFO N's current value (COS selects the \
               cosine output of a SIN LFO)",
        run: || {
            // Rate 0 freezes the SIN oscillator at its initial phase:
            // sin = 0, cos = -1.0. RDAL reads the range-scaled value.
            let amp = 12345u16;
            let mut fv1 = vm(&[
                Instruction::Skp {
                    cond: SkpCond::RUN,
                    n: 1,
                },
                Instruction::Wlds {
                    lfo: false,
                    freq: 0,
                    amp,
                },
                Instruction::ChoRdal {
                    lfo: LfoSel::Sin0,
                    flags: ChoFlags::COS,
                },
                OUT,
            ]);
            fv1.process_raw(0, 0);
            let got = i64::from(fv1.process_raw(0, 0).0);
            let expected = mulq(-ONE, i64::from(amp) << 8, 23);
            assert_eq!(got, expected, "cos(0) * amplitude");
            // And the sine output at phase 0 is exactly zero.
            let mut fv1 = vm(&[
                Instruction::Skp {
                    cond: SkpCond::RUN,
                    n: 1,
                },
                Instruction::Wlds {
                    lfo: false,
                    freq: 0,
                    amp,
                },
                Instruction::ChoRdal {
                    lfo: LfoSel::Sin0,
                    flags: ChoFlags::SIN,
                },
                OUT,
            ]);
            assert_eq!(fv1.process_raw(0, 0).0, 0);
        },
    },
    Case {
        variant: "RAW",
        spec: "words with undefined opcodes execute as no-ops",
        run: || {
            assert_eq!(eval1(&[Instruction::Raw(0xDEAD_BE1F)], 987_654), 987_654);
        },
    },
];

// Pseudo-ops are spec-defined spellings of real instructions; CLR and NOP
// are covered here explicitly, NOT/ABSA/LDAX inside their parents' cases.
#[test]
fn pseudo_ops_follow_spec() {
    // CLR: ACC = 0.
    assert_eq!(eval1(&[Instruction::CLR], -8_000_000), 0);
    // NOP: no state change.
    assert_eq!(eval1(&[Instruction::NOP], 246_810), 246_810);
}

#[test]
fn spec_matrix() {
    let mut covered: Vec<&str> = Vec::new();
    for case in CASES {
        if let Err(payload) = std::panic::catch_unwind(case.run) {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("(no message)");
            panic!(
                "spec violated for {}\n  spec: {}\n  failure: {detail}",
                case.variant, case.spec
            );
        }
        covered.push(case.variant);
    }
    // Completeness: every Instruction variant must have a spec case.
    for ins in all_variants() {
        let name = variant_name(&ins);
        assert!(
            covered.contains(&name),
            "instruction {name} has no spec-conformance case"
        );
    }
}

/// The register file itself is part of the spec: 64 addresses, with
/// ADC/POT inputs refreshed every sample and DAC outputs holding.
#[test]
fn register_file_follows_spec() {
    let mut fv1 = Fv1::new();
    let program =
        spinfv1::Program::from_instructions(&[Instruction::ldax(reg::POT2), OUT]).unwrap();
    fv1.load_program(&program);
    fv1.set_pot(2, 1.0);
    assert_eq!(i64::from(fv1.process_raw(0, 0).0), MAX, "POT full scale");
}
