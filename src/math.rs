//! Tiny math shim so [`crate::fixed`] and [`crate::vm`] can use the handful
//! of transcendental/rounding operations they need under both `std` and
//! `no_std` (+ `libm`) builds.
//!
//! `core` does not provide floating-point transcendental functions (there is
//! no libm in `core`), so under `std` we dispatch to the inherent [`f64`]
//! methods (backed by the platform's libm), and under `no_std` we dispatch to
//! the pure-Rust `libm` crate. Behavior is identical either way: both paths
//! ultimately compute the same IEEE-754 operations.

/// Round to the nearest integer, ties away from zero.
#[inline]
#[must_use]
pub(crate) fn round(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        x.round()
    }
    #[cfg(not(feature = "std"))]
    {
        libm::round(x)
    }
}

/// Base-2 logarithm.
#[inline]
#[must_use]
pub(crate) fn log2(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        x.log2()
    }
    #[cfg(not(feature = "std"))]
    {
        libm::log2(x)
    }
}

/// Base-2 exponential (`2^x`).
#[inline]
#[must_use]
pub(crate) fn exp2(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        x.exp2()
    }
    #[cfg(not(feature = "std"))]
    {
        libm::exp2(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_std() {
        for x in [0.0, 0.4, 0.5, 0.6, -0.5, 1.5, -1.5, 12345.6789] {
            assert_eq!(round(x), x.round());
        }
    }

    #[test]
    fn log2_matches_std() {
        for x in [1.0, 2.0, 0.5, 1024.0, 0.001] {
            assert!((log2(x) - x.log2()).abs() < 1e-12);
        }
    }

    #[test]
    fn exp2_matches_std() {
        for x in [0.0, 1.0, -1.0, 10.0, -10.5] {
            assert!((exp2(x) - x.exp2()).abs() < 1e-9);
        }
    }
}
