//! Deterministic transcendental and elementary math (`no_std`, no `libm`).
//!
//! The emulator promises bit-exact output, and that promise has to hold
//! *across platforms*: golden hashes and committed reference WAVs are
//! only meaningful if Linux/macOS/Windows and `std`/`no_std` builds all
//! compute identical samples. Platform `libm`s disagree in the last ulp
//! of `log2`/`exp2`/`sin` (they are not required to be correctly
//! rounded), so this module implements the operations the VM, assembler,
//! and polyphase resampler need from nothing but IEEE-754 `+ - * /` and
//! integer bit manipulation — all of which are exactly specified —
//! giving the same result everywhere, with ~1e-15 relative accuracy.

/// Round to the nearest integer, ties away from zero.
///
/// Exact for |x| < 2^52: `trunc` via `as i64` is exact, and `x - trunc(x)`
/// is exact IEEE subtraction (both are multiples of `ulp(x) <= 1`).
#[inline]
#[must_use]
pub(crate) fn round(x: f64) -> f64 {
    let t = x as i64 as f64; // trunc toward zero, exact in our range
    let frac = x - t; // exact
    if frac >= 0.5 {
        t + 1.0
    } else if frac <= -0.5 {
        t - 1.0
    } else {
        t
    }
}

/// Largest integer less than or equal to `x` (`floor`), exact for |x| < 2^52.
#[cfg(any(feature = "resampler", test))]
#[inline]
#[must_use]
pub(crate) fn floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if x < t { t - 1.0 } else { t }
}

