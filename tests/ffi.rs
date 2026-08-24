//! C ABI surface tests: lifecycle, error paths, panic containment, and
//! parity with the direct Rust API.
//!
//! Run with `cargo test --features ffi`.
#![cfg(feature = "ffi")]

use std::ffi::{CStr, CString};

use spinfv1::ffi::{
    SPINFV1_ERR_NULL, SPINFV1_ERR_PROGRAM, SPINFV1_OK, spinfv1_assemble, spinfv1_create,
    spinfv1_destroy, spinfv1_last_error, spinfv1_latency, spinfv1_load_asm, spinfv1_load_bank,
    spinfv1_native_rate, spinfv1_process, spinfv1_process_block, spinfv1_reset, spinfv1_set_pot,
};
use spinfv1::{Fv1, assemble};

const ECHO: &str = "
    DELAY MEM 1000
    ldax ADCL
    wra DELAY, 0
    rda DELAY#, 0.5
    wrax DACL, 0
    ldax ADCR
    wrax DACR, 0
";

#[test]
fn create_modes_and_destroy() {
    let native = spinfv1_create(0.0);
    assert!(!native.is_null());
    let hosted = spinfv1_create(48_000.0);
    assert!(!hosted.is_null());
    unsafe {
        assert_eq!(spinfv1_latency(native), 0, "crystal-swap has no latency");
        assert!(
            spinfv1_latency(hosted) > 0,
            "resampled mode reports latency"
        );
        spinfv1_destroy(native);
        spinfv1_destroy(hosted);
        spinfv1_destroy(std::ptr::null_mut()); // null is a no-op
    }
    assert!(spinfv1_create(-1.0).is_null());
    assert!(spinfv1_create(f64::NAN).is_null());
    // Extreme finite rates are rejected too: the converter's priming
    // and buffering must stay bounded.
    assert!(spinfv1_create(1e-300).is_null());
    assert!(spinfv1_create(999.0).is_null());
    assert!(spinfv1_create(1e15).is_null());
    let edge = spinfv1_create(1_000.0);
    assert!(!edge.is_null());
    unsafe { spinfv1_destroy(edge) };
    assert!((spinfv1_native_rate() - 32_768.0).abs() < f64::EPSILON);
}

#[test]
fn null_arguments_are_rejected_not_crashed() {
    unsafe {
        let mut l = 1.0f32;
        let mut r = 1.0f32;
        assert_eq!(
            spinfv1_process(std::ptr::null_mut(), 0.0, 0.0, &raw mut l, &raw mut r),
            SPINFV1_ERR_NULL
        );
        assert_eq!(
            spinfv1_load_asm(std::ptr::null_mut(), std::ptr::null()),
            SPINFV1_ERR_NULL
        );
        assert_eq!(spinfv1_reset(std::ptr::null_mut()), SPINFV1_ERR_NULL);
        assert!(spinfv1_last_error(std::ptr::null()).is_null());
        let h = spinfv1_create(0.0);
        assert_eq!(
            spinfv1_process(h, 0.0, 0.0, std::ptr::null_mut(), std::ptr::null_mut()),
            SPINFV1_ERR_NULL
        );
        assert_eq!(
            spinfv1_load_bank(h, std::ptr::null(), 512, 0),
            SPINFV1_ERR_NULL
        );
        spinfv1_destroy(h);
    }
}

#[test]
fn bad_programs_report_errors_with_messages() {
    unsafe {
        let h = spinfv1_create(0.0);
        let bad = CString::new("BOGUS_MNEMONIC 1, 2\n").unwrap();
        assert_eq!(spinfv1_load_asm(h, bad.as_ptr()), SPINFV1_ERR_PROGRAM);
        let msg = CStr::from_ptr(spinfv1_last_error(h)).to_string_lossy();
        assert!(
            msg.to_lowercase().contains("mnemonic"),
            "unhelpful error: {msg}"
        );
        // A short bank slot is rejected, not read out of bounds.
        let bank = [0u8; 100];
        assert_eq!(
            spinfv1_load_bank(h, bank.as_ptr(), bank.len(), 0),
            SPINFV1_ERR_PROGRAM
        );
        // A slot index whose byte offset would overflow is rejected,
        // not wrapped into a bogus in-range slot.
        assert_eq!(
            spinfv1_load_bank(h, bank.as_ptr(), bank.len(), u32::MAX),
            SPINFV1_ERR_PROGRAM
        );
        spinfv1_destroy(h);
    }
}

