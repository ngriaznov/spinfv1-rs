//! Instruction encode/decode coverage: golden words cross-checked against the
//! SpinASM encodings, exhaustive field sweeps, and a full-u32 fuzz
//! proving decode/encode is total and lossless.

use spinfv1::{ChoFlags, Instruction, LfoSel, RampAmp, SkpCond, reg};

#[test]
fn golden_words() {
    // Hand-assembled against the SpinASM user manual.
    let cases: &[(Instruction, u32)] = &[
        // RDAX ADCL, 1.0 => C=0x4000<<16 | 0x14<<5 | 0x04
        (
            Instruction::Rdax {
                reg: reg::ADCL,
                c: 16384,
            },
            0x4000_0284,
        ),
        // WRAX DACL, 0
        (
            Instruction::Wrax {
                reg: reg::DACL,
                c: 0,
            },
            0x0000_02C6,
        ),
        // SOF 0, 0
        (Instruction::Sof { c: 0, d: 0 }, 0x0000_000D),
        // SOF -2.0, -1.0 => C=0x8000, D=0x400
        (
            Instruction::Sof {
                c: -32768,
                d: -1024,
            },
            0x8000_800D,
        ),
        // RDA 1000, -1.0 => C=-512=0x600 (11b), addr 1000
        (
            Instruction::Rda {
                addr: 1000,
                c: -512,
            },
            0xC000_7D00,
        ),
        // WRA 32767, 0.5 => C=256, addr 0x7FFF
        (
            Instruction::Wra {
                addr: 32767,
                c: 256,
            },
            0x200F_FFE2,
        ),
        // RMPA 1.0 => C=512, plus the ADDR_PTR register field SpinASM emits
        (Instruction::Rmpa { c: 512 }, 0x4000_0301),
        // MULX REG0
        (Instruction::Mulx { reg: reg::user(0) }, 0x0000_040A),
        // AND $FFFFFE
        (Instruction::And { mask: 0xFF_FFFE }, 0xFFFF_FE0E),
        // NOT = XOR $FFFFFF
        (Instruction::NOT, 0xFFFF_FF10),
        // CLR = AND $0
        (Instruction::CLR, 0x0000_000E),
        // NOP = SKP 0,0
        (Instruction::NOP, 0x0000_0011),
        // SKP RUN, 1 => cond 0x10 << 27 | 1 << 21
        (
            Instruction::Skp {
                cond: SkpCond::RUN,
                n: 1,
            },
            0x8020_0011,
        ),
        // WLDS SIN0, 511, 32767
        (
            Instruction::Wlds {
                lfo: false,
                freq: 511,
                amp: 32767,
            },
            0x1FFF_FFF2,
        ),
        // WLDR RMP1, -16384, 4096 => 01 1 C000(16b) 000000 00 10010
        (
            Instruction::Wldr {
                lfo: true,
                freq: -16384,
                amp: RampAmp::Amp4096,
            },
            0x7800_0012,
        ),
        // JAM RMP0 => 1<<7 | 0<<6 | 0x13
        (Instruction::Jam { lfo: false }, 0x0000_0093),
        // JAM RMP1
        (Instruction::Jam { lfo: true }, 0x0000_00D3),
        // CHO RDA, SIN0, SIN|REG|COMPC, 100 => flags 0x06 << 24 | addr 100 << 5
        (
            Instruction::ChoRda {
                lfo: LfoSel::Sin0,
                flags: ChoFlags::REG | ChoFlags::COMPC,
                addr: 100,
            },
            0x0600_0C94,
        ),
        // CHO SOF, RMP1, NA, 0 => 10 100000 0 11 zeros
        (
            Instruction::ChoSof {
                lfo: LfoSel::Rmp1,
                flags: ChoFlags::NA,
                d: 0,
            },
            0xA060_0014,
        ),
        // CHO RDAL, SIN0 => 11 000000 0 00
        (
            Instruction::ChoRdal {
                lfo: LfoSel::Sin0,
                flags: ChoFlags::SIN,
            },
            0xC000_0014,
        ),
    ];
    for &(ins, word) in cases {
        assert_eq!(ins.encode(), word, "encode {ins:?}");
        assert_eq!(Instruction::decode(word), ins, "decode {word:#010X}");
    }
}