/// Non-negative square root for finite `x >= 0`.
///
/// Seeds Newton-Raphson by halving the IEEE-754 biased exponent (within a
/// factor of `sqrt(2)` of the root), then runs six Heron iterations to
/// reach full 53-bit precision across the normal range.
#[cfg(any(feature = "resampler", test))]
#[inline]
#[must_use]
pub(crate) fn sqrt(x: f64) -> f64 {
    debug_assert!(x >= 0.0 && x.is_finite());
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f64::from_bits((x.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..6 {
        y = 0.5 * (y + x / y);
    }
    y
}

/// Sine for finite `x` (used to build the polyphase sinc kernel at
/// resampler construction).
///
/// Reduces `x` modulo `pi` into `[-pi/2, pi/2]` and evaluates the odd
/// Taylor polynomial through `r^21 / 21!` (~1e-16 accuracy).
#[cfg(any(feature = "resampler", test))]
#[inline]
#[must_use]
pub(crate) fn sin(x: f64) -> f64 {
    debug_assert!(x.is_finite());
    const PI: f64 = core::f64::consts::PI;
    let k = round(x / PI);
    let r = x - k * PI;
    let r2 = r * r;
    let mut term = r;
    let mut sum = r;
    let mut n = 2.0;
    for _ in 0..10 {
        term *= -r2 / (n * (n + 1.0));
        sum += term;
        n += 2.0;
    }
    if (k as i64) & 1 != 0 { -sum } else { sum }
}

const LN_2: f64 = core::f64::consts::LN_2;
const SQRT_2: f64 = core::f64::consts::SQRT_2;

/// Base-2 logarithm for finite `x > 0`.
///
/// Splits `x` into exponent and mantissa by bit manipulation, centers
/// the mantissa on `[sqrt(2)/2, sqrt(2)]`, and evaluates
/// `ln(m) = 2 * atanh((m - 1) / (m + 1))` by its odd power series
/// (`|z| <= 0.1716`, eleven terms reach ~1e-17).
#[inline]
#[must_use]
pub(crate) fn log2(x: f64) -> f64 {
    debug_assert!(x > 0.0 && x.is_finite());
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let mut m = if e == -1023 {
        // Subnormal (never produced by the VM's inputs, handled for
        // completeness): renormalize through a 2^52 scale.
        let scaled = x * f64::from_bits(0x4330_0000_0000_0000); // 2^52
        let sb = scaled.to_bits();
        e = ((sb >> 52) & 0x7FF) as i64 - 1023 - 52;
        f64::from_bits((sb & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000)
    } else {
        f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000)
    };
    if m > SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut term = z;
    let mut sum = z;
    let mut k = 3.0;
    for _ in 0..10 {
        term *= z2;
        sum += term / k;
        k += 2.0;
    }
    e as f64 + (2.0 * sum) / LN_2
}

/// Base-2 exponential (`2^x`) for `x` in roughly ±1000.
///
/// Splits into integer and fractional parts, evaluates `e^(r * ln 2)`
/// by Taylor series (`|y| <= 0.347`, fourteen terms reach ~1e-19), and
/// applies the integer exponent through the bit pattern.
#[inline]
#[must_use]
pub(crate) fn exp2(x: f64) -> f64 {
    debug_assert!(x.is_finite() && x.abs() < 1000.0);
    let n = round(x);
    let y = (x - n) * LN_2; // |y| <= ln(2)/2
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..=14 {
        term *= y / f64::from(k);
        sum += term;
    }
    // 2^n via the exponent field (n is within ±1000, always normal here).
    let scale = f64::from_bits((((n as i64) + 1023) as u64) << 52);
    sum * scale
}

/// Real power `base^exp` for constant-expression evaluation in the assembler.
#[inline]
#[must_use]
pub(crate) fn powf(base: f64, exp: f64) -> f64 {
    if exp == 0.0 || base == 1.0 {
        return 1.0;
    }
    let exp_int = round(exp);
    if exp == exp_int && exp_int.abs() <= 1024.0 {
        let mut n = (exp_int as i64).unsigned_abs();
        let mut b = base;
        let mut acc = 1.0;
        while n > 0 {
            if n & 1 != 0 {
                acc *= b;
            }
            b *= b;
            n >>= 1;
        }
        return if exp_int < 0.0 { 1.0 / acc } else { acc };
    }
    if base <= 0.0 {
        return if base == 0.0 && exp > 0.0 {
            0.0
        } else {
            f64::NAN
        };
    }
    exp2(exp * log2(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_std_including_tie_edges() {
        for x in [
            0.0,
            0.4,
            0.5,
            0.6,
            -0.5,
            1.5,
            -1.5,
            12345.6789,
            0.499_999_999_999_999_94,
            -0.499_999_999_999_999_94,
            8_388_607.499,
            -8_388_608.5,
        ] {
            assert_eq!(round(x), x.round(), "round({x})");
        }
    }

    #[test]
    fn floor_matches_std() {
        for x in [
            0.0, -0.0, 0.1, 0.999, 1.0, -0.1, -0.999, -1.0, -123.456, 789.012,
        ] {
            assert_eq!(floor(x), x.floor(), "floor({x})");
        }
    }

    #[test]
    fn sqrt_matches_std_to_sub_ulp_precision() {
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(1.0), 1.0);
        assert_eq!(sqrt(4.0), 2.0);
        for i in 1..=2000u32 {
            let x = f64::from(i) * 0.0137;
            let err = (sqrt(x) - x.sqrt()).abs() / x.sqrt();
            assert!(err < 1e-15, "sqrt({x}): {} vs {}", sqrt(x), x.sqrt());
        }
    }

    #[test]
    fn sin_matches_std_across_sinc_domain() {
        assert_eq!(sin(0.0), 0.0);
        for i in -2000i32..=2000 {
            let x = f64::from(i) * 0.025;
            let err = (sin(x) - x.sin()).abs();
            assert!(err < 1e-14, "sin({x}): {} vs {}", sin(x), x.sin());
        }
    }

    #[test]
    fn log2_matches_std_to_sub_ulp_precision() {
        // Sweep the VM's actual domain: mag/2^23 for mag in 1..=2^23.
        for i in 1..=2000u32 {
            for &x in &[
                f64::from(i) / 8_388_608.0,
                f64::from(i * 4001) / 8_388_608.0,
                f64::from(i) * 0.9973,
            ] {
                let rel = (log2(x) - x.log2()).abs() / x.log2().abs().max(1e-30);
                assert!(rel < 1e-14, "log2({x}): {} vs {}", log2(x), x.log2());
            }
        }
        assert_eq!(log2(1.0), 0.0);
        assert_eq!(log2(2.0), 1.0);
        assert_eq!(log2(0.25), -2.0);
        assert_eq!(log2(f64::from(1u32 << 22) / 8_388_608.0), -1.0);
    }

    #[test]
    fn exp2_matches_std_to_sub_ulp_precision() {
        for i in -2000i32..=2000 {
            let x = f64::from(i) / 121.7;
            let rel = (exp2(x) - x.exp2()).abs() / x.exp2();
            assert!(rel < 1e-14, "exp2({x}): {} vs {}", exp2(x), x.exp2());
        }
        assert_eq!(exp2(0.0), 1.0);
        assert_eq!(exp2(1.0), 2.0);
        assert_eq!(exp2(-16.0), 1.0 / 65536.0);
    }

    #[test]
    fn log2_exp2_round_trip() {
        for i in 1..=4000u32 {
            let x = f64::from(i) / 8_388_608.0;
            let rt = exp2(log2(x));
            assert!(((rt - x) / x).abs() < 1e-13, "round trip {x} -> {rt}");
        }
    }
}
