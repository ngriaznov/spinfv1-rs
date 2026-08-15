//! Fixed-point formats and arithmetic used by the FV-1.
//!
//! The FV-1 accumulator and registers are 24-bit signed values in **S.23**
//! format: 1 sign bit, 23 fractional bits, representing `[-1.0, +1.0)`.
//! We store them sign-extended in [`i32`] and do intermediate arithmetic in
//! [`i64`], saturating back to 24 bits at each architectural write — exactly
//! what the chip's saturating ALU does.
//!
//! Coefficient formats that appear in instruction words:
//!
//! | format | bits | fractional | range              | used by                    |
//! |--------|------|------------|--------------------|----------------------------|
//! | S1.14  | 16   | 14         | `[-2.0, +2.0)`     | RDAX/WRAX/... , LOG/EXP/SOF C |
//! | S1.9   | 11   | 9          | `[-2.0, +2.0)`     | RDA/RMPA/WRA/WRAP C        |
//! | S.10   | 11   | 10         | `[-1.0, +1.0)`     | SOF/EXP D                  |
//! | S4.6   | 11   | 6          | `[-16.0, +16.0)`   | LOG D                      |
//! | S.15   | 16   | 15         | `[-1.0, +1.0)`     | CHO SOF D, WLDS amplitude  |
//! | S4.19  | 24   | 19         | `[-16.0, +16.0)`   | ACC as seen by LOG/EXP     |

/// One (`1.0`) in S.23, i.e. `2^23`. The accumulator can never quite reach it.
pub const ONE_S23: i32 = 1 << 23;
/// Largest representable accumulator value (`+0.99999988...`).
pub const ACC_MAX: i32 = ONE_S23 - 1;
/// Smallest representable accumulator value (`-1.0`).
pub const ACC_MIN: i32 = -ONE_S23;

/// Saturate a wide intermediate result to the 24-bit accumulator range.
#[inline]
#[must_use]
pub const fn sat24(v: i64) -> i32 {
    if v > ACC_MAX as i64 {
        ACC_MAX
    } else if v < ACC_MIN as i64 {
        ACC_MIN
    } else {
        v as i32
    }
}

/// Sign-extend a 24-bit value (bits above 23 are discarded first).
#[inline]
#[must_use]
pub const fn sx24(v: i32) -> i32 {
    (v << 8) >> 8
}

/// Sign-extend the low `bits` bits of a raw field.
#[inline]
#[must_use]
pub const fn sx(v: u32, bits: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

/// Convert a float sample in `[-1.0, 1.0)` to S.23 (rounded, saturated).
#[inline]
#[must_use]
pub fn f32_to_s23(x: f32) -> i32 {
    sat24((f64::from(x) * ONE_S23 as f64).round() as i64)
}

/// Convert an S.23 value to a float sample.
#[inline]
#[must_use]
pub fn s23_to_f32(v: i32) -> f32 {
    (v as f64 / ONE_S23 as f64) as f32
}

/// Multiply an S.23 value by an S1.14 coefficient (raw), truncating like the
/// hardware multiplier. The result is *not* saturated; callers saturate at the
/// architectural write.
#[inline]
#[must_use]
pub const fn mul_s1_14(x: i64, c: i64) -> i64 {
    (x * c) >> 14
}

/// Multiply an S.23 value by an S1.9 coefficient (raw), truncating.
#[inline]
#[must_use]
pub const fn mul_s1_9(x: i64, c: i64) -> i64 {
    (x * c) >> 9
}

/// Multiply two S.23 values (raw), truncating.
#[inline]
#[must_use]
pub const fn mul_s23(x: i64, y: i64) -> i64 {
    (x * y) >> 23
}

/// Coefficient quantizers: convert real numbers to the raw two's-complement
/// field values an assembler would emit. Values are rounded to nearest and
/// clamped to the representable range, mirroring SpinASM/reference-impl.
pub mod coeff {
    /// S1.14: 16-bit field, `[-2.0, 1.99993896484375]`.
    #[must_use]
    pub fn s1_14(x: f64) -> i16 {
        (x * 16384.0).round().clamp(-32768.0, 32767.0) as i16
    }

    /// S1.9: 11-bit field, `[-2.0, 1.998046875]`.
    #[must_use]
    pub fn s1_9(x: f64) -> i16 {
        (x * 512.0).round().clamp(-1024.0, 1023.0) as i16
    }

    /// S.10: 11-bit field, `[-1.0, 0.9990234375]`.
    #[must_use]
    pub fn s_10(x: f64) -> i16 {
        (x * 1024.0).round().clamp(-1024.0, 1023.0) as i16
    }

    /// S4.6: 11-bit field, `[-16.0, 15.984375]`.
    #[must_use]
    pub fn s4_6(x: f64) -> i16 {
        (x * 64.0).round().clamp(-1024.0, 1023.0) as i16
    }

    /// S.15: 16-bit field, `[-1.0, 0.999969482421875]`.
    #[must_use]
    pub fn s_15(x: f64) -> i16 {
        (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16
    }

    /// S.23: 24-bit register value, `[-1.0, ~1.0)`.
    #[must_use]
    pub fn s_23(x: f64) -> i32 {
        super::sat24((x * super::ONE_S23 as f64).round() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_rails() {
        assert_eq!(sat24(i64::from(ACC_MAX) + 1), ACC_MAX);
        assert_eq!(sat24(i64::from(ACC_MIN) - 1), ACC_MIN);
        assert_eq!(sat24(1234), 1234);
        assert_eq!(sat24(-1234), -1234);
    }

    #[test]
    fn sign_extension() {
        assert_eq!(sx24(0x00FF_FFFF), -1);
        assert_eq!(sx24(0x007F_FFFF), ACC_MAX);
        assert_eq!(sx24(0x0080_0000), ACC_MIN);
        assert_eq!(sx(0x7FF, 11), -1);
        assert_eq!(sx(0x3FF, 11), 1023);
        assert_eq!(sx(0x400, 11), -1024);
    }

    #[test]
    fn float_conversion_roundtrip() {
        for v in [0, 1, -1, 12345, -12345, ACC_MAX, ACC_MIN] {
            assert_eq!(f32_to_s23(s23_to_f32(v)), v, "roundtrip of {v}");
        }
        assert_eq!(f32_to_s23(2.0), ACC_MAX);
        assert_eq!(f32_to_s23(-2.0), ACC_MIN);
    }

    #[test]
    fn multiplier_truncates_toward_negative_infinity() {
        // (3 * 1) >> 14 with sign: -3 * 16383 = -49149 >> 14 = -3 (floor of -2.99...)
        assert_eq!(mul_s1_14(-3, 16383), -3);
        assert_eq!(mul_s1_14(3, 16383), 2);
    }

    #[test]
    fn coeff_quantizers() {
        assert_eq!(coeff::s1_14(1.0), 16384);
        assert_eq!(coeff::s1_14(-2.0), -32768);
        assert_eq!(coeff::s1_14(2.0), 32767);
        assert_eq!(coeff::s1_9(-1.0), -512);
        assert_eq!(coeff::s_10(0.5), 512);
        assert_eq!(coeff::s4_6(-16.0), -1024);
        assert_eq!(coeff::s_15(0.999), 32735);
        assert_eq!(coeff::s_23(-1.0), ACC_MIN);
    }
}
