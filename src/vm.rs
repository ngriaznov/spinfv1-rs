//! The FV-1 virtual machine: registers, delay RAM, LFOs and the
//! per-sample execution loop.

use crate::fixed::{
    ACC_MAX, ACC_MIN, ONE_S23, f32_to_s23, mul_s1_9, mul_s1_14, mul_s23, s23_to_f32, sat24, sx24,
};
use crate::instruction::{ChoFlags, Instruction, LfoSel, SkpCond, reg};
use crate::lfo::{ChoValue, RampLfo, SinLfo};
use crate::program::{PROGRAM_LEN, Program};

/// Delay RAM size in samples (~1 second at 32,768 Hz).
pub const DELAY_LEN: usize = 32768;

const DELAY_MASK: i32 = DELAY_LEN as i32 - 1;

/// The chip's native sample rate in Hz. The emulator itself is rate-agnostic
/// (it processes one sample per call); run it at this rate for authentic
/// delay times and LFO frequencies.
pub const SAMPLE_RATE: f64 = 32768.0;

/// A complete FV-1: program memory, 64 registers, 32K-sample delay RAM,
/// two SIN and two RMP LFOs, and the S.23 saturating ALU.
///
/// # Examples
/// ```
/// use spinfv1::{Fv1, Instruction, Program, reg};
///
/// // Pass-through: ACC = ADCL, DACL = ACC.
/// let program = Program::from_instructions(&[
///     Instruction::ldax(reg::ADCL),
///     Instruction::Wrax { reg: reg::DACL, c: 0 },
/// ])
/// .unwrap();
///
/// let mut fv1 = Fv1::new();
/// fv1.load_program(&program);
/// let (l, _r) = fv1.process(0.5, 0.0);
/// assert!((l - 0.5).abs() < 1e-6);
/// ```
pub struct Fv1 {
    instructions: [Instruction; PROGRAM_LEN],
    delay: Box<[i32]>,
    /// Base pointer into delay RAM; decremented every sample.
    cursor: i32,
    regs: [i32; 64],
    acc: i32,
    /// ACC as it was *before the previous* executed instruction — the
    /// hardware pipelines the accumulator by one instruction.
    pacc: i32,
    /// ACC before the current instruction (becomes `pacc` next instruction).
    prev_acc: i32,
    /// Last value read from delay RAM (used by WRAP).
    lr: i32,
    first_run: bool,
    sin_lfo: [SinLfo; 2],
    rmp_lfo: [RampLfo; 2],
    pots: [i32; 3],
    quantize_delay_14bit: bool,
}

impl Fv1 {
    /// A fresh FV-1 with an all-NOP program (outputs silence).
    #[must_use]
    pub fn new() -> Self {
        Self {
            instructions: Program::default().instructions(),
            delay: vec![0; DELAY_LEN].into_boxed_slice(),
            cursor: 0,
            regs: [0; 64],
            acc: 0,
            pacc: 0,
            prev_acc: 0,
            lr: 0,
            first_run: true,
            sin_lfo: [SinLfo::new(), SinLfo::new()],
            rmp_lfo: [RampLfo::new(), RampLfo::new()],
            pots: [0; 3],
            quantize_delay_14bit: false,
        }
    }

    /// Load a program and reset all DSP state (registers, delay RAM, LFOs,
    /// accumulator), as if the chip had just switched programs.
    pub fn load_program(&mut self, program: &Program) {
        self.instructions = program.instructions();
        self.reset();
    }

    /// Reset all DSP state without changing the loaded program.
    pub fn reset(&mut self) {
        self.delay.fill(0);
        self.cursor = 0;
        self.regs = [0; 64];
        self.acc = 0;
        self.pacc = 0;
        self.prev_acc = 0;
        self.lr = 0;
        self.first_run = true;
        self.sin_lfo = [SinLfo::new(), SinLfo::new()];
        self.rmp_lfo = [RampLfo::new(), RampLfo::new()];
    }

