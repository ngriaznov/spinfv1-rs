//! # spinfv1 — a Spin Semiconductor FV-1 emulator
//!
//! A bit-exact emulator of the FV-1 audio DSP: the full 21-opcode instruction
//! set, the S.23 saturating fixed-point ALU with its one-instruction PACC
//! pipeline, 32K-sample circulating delay RAM, and all four LFOs (two SIN,
//! two RMP) including the `CHO` interpolation machinery used by chorus,
//! flanger and pitch-shift programs.
//!
//! The core is dependency-free and processes one sample per call, which makes
//! it straightforward to embed in real-time hosts such as VCV Rack modules.
//!
//! ## Quick start
//! ```
//! use spinfv1::{Fv1, Instruction, Program, coeff, reg};
//!
//! // A 100-sample echo at half gain.
//! let program = Program::from_instructions(&[
//!     Instruction::ldax(reg::ADCL),
//!     Instruction::Wra { addr: 0, c: 0 },
//!     Instruction::Rda { addr: 100, c: coeff::s1_9(0.5) },
//!     Instruction::Wrax { reg: reg::DACL, c: 0 },
//! ])
//! .unwrap();
//!
//! let mut fv1 = Fv1::new();
//! fv1.load_program(&program);
//! for _ in 0..200 {
//!     let (_l, _r) = fv1.process(0.25, 0.25);
//! }
//! ```
//!
//! Programs can also be loaded from standard 512-byte EEPROM/bank images via
//! [`Program::from_bytes`].
//!
//! ## Fidelity notes
//!
//! * All arithmetic is integer fixed point matching the hardware formats;
//!   multiplications truncate and additions saturate at the 24-bit rails.
//! * LFO phase/scaling behavior follows the hardware-validated
//!   [`reference-impl`](https://example.invalid) model: SIN LFOs are
//!   magic-circle oscillators stepping `rate / 2^17` radians per sample; RMP
//!   LFOs count a 22-bit phase down by `rate / 16` per sample.
//! * `WLDS`/`WLDR` write the LFO rate/range registers without resetting the
//!   oscillator phase, so programs that run them every pass (instead of
//!   guarding with `SKP RUN`) still modulate correctly; `JAM` resets a ramp.
//! * Delay RAM keeps full 24-bit precision by default; the real chip stores
//!   14 bits. Call [`Fv1::set_delay_quantization`] to emulate that.
//! * `LOG`/`EXP` are computed in double precision and rounded to S.23, which
//!   is at least as accurate as the chip's internal approximation.

pub mod fixed;
mod instruction;
mod lfo;
mod program;
mod vm;

pub use fixed::coeff;
pub use instruction::{ChoFlags, Instruction, LfoSel, RampAmp, SkpCond, reg};
pub use program::{NOP_WORD, PROGRAM_LEN, Program, ProgramError};
pub use vm::{DELAY_LEN, Fv1, SAMPLE_RATE};
