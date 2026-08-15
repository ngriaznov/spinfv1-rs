//! FV-1 instruction words: decoding, encoding and pretty-printing.
//!
//! Every FV-1 instruction is a 32-bit word whose low 5 bits are the opcode.
//! Three opcodes are shared and disambiguated by the top two bits:
//! `0b10010` (WLDS/WLDR) and `0b10100` (CHO RDA / CHO SOF / CHO RDAL).
//!
//! [`Instruction::decode`] is *total*: every `u32` decodes to something.
//! Words with undefined opcodes (or undefined sub-type bits) decode to
//! [`Instruction::Raw`], which executes as a no-op and re-encodes verbatim.
//!
//! Coefficients are stored as raw two's-complement field values (see
//! [`crate::fixed`] for the formats); use [`crate::fixed::coeff`] to quantize
//! real numbers when building programs by hand.
//!
//! Encodings follow the Spin ASM users manual; the bit layouts were
//! cross-checked against independent public documentation of the format.

use core::fmt;

use crate::fixed::sx;

/// Skip-condition mask for [`Instruction::Skp`] (5 bits).
///
/// When several conditions are set, **all** of them must hold for the skip to
/// be taken.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SkpCond(pub u8);

impl SkpCond {
    /// No conditions: "all set conditions hold" is vacuously true, so
    /// `SKP 0,N` always skips (the chip's unconditional jump — SpinASM's
    /// `JMP`); `SKP 0,0` is the `NOP` pseudo-op.
    pub const NONE: Self = Self(0);
    /// Skip if ACC is negative.
    pub const NEG: Self = Self(0x01);
    /// Skip if ACC is greater than or equal to zero.
    pub const GEZ: Self = Self(0x02);
    /// Skip if ACC is exactly zero.
    pub const ZRO: Self = Self(0x04);
    /// Skip if the signs of ACC and PACC differ (zero crossing).
    pub const ZRC: Self = Self(0x08);
    /// Skip on every pass except the first after program load.
    pub const RUN: Self = Self(0x10);

    /// True if `other`'s bits are all set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True if no condition bits are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for SkpCond {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Flag bits for the `CHO` instructions (6 bits).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ChoFlags(pub u8);

impl ChoFlags {
    /// Use the sine output (default; bit value 0).
    pub const SIN: Self = Self(0x00);
    /// Use the cosine output instead of sine (SIN LFOs only).
    pub const COS: Self = Self(0x01);
    /// Latch the LFO value for subsequent CHO instructions.
    ///
    /// The emulator updates LFOs once per sample, between program passes, so
    /// the value is already stable throughout a pass and this flag is a no-op.
    pub const REG: Self = Self(0x02);
    /// Complement the interpolation coefficient (`1.0 - c`).
    pub const COMPC: Self = Self(0x04);
    /// Complement the address offset (negate for SIN, `range - x` for RMP).
    pub const COMPA: Self = Self(0x08);
    /// RMP only: read the second tap, half the ramp range away.
    pub const RPTR2: Self = Self(0x10);
    /// RMP only: no address — use the cross-fade envelope as coefficient.
    pub const NA: Self = Self(0x20);

    /// True if `other`'s bits are all set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for ChoFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// LFO selector used by `CHO` (2 bits).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LfoSel {
    /// Sine LFO 0.
    Sin0,
    /// Sine LFO 1.
    Sin1,
    /// Ramp LFO 0.
    Rmp0,
    /// Ramp LFO 1.
    Rmp1,
}

impl LfoSel {
    #[must_use]
    const fn from_bits(v: u32) -> Self {
        match v & 3 {
            0 => Self::Sin0,
            1 => Self::Sin1,
            2 => Self::Rmp0,
            _ => Self::Rmp1,
        }
    }

    #[must_use]
    const fn bits(self) -> u32 {
        self as u32
    }
}

/// Ramp LFO amplitude (window length in delay samples), 2-bit encoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RampAmp {
    /// 4096-sample window (code 0).
    Amp4096,
    /// 2048-sample window (code 1).
    Amp2048,
    /// 1024-sample window (code 2).
    Amp1024,
    /// 512-sample window (code 3).
    Amp512,
}

