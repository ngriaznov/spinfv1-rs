//! The FV-1's two SIN and two RMP low-frequency oscillators.
//!
//! Both LFO types read their rate/range from the register file every sample,
//! so programs can modulate them live (e.g. `WRAX SIN0_RATE`). They advance
//! once per sample, after the program pass — their value is constant within a
//! pass, which is why the `REG` latch flag is a no-op here.
//!
//! The oscillator models implement the hardware behavior described in Spin's
//! datasheet and application notes (phase step formulas, amplitude scaling,
//! interpolation fraction widths), cross-checked against behavior others have
//! validated on real chips. The implementation itself is original.

use crate::fixed::{ACC_MAX, mul_s23};
use crate::instruction::ChoFlags;

/// What a `CHO` instruction gets from an LFO: a delay-address offset and an
/// S.23 interpolation coefficient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChoValue {
    /// Offset added to the delay address (signed, in samples).
    pub offset: i32,
    /// Interpolation coefficient, raw S.23 in `[0, 1)`.
    pub coeff: i32,
}

/// Sine LFO: a "magic circle" coupled oscillator whose state stays on the
/// unit circle. Frequency ≈ `rate * SR / (2π * 2^17)` Hz.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SinLfo {
    sin: i32,
    cos: i32,
}

impl SinLfo {
    pub fn new() -> Self {
        Self {
            sin: 0,
            cos: -crate::fixed::ONE_S23,
        }
    }

    /// Advance one sample. `rate_reg` is the raw S.23 rate register
    /// (WLDS stores `freq << 14`, so the rotation step is `freq / 2^17` rad).
    pub fn tick(&mut self, rate_reg: i32) {
        let coeff = i64::from(rate_reg >> 8);
        self.cos = (i64::from(self.cos) + mul_s23(i64::from(self.sin), coeff)) as i32;
        self.sin = (i64::from(self.sin) - mul_s23(i64::from(self.cos), coeff)) as i32;
    }

    /// Range-scaled output as read by `CHO RDAL` (raw S.23).
    pub fn value(&self, range_reg: i32, cos: bool) -> i32 {
        mul_s23(
            i64::from(if cos { self.cos } else { self.sin }),
            i64::from(range_reg),
        ) as i32
    }

    /// Resolve a `CHO` access: address offset plus interpolation coefficient
    /// from the 10 fractional bits below the sample offset — the same
    /// address/fraction split as the RMP LFO datapath.
    ///
    /// The excursion in delay samples is `amp / 4`: AN-0001 gives the
    /// amplitude formula `Ka = N * 32767 / 16385` for a delay of N samples
    /// (i.e. `Ka ≈ 2N`, excursion `±(N-1)/2 ≈ ±Ka/4`) and its worked chorus
    /// example pairs `Ka = 16384` with "+/-4096 samples" in a `MEM 8193`
    /// buffer; the SPINAsm manual's WLDS/CHO examples repeat the same 2:1
    /// amplitude-to-length pairing (`Amp EQU 8194` for a 4097-sample line).
    pub fn cho(&self, range_reg: i32, flags: ChoFlags) -> ChoValue {
        let v = self.value(range_reg, flags.contains(ChoFlags::COS));
        let mut coeff = (v & 0x3FF) << 13;
        let v = if flags.contains(ChoFlags::COMPA) {
            -v
        } else {
            v
        };
        if flags.contains(ChoFlags::COMPC) {
            coeff = ACC_MAX - coeff;
        }
        ChoValue {
            offset: v >> 10,
            coeff,
        }
    }
}

/// Ramp LFO: a phase accumulator over a 22-bit space where 1024 units equal
/// one delay sample; the window is 4096/2048/1024/512 samples.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RampLfo {
    phase: i32,
}

/// Full-scale ramp phase: 4096 samples × 1024 fractional units.
const RAMP_FULL: i32 = 0x3F_FFFF;

impl RampLfo {
    pub fn new() -> Self {
        Self { phase: 0 }
    }

    /// `JAM` — reset the ramp phase.
    pub fn jam(&mut self) {
        self.phase = 0;
    }

    fn amp_shift(range_reg: i32) -> i32 {
        (range_reg >> 21) & 3
    }

    /// Current wrap range for the configured window (`range_reg` stores the
    /// 2-bit amplitude code shifted left by 21).
    fn range(range_reg: i32) -> i32 {
        RAMP_FULL >> Self::amp_shift(range_reg)
    }

    /// Advance one sample. WLDR stores `freq << 8` in the rate register; the
    /// phase moves by `freq / 16` units, so the tap slides `freq / 16384`
    /// samples per sample (pitch ratio `1 + freq / 16384`).
    pub fn tick(&mut self, rate_reg: i32, range_reg: i32) {
        let rate = (rate_reg >> 8) >> 4;
        self.phase = (self.phase - rate) & Self::range(range_reg);
    }

    /// Raw phase as an S.23 value (what `CHO RDAL` reads).
    pub fn value(&self) -> i32 {
        self.phase
    }

    /// Cross-fade envelope: a triangle spanning the ramp period, 0 → 1 → 0.
    fn crossfade(&self, range_reg: i32) -> i32 {
        let r = Self::range(range_reg);
        let v = if self.phase > r / 2 {
            r - self.phase
        } else {
            self.phase
        };
        v << (2 + Self::amp_shift(range_reg))
    }

    /// Resolve a `CHO` access; the coefficient comes from the 10 fractional
    /// bits below the sample offset (or the cross-fade envelope with `NA`).
    pub fn cho(&self, range_reg: i32, flags: ChoFlags) -> ChoValue {
        if flags.contains(ChoFlags::NA) {
            let mut coeff = self.crossfade(range_reg);
            if flags.contains(ChoFlags::COMPC) {
                coeff = ACC_MAX - coeff;
            }
            ChoValue { offset: 0, coeff }
        } else {
            let r = Self::range(range_reg);
            let mut v = self.phase;
            if flags.contains(ChoFlags::RPTR2) {
                v = (v + (r >> 1)) & r;
            }
            let mut coeff = (v & 0x3FF) << 13;
            if flags.contains(ChoFlags::COMPA) {
                v = r - v;
            }
            if flags.contains(ChoFlags::COMPC) {
                coeff = ACC_MAX - coeff;
            }
            ChoValue {
                offset: v >> 10,
                coeff,
            }
        }
    }
}