    /// Set potentiometer `idx` (0..=2) to `value` in `[0.0, 1.0]`.
    pub fn set_pot(&mut self, idx: usize, value: f32) {
        if let Some(pot) = self.pots.get_mut(idx) {
            *pot = f32_to_s23(value.clamp(0.0, 1.0));
        }
    }

    /// The real chip stores samples in 14-bit delay RAM; the emulator keeps
    /// full 24-bit precision by default. Enable this to truncate delay writes
    /// to 14 significant bits for a hardware-flavored noise floor.
    pub fn set_delay_quantization(&mut self, enabled: bool) {
        self.quantize_delay_14bit = enabled;
    }

    /// Current accumulator, raw S.23.
    #[must_use]
    pub fn acc(&self) -> i32 {
        self.acc
    }

    /// Raw S.23 value of register `r` (6-bit address).
    #[must_use]
    pub fn register(&self, r: u8) -> i32 {
        self.regs[usize::from(r & 0x3F)]
    }

    /// Read-only view of delay RAM (absolute addresses, raw S.23).
    #[must_use]
    pub fn delay_ram(&self) -> &[i32] {
        &self.delay
    }

    /// Process one stereo sample (floats in `[-1.0, 1.0)`).
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let (l, r) = self.process_raw(f32_to_s23(in_l), f32_to_s23(in_r));
        (s23_to_f32(l), s23_to_f32(r))
    }

    /// Process buffers in one call; all four slices must be the same length.
    ///
    /// # Panics
    /// Panics if slice lengths differ.
    pub fn process_block(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        assert!(
            in_l.len() == in_r.len() && in_l.len() == out_l.len() && in_l.len() == out_r.len(),
            "process_block: all slices must have equal length"
        );
        for i in 0..in_l.len() {
            (out_l[i], out_r[i]) = self.process(in_l[i], in_r[i]);
        }
    }

    /// Process one stereo sample in raw S.23.
    #[allow(clippy::too_many_lines, reason = "one match arm per FV-1 instruction")]
    pub fn process_raw(&mut self, in_l: i32, in_r: i32) -> (i32, i32) {
        self.regs[usize::from(reg::ADCL)] = sat24(i64::from(in_l));
        self.regs[usize::from(reg::ADCR)] = sat24(i64::from(in_r));
        self.regs[usize::from(reg::POT0)] = self.pots[0];
        self.regs[usize::from(reg::POT1)] = self.pots[1];
        self.regs[usize::from(reg::POT2)] = self.pots[2];

        let mut ic = 0usize;
        while ic < PROGRAM_LEN {
            let ins = self.instructions[ic];
            ic += 1;
            let acc = i64::from(self.acc);
            match ins {
                Instruction::Rda { addr, c } => {
                    let s = self.delay_read(i32::from(addr));
                    self.acc = sat24(mul_s1_9(i64::from(s), i64::from(c)) + acc);
                }
                Instruction::Rmpa { c } => {
                    let ptr = (self.regs[usize::from(reg::ADDR_PTR)] >> 8) & DELAY_MASK;
                    let s = self.delay_read(ptr);
                    self.acc = sat24(mul_s1_9(i64::from(s), i64::from(c)) + acc);
                }
                Instruction::Wra { addr, c } => {
                    self.delay_write(i32::from(addr), self.acc);
                    self.acc = sat24(mul_s1_9(acc, i64::from(c)));
                }
                Instruction::Wrap { addr, c } => {
                    self.delay_write(i32::from(addr), self.acc);
                    self.acc = sat24(mul_s1_9(acc, i64::from(c)) + i64::from(self.lr));
                }
                Instruction::Rdax { reg, c } => {
                    let r = i64::from(self.regs[usize::from(reg & 0x3F)]);
                    self.acc = sat24(mul_s1_14(r, i64::from(c)) + acc);
                }
                Instruction::Rdfx { reg, c } => {
                    let r = i64::from(self.regs[usize::from(reg & 0x3F)]);
                    self.acc = sat24(mul_s1_14(acc - r, i64::from(c)) + r);
                }
                Instruction::Wrax { reg, c } => {
                    self.regs[usize::from(reg & 0x3F)] = self.acc;
                    self.acc = sat24(mul_s1_14(acc, i64::from(c)));
                }
                Instruction::Wrhx { reg, c } => {
                    self.regs[usize::from(reg & 0x3F)] = self.acc;
                    self.acc = sat24(mul_s1_14(acc, i64::from(c)) + i64::from(self.pacc));
                }
                Instruction::Wrlx { reg, c } => {
                    self.regs[usize::from(reg & 0x3F)] = self.acc;
                    let pacc = i64::from(self.pacc);
                    self.acc = sat24(mul_s1_14(pacc - acc, i64::from(c)) + pacc);
                }
                Instruction::Maxx { reg, c } => {
                    let r = i64::from(self.regs[usize::from(reg & 0x3F)]);
                    let rxc = mul_s1_14(r, i64::from(c)).abs();
                    self.acc = sat24(rxc.max(acc.abs()));
                }
                Instruction::Mulx { reg } => {
                    let r = i64::from(self.regs[usize::from(reg & 0x3F)]);
                    self.acc = sat24(mul_s23(acc, r));
                }
                Instruction::Log { c, d } => {
                    // ACC is treated as S4.19 in the log domain.
                    let l = if self.acc == 0 {
                        i64::from(ACC_MIN)
                    } else {
                        let mag = f64::from(self.acc.unsigned_abs()) / f64::from(ONE_S23);
                        sat24((mag.log2() * f64::from(1 << 19)).round() as i64).into()
                    };
                    self.acc = sat24(mul_s1_14(l, i64::from(c)) + (i64::from(d) << 13));
                }
                Instruction::Exp { c, d } => {
                    // ACC (S4.19) exponentiated; 2^x >= 1.0 saturates.
                    let e = if self.acc >= 0 {
                        i64::from(ACC_MAX)
                    } else {
                        let x = f64::from(self.acc) / f64::from(1 << 19);
                        (x.exp2() * f64::from(ONE_S23)).round() as i64
                    };
                    self.acc = sat24(mul_s1_14(e, i64::from(c)) + (i64::from(d) << 13));
                }
                Instruction::Sof { c, d } => {
                    self.acc = sat24(mul_s1_14(acc, i64::from(c)) + (i64::from(d) << 13));
                }
                Instruction::And { mask } => {
                    self.acc = sx24(self.acc & mask as i32);
                }
                Instruction::Or { mask } => {
                    self.acc = sx24((self.acc & 0xFF_FFFF) | mask as i32);
                }
                Instruction::Xor { mask } => {
                    self.acc = sx24((self.acc & 0xFF_FFFF) ^ mask as i32);
                }
                Instruction::Skp { cond, n } => {
                    if !cond.is_empty() && self.skp_taken(cond) {
                        ic += usize::from(n);
                    }
                }
                Instruction::Wlds { lfo, freq, amp } => {
                    let base = if lfo { reg::SIN1_RATE } else { reg::SIN0_RATE };
                    self.regs[usize::from(base)] = i32::from(freq) << 14;
                    self.regs[usize::from(base) + 1] = i32::from(amp) << 8;
                }
                Instruction::Wldr { lfo, freq, amp } => {
                    let base = if lfo { reg::RMP1_RATE } else { reg::RMP0_RATE };
                    self.regs[usize::from(base)] = i32::from(freq) << 8;
                    self.regs[usize::from(base) + 1] = (amp.code() as i32) << 21;
                }
                Instruction::Jam { lfo } => {
                    self.rmp_lfo[usize::from(lfo)].jam();
                }
                Instruction::ChoRda { lfo, flags, addr } => {
                    let ChoValue { offset, coeff } = self.cho_value(lfo, flags);
                    let s = self.delay_read(i32::from(addr).wrapping_add(offset));
                    self.acc = sat24(mul_s23(i64::from(s), i64::from(coeff)) + acc);
                }
                Instruction::ChoSof { lfo, flags, d } => {
                    let ChoValue { coeff, .. } = self.cho_value(lfo, flags);
                    self.acc = sat24(mul_s23(acc, i64::from(coeff)) + (i64::from(d) << 8));
                }
                Instruction::ChoRdal { lfo, flags } => {
                    self.acc = match lfo {
                        LfoSel::Sin0 => self.sin_lfo[0]
                            .value(self.lfo_reg(reg::SIN0_RANGE), flags.contains(ChoFlags::COS)),
                        LfoSel::Sin1 => self.sin_lfo[1]
                            .value(self.lfo_reg(reg::SIN1_RANGE), flags.contains(ChoFlags::COS)),
                        LfoSel::Rmp0 => self.rmp_lfo[0].value(),
                        LfoSel::Rmp1 => self.rmp_lfo[1].value(),
                    };
                }
                Instruction::Raw(_) => {}
            }
            // The accumulator pipeline: PACC lags ACC by one instruction, so
            // during instruction N it holds ACC as of the start of N-1.
            self.pacc = self.prev_acc;
            self.prev_acc = self.acc;
        }

        self.first_run = false;
        self.cursor = (self.cursor - 1) & DELAY_MASK;
        self.sin_lfo[0].tick(self.lfo_reg(reg::SIN0_RATE));
        self.sin_lfo[1].tick(self.lfo_reg(reg::SIN1_RATE));
        self.rmp_lfo[0].tick(self.lfo_reg(reg::RMP0_RATE), self.lfo_reg(reg::RMP0_RANGE));
        self.rmp_lfo[1].tick(self.lfo_reg(reg::RMP1_RATE), self.lfo_reg(reg::RMP1_RANGE));

        (
            self.regs[usize::from(reg::DACL)],
            self.regs[usize::from(reg::DACR)],
        )
    }

    fn skp_taken(&self, cond: SkpCond) -> bool {
        let mut take = true;
        if cond.contains(SkpCond::NEG) {
            take &= self.acc < 0;
        }
        if cond.contains(SkpCond::GEZ) {
            take &= self.acc >= 0;
        }
        if cond.contains(SkpCond::ZRO) {
            take &= self.acc == 0;
        }
        if cond.contains(SkpCond::ZRC) {
            take &= (self.acc >= 0) != (self.pacc >= 0);
        }
        if cond.contains(SkpCond::RUN) {
            take &= !self.first_run;
        }
        take
    }

    fn lfo_reg(&self, r: u8) -> i32 {
        self.regs[usize::from(r)]
    }

    fn cho_value(&self, lfo: LfoSel, flags: ChoFlags) -> ChoValue {
        match lfo {
            LfoSel::Sin0 => self.sin_lfo[0].cho(self.lfo_reg(reg::SIN0_RANGE), flags),
            LfoSel::Sin1 => self.sin_lfo[1].cho(self.lfo_reg(reg::SIN1_RANGE), flags),
            LfoSel::Rmp0 => self.rmp_lfo[0].cho(self.lfo_reg(reg::RMP0_RANGE), flags),
            LfoSel::Rmp1 => self.rmp_lfo[1].cho(self.lfo_reg(reg::RMP1_RANGE), flags),
        }
    }

    fn delay_read(&mut self, offset: i32) -> i32 {
        let idx = (self.cursor.wrapping_add(offset) & DELAY_MASK) as usize;
        let v = self.delay[idx];
        self.lr = v;
        v
    }

    fn delay_write(&mut self, offset: i32, value: i32) {
        let idx = (self.cursor.wrapping_add(offset) & DELAY_MASK) as usize;
        self.delay[idx] = if self.quantize_delay_14bit {
            (value >> 10) << 10
        } else {
            value
        };
    }
}

impl Default for Fv1 {
    fn default() -> Self {
        Self::new()
    }
}