impl RampAmp {
    #[must_use]
    const fn from_bits(v: u32) -> Self {
        match v & 3 {
            0 => Self::Amp4096,
            1 => Self::Amp2048,
            2 => Self::Amp1024,
            _ => Self::Amp512,
        }
    }

    /// The 2-bit field value.
    #[must_use]
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// Window length in delay samples.
    #[must_use]
    pub const fn samples(self) -> i32 {
        4096 >> self.code()
    }
}

/// FV-1 register addresses (6-bit space used by RDAX/WRAX/... and MULX).
pub mod reg {
    /// SIN LFO 0 rate.
    pub const SIN0_RATE: u8 = 0x00;
    /// SIN LFO 0 range (amplitude).
    pub const SIN0_RANGE: u8 = 0x01;
    /// SIN LFO 1 rate.
    pub const SIN1_RATE: u8 = 0x02;
    /// SIN LFO 1 range (amplitude).
    pub const SIN1_RANGE: u8 = 0x03;
    /// Ramp LFO 0 rate.
    pub const RMP0_RATE: u8 = 0x04;
    /// Ramp LFO 0 range.
    pub const RMP0_RANGE: u8 = 0x05;
    /// Ramp LFO 1 rate.
    pub const RMP1_RATE: u8 = 0x06;
    /// Ramp LFO 1 range.
    pub const RMP1_RANGE: u8 = 0x07;
    /// Potentiometer 0 (read-only from the program's perspective).
    pub const POT0: u8 = 0x10;
    /// Potentiometer 1.
    pub const POT1: u8 = 0x11;
    /// Potentiometer 2.
    pub const POT2: u8 = 0x12;
    /// Left ADC input.
    pub const ADCL: u8 = 0x14;
    /// Right ADC input.
    pub const ADCR: u8 = 0x15;
    /// Left DAC output.
    pub const DACL: u8 = 0x16;
    /// Right DAC output.
    pub const DACR: u8 = 0x17;
    /// Indirect delay address pointer used by RMPA (address in bits 23..8).
    pub const ADDR_PTR: u8 = 0x18;
    /// General-purpose register `REG0..=REG31` (`n` in `0..=31`).
    #[must_use]
    pub const fn user(n: u8) -> u8 {
        0x20 + (n & 0x1F)
    }
}