#[test]
fn ffi_output_matches_direct_rust_api() {
    let program = assemble(ECHO).unwrap();
    let mut direct = Fv1::new();
    direct.load_program(&program);
    direct.set_pot(0, 0.3);

    unsafe {
        let h = spinfv1_create(0.0);
        let source = CString::new(ECHO).unwrap();
        assert_eq!(spinfv1_load_asm(h, source.as_ptr()), SPINFV1_OK);
        assert_eq!(spinfv1_set_pot(h, 0, 0.3), SPINFV1_OK);

        for n in 0..4000u32 {
            let x = if n % 37 == 0 { 0.5 } else { -0.01 };
            let expected = direct.process(x, -x);
            let (mut l, mut r) = (0.0f32, 0.0f32);
            assert_eq!(
                spinfv1_process(h, x, -x, &raw mut l, &raw mut r),
                SPINFV1_OK
            );
            assert_eq!((l, r), expected, "frame {n}");
        }
        spinfv1_destroy(h);
    }
}

#[test]
fn block_processing_matches_per_sample() {
    let source = CString::new(ECHO).unwrap();
    unsafe {
        let a = spinfv1_create(48_000.0);
        let b = spinfv1_create(48_000.0);
        assert_eq!(spinfv1_load_asm(a, source.as_ptr()), SPINFV1_OK);
        assert_eq!(spinfv1_load_asm(b, source.as_ptr()), SPINFV1_OK);

        let n = 2048;
        let in_l: Vec<f32> = (0..n).map(|i| ((i % 100) as f32 - 50.0) / 100.0).collect();
        let in_r: Vec<f32> = in_l.iter().map(|x| -x).collect();
        let mut out_l = vec![0.0f32; n];
        let mut out_r = vec![0.0f32; n];
        assert_eq!(
            spinfv1_process_block(
                a,
                in_l.as_ptr(),
                in_r.as_ptr(),
                out_l.as_mut_ptr(),
                out_r.as_mut_ptr(),
                n
            ),
            SPINFV1_OK
        );
        for i in 0..n {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            assert_eq!(
                spinfv1_process(b, in_l[i], in_r[i], &raw mut l, &raw mut r),
                SPINFV1_OK
            );
            assert_eq!((out_l[i], out_r[i]), (l, r), "frame {i}");
        }
        spinfv1_destroy(a);
        spinfv1_destroy(b);
    }
}

#[test]
fn reset_clears_echo_tail() {
    let source = CString::new(ECHO).unwrap();
    unsafe {
        let h = spinfv1_create(0.0);
        assert_eq!(spinfv1_load_asm(h, source.as_ptr()), SPINFV1_OK);
        let (mut l, mut r) = (0.0f32, 0.0f32);
        spinfv1_process(h, 0.9, 0.0, &raw mut l, &raw mut r);
        assert_eq!(spinfv1_reset(h), SPINFV1_OK);
        for n in 0..2000 {
            spinfv1_process(h, 0.0, 0.0, &raw mut l, &raw mut r);
            assert_eq!(l, 0.0, "stale echo after reset at frame {n}");
        }
        spinfv1_destroy(h);
    }
}

#[test]
fn assemble_matches_the_library_assembler() {
    let source = CString::new(ECHO).unwrap();
    let expected = spinfv1::assemble(ECHO).unwrap().to_bytes();
    let mut image = [0u8; 512];
    let mut err = [0 as std::ffi::c_char; 64];
    unsafe {
        assert_eq!(
            spinfv1_assemble(
                source.as_ptr(),
                image.as_mut_ptr(),
                err.as_mut_ptr(),
                err.len()
            ),
            SPINFV1_OK
        );
    }
    assert_eq!(image, expected);
    assert_eq!(err[0], 0);

    let bad = CString::new("garbage\n").unwrap();
    unsafe {
        assert_eq!(
            spinfv1_assemble(
                bad.as_ptr(),
                image.as_mut_ptr(),
                err.as_mut_ptr(),
                err.len()
            ),
            SPINFV1_ERR_PROGRAM
        );
        assert_ne!(err[0], 0);
    }
}