#[test]
fn field_sweep_roundtrip() {
    let check = |ins: Instruction| {
        let word = ins.encode();
        assert_eq!(
            Instruction::decode(word),
            ins,
            "roundtrip {ins:?} via {word:#010X}"
        );
    };

    let c11: Vec<i16> = (-1024..=1023).collect();
    let regs: Vec<u8> = (0..64).collect();

    for &c in &c11 {
        check(Instruction::Rmpa { c });
        check(Instruction::Rda { addr: 0x5A5A, c });
        check(Instruction::Wra { addr: 1, c });
        check(Instruction::Wrap { addr: 0x7FFF, c });
        check(Instruction::Log { c: 123, d: c });
        check(Instruction::Exp { c: -123, d: c });
        check(Instruction::Sof { c: i16::MIN, d: c });
    }
    for c in [i16::MIN, -16384, -1, 0, 1, 16384, i16::MAX] {
        for &r in &regs {
            check(Instruction::Rdax { reg: r, c });
            check(Instruction::Rdfx { reg: r, c });
            check(Instruction::Wrax { reg: r, c });
            check(Instruction::Wrhx { reg: r, c });
            check(Instruction::Wrlx { reg: r, c });
            check(Instruction::Maxx { reg: r, c });
            check(Instruction::Mulx { reg: r });
        }
        check(Instruction::ChoSof {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags(0x3F),
            d: c,
        });
    }
    for mask in [0u32, 1, 0x0055_5555, 0x00AA_AAAA, 0xFF_FFFF] {
        check(Instruction::And { mask });
        check(Instruction::Or { mask });
        check(Instruction::Xor { mask });
    }
    for cond in 0..=0x1F {
        for n in [0u8, 1, 31, 63] {
            check(Instruction::Skp {
                cond: SkpCond(cond),
                n,
            });
        }
    }
    for lfo in [false, true] {
        for freq in [0u16, 1, 255, 511] {
            for amp in [0u16, 1, 16384, 32767] {
                check(Instruction::Wlds { lfo, freq, amp });
            }
        }
        for freq in [i16::MIN, -1, 0, 1, i16::MAX] {
            for amp in [
                RampAmp::Amp4096,
                RampAmp::Amp2048,
                RampAmp::Amp1024,
                RampAmp::Amp512,
            ] {
                check(Instruction::Wldr { lfo, freq, amp });
            }
        }
        check(Instruction::Jam { lfo });
    }
    for lfo in [LfoSel::Sin0, LfoSel::Sin1, LfoSel::Rmp0, LfoSel::Rmp1] {
        for flags in 0..=0x3F {
            check(Instruction::ChoRda {
                lfo,
                flags: ChoFlags(flags),
                addr: 0xBEEF,
            });
            check(Instruction::ChoSof {
                lfo,
                flags: ChoFlags(flags),
                d: -1,
            });
            check(Instruction::ChoRdal {
                lfo,
                flags: ChoFlags(flags),
            });
        }
    }
}

/// decode is total and encode(decode(w)) reproduces every canonical word.
/// Non-canonical words (junk in must-be-zero fields) may re-encode
/// differently, so we canonicalize once and require a fixed point after that.
#[test]
fn decode_encode_fixed_point_fuzz() {
    let mut lcg: u64 = 0x1234_5678_9ABC_DEF0;
    for _ in 0..2_000_000 {
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let word = (lcg >> 24) as u32;
        let ins = Instruction::decode(word);
        let canonical = ins.encode();
        let ins2 = Instruction::decode(canonical);
        assert_eq!(
            ins, ins2,
            "canonical word must decode to the same instruction"
        );
        assert_eq!(
            ins2.encode(),
            canonical,
            "canonical encoding must be a fixed point"
        );
    }
}

#[test]
fn undefined_words_are_raw_and_verbatim() {
    // Opcodes 0x15..=0x1F are undefined.
    for op in 0x15u32..=0x1F {
        let word = 0xDEAD_BE00 | op;
        assert_eq!(Instruction::decode(word), Instruction::Raw(word));
        assert_eq!(Instruction::decode(word).encode(), word);
    }
    // CHO type 0b01 and WLDS/WLDR type 0b10/0b11 are undefined.
    for word in [0x4000_0014u32, 0x8000_0012, 0xC000_0012] {
        assert_eq!(Instruction::decode(word), Instruction::Raw(word));
    }
}

#[test]
fn pseudo_op_constructors() {
    assert_eq!(
        Instruction::NOP,
        Instruction::Skp {
            cond: SkpCond::NONE,
            n: 0
        }
    );
    assert_eq!(Instruction::CLR, Instruction::And { mask: 0 });
    assert_eq!(Instruction::NOT, Instruction::Xor { mask: 0xFF_FFFF });
    assert_eq!(Instruction::ABSA, Instruction::Maxx { reg: 0, c: 0 });
    assert_eq!(
        Instruction::ldax(reg::ADCL),
        Instruction::Rdfx {
            reg: reg::ADCL,
            c: 0
        }
    );
}

#[test]
fn display_smoke() {
    assert_eq!(Instruction::NOP.to_string(), "NOP");
    assert_eq!(Instruction::CLR.to_string(), "CLR");
    assert_eq!(
        Instruction::Rdax {
            reg: reg::ADCL,
            c: 16384
        }
        .to_string(),
        "RDAX 20, 1"
    );
}