/// A decoded FV-1 instruction.
///
/// Coefficient fields hold raw two's-complement values in the hardware's
/// fixed-point formats (documented per variant).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Instruction {
    /// `RDA addr, C` — `ACC += delay[addr] * C`; sets LR. `c` is S1.9.
    Rda {
        /// Delay address (15 bits used; the field is 16 bits wide).
        addr: u16,
        /// Coefficient, raw S1.9.
        c: i16,
    },
    /// `RMPA C` — `ACC += delay[ADDR_PTR] * C`; sets LR. `c` is S1.9.
    Rmpa {
        /// Coefficient, raw S1.9.
        c: i16,
    },
    /// `WRA addr, C` — `delay[addr] = ACC; ACC *= C`. `c` is S1.9.
    Wra {
        /// Delay address.
        addr: u16,
        /// Coefficient, raw S1.9.
        c: i16,
    },
    /// `WRAP addr, C` — `delay[addr] = ACC; ACC = ACC * C + LR`. `c` is S1.9.
    Wrap {
        /// Delay address.
        addr: u16,
        /// Coefficient, raw S1.9.
        c: i16,
    },
    /// `RDAX reg, C` — `ACC += REG[reg] * C`. `c` is S1.14.
    Rdax {
        /// Register address (6 bits).
        reg: u8,
        /// Coefficient, raw S1.14.
        c: i16,
    },
    /// `RDFX reg, C` — `ACC = (ACC - REG[reg]) * C + REG[reg]`. `c` is S1.14.
    ///
    /// With `c == 0` this is the `LDAX` pseudo-op.
    Rdfx {
        /// Register address.
        reg: u8,
        /// Coefficient, raw S1.14.
        c: i16,
    },
    /// `WRAX reg, C` — `REG[reg] = ACC; ACC *= C`. `c` is S1.14.
    Wrax {
        /// Register address.
        reg: u8,
        /// Coefficient, raw S1.14.
        c: i16,
    },
    /// `WRHX reg, C` — `REG[reg] = ACC; ACC = ACC * C + PACC`. `c` is S1.14.
    Wrhx {
        /// Register address.
        reg: u8,
        /// Coefficient, raw S1.14.
        c: i16,
    },
    /// `WRLX reg, C` — `REG[reg] = ACC; ACC = (PACC - ACC) * C + PACC`. `c` is S1.14.
    Wrlx {
        /// Register address.
        reg: u8,
        /// Coefficient, raw S1.14.
        c: i16,
    },
    /// `MAXX reg, C` — `ACC = max(|REG[reg] * C|, |ACC|)`. `c` is S1.14.
    ///
    /// With `reg == 0, c == 0` this is the `ABSA` pseudo-op.
    Maxx {
        /// Register address.
        reg: u8,
        /// Coefficient, raw S1.14.
        c: i16,
    },
    /// `MULX reg` — `ACC *= REG[reg]` (S.23 × S.23).
    Mulx {
        /// Register address.
        reg: u8,
    },
    /// `LOG C, D` — `ACC = C * log2(|ACC|) + D` in S4.19; `c` S1.14, `d` S4.6.
    ///
    /// The SPINAsm manual: D is an offset in the logarithmic domain,
    /// entered as Real(S4.6) in the range -16 to +15.999998.
    Log {
        /// Coefficient, raw S1.14.
        c: i16,
        /// Offset, raw S4.6 (11 bits).
        d: i16,
    },
    /// `EXP C, D` — `ACC = C * 2^ACC + D` (ACC read as S4.19); `c` S1.14, `d` S.10.
    Exp {
        /// Coefficient, raw S1.14.
        c: i16,
        /// Offset, raw S.10 (11 bits).
        d: i16,
    },
    /// `SOF C, D` — `ACC = ACC * C + D`; `c` S1.14, `d` S.10.
    Sof {
        /// Coefficient, raw S1.14.
        c: i16,
        /// Offset, raw S.10 (11 bits).
        d: i16,
    },
    /// `AND mask` — bitwise AND on the 24-bit ACC. `AND 0` is the `CLR` pseudo-op.
    And {
        /// 24-bit mask.
        mask: u32,
    },
    /// `OR mask` — bitwise OR on the 24-bit ACC.
    Or {
        /// 24-bit mask.
        mask: u32,
    },
    /// `XOR mask` — bitwise XOR on the 24-bit ACC. `XOR 0xFFFFFF` is `NOT`.
    Xor {
        /// 24-bit mask.
        mask: u32,
    },
    /// `SKP cond, N` — skip the next `n` instructions if all conditions hold.
    ///
    /// `SKP 0,0` is the `NOP` pseudo-op.
    Skp {
        /// Condition mask; empty means never skip.
        cond: SkpCond,
        /// Number of instructions to skip (6 bits).
        n: u8,
    },
    /// `WLDS lfo, F, A` — load SIN LFO `lfo` with 9-bit rate and 15-bit amplitude.
    Wlds {
        /// Which SIN LFO (false = SIN0, true = SIN1).
        lfo: bool,
        /// Rate, 0..=511.
        freq: u16,
        /// Amplitude in delay samples, 0..=32767.
        amp: u16,
    },
    /// `WLDR lfo, F, A` — load RMP LFO `lfo` with 16-bit signed rate and window.
    Wldr {
        /// Which RMP LFO (false = RMP0, true = RMP1).
        lfo: bool,
        /// Signed rate; pitch ratio is `1 + freq / 16384`.
        freq: i16,
        /// Ramp window length.
        amp: RampAmp,
    },
    /// `JAM lfo` — reset RMP LFO `lfo` phase to zero.
    Jam {
        /// Which RMP LFO (false = RMP0, true = RMP1).
        lfo: bool,
    },
    /// `CHO RDA, lfo, flags, addr` — LFO-modulated delay read with
    /// interpolation coefficient: `ACC += delay[addr + offset] * coeff`.
    ChoRda {
        /// LFO source.
        lfo: LfoSel,
        /// Modifier flags.
        flags: ChoFlags,
        /// Base delay address (16-bit field).
        addr: u16,
    },
    /// `CHO SOF, lfo, flags, D` — `ACC = ACC * coeff + D`; `d` is S.15.
    ChoSof {
        /// LFO source.
        lfo: LfoSel,
        /// Modifier flags.
        flags: ChoFlags,
        /// Offset, raw S.15 (16 bits).
        d: i16,
    },
    /// `CHO RDAL, lfo` — load the raw LFO value into ACC.
    ChoRdal {
        /// LFO source.
        lfo: LfoSel,
        /// Modifier flags (`COS` selects the cosine output of a SIN LFO).
        flags: ChoFlags,
    },
    /// Any word with an undefined opcode; executes as a no-op, re-encodes verbatim.
    Raw(u32),
}

