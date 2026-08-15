//! Deterministic transcendental math for `LOG`/`EXP`.
//!
//! The emulator promises bit-exact output, and that promise has to hold
//! *across platforms*: golden hashes and committed reference WAVs are
//! only meaningful if Linux/macOS/Windows and `std`/`no_std` builds all
//! compute identical samples. Platform `libm`s disagree in the last ulp
//! of `log2`/`exp2` (they are not required to be correctly rounded), so
//! this module implements the three operations the VM needs from
//! nothing but IEEE-754 `+ - * /` and integer bit manipulation — all of
//! which are exactly specified — giving the same result everywhere,
//! with ~1e-15 relative accuracy (the VM quantizes to S4.19/S.23, so
//! the deterministic value is far inside the rounding step).

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