impl Instruction {
    /// `NOP` pseudo-op (`SKP 0,0`).
    pub const NOP: Self = Self::Skp {
        cond: SkpCond::NONE,
        n: 0,
    };
    /// `CLR` pseudo-op (`AND 0`).
    pub const CLR: Self = Self::And { mask: 0 };
    /// `NOT` pseudo-op (`XOR 0xFFFFFF`).
    pub const NOT: Self = Self::Xor { mask: 0xFF_FFFF };
    /// `ABSA` pseudo-op (`MAXX 0,0`).
    pub const ABSA: Self = Self::Maxx { reg: 0, c: 0 };

    /// `LDAX reg` pseudo-op (`RDFX reg, 0`).
    #[must_use]
    pub const fn ldax(reg: u8) -> Self {
        Self::Rdfx { reg, c: 0 }
    }

    /// Decode a 32-bit program word. Total: never fails.
    #[must_use]
    pub fn decode(word: u32) -> Self {
        let op = word & 0x1F;
        let hi2 = word >> 30;
        match op {
            0x00 => Self::Rda {
                addr: ((word >> 5) & 0xFFFF) as u16,
                c: sx(word >> 21, 11) as i16,
            },
            0x01 => Self::Rmpa {
                c: sx(word >> 21, 11) as i16,
            },
            0x02 => Self::Wra {
                addr: ((word >> 5) & 0xFFFF) as u16,
                c: sx(word >> 21, 11) as i16,
            },
            0x03 => Self::Wrap {
                addr: ((word >> 5) & 0xFFFF) as u16,
                c: sx(word >> 21, 11) as i16,
            },
            0x04 => Self::Rdax {
                reg: reg_field(word),
                c: c16(word),
            },
            0x05 => Self::Rdfx {
                reg: reg_field(word),
                c: c16(word),
            },
            0x06 => Self::Wrax {
                reg: reg_field(word),
                c: c16(word),
            },
            0x07 => Self::Wrhx {
                reg: reg_field(word),
                c: c16(word),
            },
            0x08 => Self::Wrlx {
                reg: reg_field(word),
                c: c16(word),
            },
            0x09 => Self::Maxx {
                reg: reg_field(word),
                c: c16(word),
            },
            0x0A => Self::Mulx {
                reg: reg_field(word),
            },
            0x0B => Self::Log {
                c: c16(word),
                d: d11(word),
            },
            0x0C => Self::Exp {
                c: c16(word),
                d: d11(word),
            },
            0x0D => Self::Sof {
                c: c16(word),
                d: d11(word),
            },
            0x0E => Self::And {
                mask: (word >> 8) & 0xFF_FFFF,
            },
            0x0F => Self::Or {
                mask: (word >> 8) & 0xFF_FFFF,
            },
            0x10 => Self::Xor {
                mask: (word >> 8) & 0xFF_FFFF,
            },
            0x11 => Self::Skp {
                cond: SkpCond(((word >> 27) & 0x1F) as u8),
                n: ((word >> 21) & 0x3F) as u8,
            },
            0x12 => match hi2 {
                0b00 => Self::Wlds {
                    lfo: word & (1 << 29) != 0,
                    freq: ((word >> 20) & 0x1FF) as u16,
                    amp: ((word >> 5) & 0x7FFF) as u16,
                },
                0b01 => Self::Wldr {
                    lfo: word & (1 << 29) != 0,
                    freq: sx(word >> 13, 16) as i16,
                    amp: RampAmp::from_bits(word >> 5),
                },
                _ => Self::Raw(word),
            },
            0x13 => Self::Jam {
                lfo: word & (1 << 6) != 0,
            },
            0x14 => {
                let lfo = LfoSel::from_bits(word >> 21);
                let flags = ChoFlags(((word >> 24) & 0x3F) as u8);
                match hi2 {
                    0b00 => Self::ChoRda {
                        lfo,
                        flags,
                        addr: ((word >> 5) & 0xFFFF) as u16,
                    },
                    0b10 => Self::ChoSof {
                        lfo,
                        flags,
                        d: sx(word >> 5, 16) as i16,
                    },
                    0b11 => Self::ChoRdal { lfo, flags },
                    _ => Self::Raw(word),
                }
            }
            _ => Self::Raw(word),
        }
    }

    /// Encode back to a 32-bit program word.
    ///
    /// For every instruction, `decode(encode(i)) == i`; for every canonical
    /// word, `encode(decode(w)) == w`.
    #[must_use]
    pub fn encode(self) -> u32 {
        match self {
            Self::Rda { addr, c } => f11(c) << 21 | u32::from(addr) << 5,
            // SpinASM sets the register field of RMPA to ADDR_PTR (0x18).
            Self::Rmpa { c } => f11(c) << 21 | u32::from(reg::ADDR_PTR) << 5 | 0x01,
            Self::Wra { addr, c } => f11(c) << 21 | u32::from(addr) << 5 | 0x02,
            Self::Wrap { addr, c } => f11(c) << 21 | u32::from(addr) << 5 | 0x03,
            Self::Rdax { reg, c } => f16(c) << 16 | rf(reg) | 0x04,
            Self::Rdfx { reg, c } => f16(c) << 16 | rf(reg) | 0x05,
            Self::Wrax { reg, c } => f16(c) << 16 | rf(reg) | 0x06,
            Self::Wrhx { reg, c } => f16(c) << 16 | rf(reg) | 0x07,
            Self::Wrlx { reg, c } => f16(c) << 16 | rf(reg) | 0x08,
            Self::Maxx { reg, c } => f16(c) << 16 | rf(reg) | 0x09,
            Self::Mulx { reg } => rf(reg) | 0x0A,
            Self::Log { c, d } => f16(c) << 16 | f11(d) << 5 | 0x0B,
            Self::Exp { c, d } => f16(c) << 16 | f11(d) << 5 | 0x0C,
            Self::Sof { c, d } => f16(c) << 16 | f11(d) << 5 | 0x0D,
            Self::And { mask } => (mask & 0xFF_FFFF) << 8 | 0x0E,
            Self::Or { mask } => (mask & 0xFF_FFFF) << 8 | 0x0F,
            Self::Xor { mask } => (mask & 0xFF_FFFF) << 8 | 0x10,
            Self::Skp { cond, n } => {
                u32::from(cond.0 & 0x1F) << 27 | u32::from(n & 0x3F) << 21 | 0x11
            }
            Self::Wlds { lfo, freq, amp } => {
                u32::from(lfo) << 29
                    | u32::from(freq & 0x1FF) << 20
                    | u32::from(amp & 0x7FFF) << 5
                    | 0x12
            }
            Self::Wldr { lfo, freq, amp } => {
                0b01 << 30
                    | u32::from(lfo) << 29
                    | u32::from(freq as u16) << 13
                    | amp.code() << 5
                    | 0x12
            }
            Self::Jam { lfo } => 1 << 7 | u32::from(lfo) << 6 | 0x13,
            Self::ChoRda { lfo, flags, addr } => {
                cho(0b00, lfo, flags) | u32::from(addr) << 5 | 0x14
            }
            Self::ChoSof { lfo, flags, d } => {
                cho(0b10, lfo, flags) | u32::from(d as u16) << 5 | 0x14
            }
            Self::ChoRdal { lfo, flags } => cho(0b11, lfo, flags) | 0x14,
            Self::Raw(word) => word,
        }
    }
}

fn reg_field(word: u32) -> u8 {
    ((word >> 5) & 0x3F) as u8
}

fn c16(word: u32) -> i16 {
    sx(word >> 16, 16) as i16
}

fn d11(word: u32) -> i16 {
    sx(word >> 5, 11) as i16
}

/// Pack an 11-bit signed field.
fn f11(v: i16) -> u32 {
    (v as u32) & 0x7FF
}

/// Pack a 16-bit signed field.
fn f16(v: i16) -> u32 {
    u32::from(v as u16)
}

/// Pack a register address field.
fn rf(reg: u8) -> u32 {
    u32::from(reg & 0x3F) << 5
}

/// Pack the CHO type/flag/LFO-select fields.
fn cho(ty: u32, lfo: LfoSel, flags: ChoFlags) -> u32 {
    ty << 30 | u32::from(flags.0 & 0x3F) << 24 | lfo.bits() << 21
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn c14(v: i16) -> f64 {
            f64::from(v) / 16384.0
        }
        fn c9(v: i16) -> f64 {
            f64::from(v) / 512.0
        }
        match *self {
            Self::CLR => write!(f, "CLR"),
            Self::NOT => write!(f, "NOT"),
            Self::ABSA => write!(f, "ABSA"),
            Self::NOP => write!(f, "NOP"),
            Self::Rdfx { reg, c: 0 } => write!(f, "LDAX {reg}"),
            Self::Rda { addr, c } => write!(f, "RDA {addr}, {}", c9(c)),
            Self::Rmpa { c } => write!(f, "RMPA {}", c9(c)),
            Self::Wra { addr, c } => write!(f, "WRA {addr}, {}", c9(c)),
            Self::Wrap { addr, c } => write!(f, "WRAP {addr}, {}", c9(c)),
            Self::Rdax { reg, c } => write!(f, "RDAX {reg}, {}", c14(c)),
            Self::Rdfx { reg, c } => write!(f, "RDFX {reg}, {}", c14(c)),
            Self::Wrax { reg, c } => write!(f, "WRAX {reg}, {}", c14(c)),
            Self::Wrhx { reg, c } => write!(f, "WRHX {reg}, {}", c14(c)),
            Self::Wrlx { reg, c } => write!(f, "WRLX {reg}, {}", c14(c)),
            Self::Maxx { reg, c } => write!(f, "MAXX {reg}, {}", c14(c)),
            Self::Mulx { reg } => write!(f, "MULX {reg}"),
            Self::Log { c, d } => write!(f, "LOG {}, {}", c14(c), f64::from(d) / 64.0),
            Self::Exp { c, d } => write!(f, "EXP {}, {}", c14(c), f64::from(d) / 1024.0),
            Self::Sof { c, d } => write!(f, "SOF {}, {}", c14(c), f64::from(d) / 1024.0),
            Self::And { mask } => write!(f, "AND ${mask:06X}"),
            Self::Or { mask } => write!(f, "OR ${mask:06X}"),
            Self::Xor { mask } => write!(f, "XOR ${mask:06X}"),
            Self::Skp { cond, n } => write!(f, "SKP ${:02X}, {n}", cond.0),
            Self::Wlds { lfo, freq, amp } => {
                write!(f, "WLDS SIN{}, {freq}, {amp}", u8::from(lfo))
            }
            Self::Wldr { lfo, freq, amp } => {
                write!(f, "WLDR RMP{}, {freq}, {}", u8::from(lfo), amp.samples())
            }
            Self::Jam { lfo } => write!(f, "JAM RMP{}", u8::from(lfo)),
            Self::ChoRda { lfo, flags, addr } => {
                write!(f, "CHO RDA, {lfo:?}, ${:02X}, {addr}", flags.0)
            }
            Self::ChoSof { lfo, flags, d } => {
                write!(
                    f,
                    "CHO SOF, {lfo:?}, ${:02X}, {}",
                    flags.0,
                    f64::from(d) / 32768.0
                )
            }
            Self::ChoRdal { lfo, flags } => write!(f, "CHO RDAL, {lfo:?}, ${:02X}", flags.0),
            Self::Raw(word) => write!(f, "RAW ${word:08X}"),
        }
    }
}
