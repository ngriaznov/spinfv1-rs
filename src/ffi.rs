//! C ABI for embedding the emulator in any non-Rust audio host.
//!
//! Build the static library with:
//!
//! ```text
//! cargo rustc --release --features ffi --crate-type staticlib
//! ```
//!
//! and include `include/spinfv1.h`, which mirrors this module one to
//! one. The handle owns either a bare chip (crystal-swap mode: the VM
//! runs at whatever rate the host calls it) or a boundary-resampled
//! chip pinned to the authentic 32,768 Hz (see [`crate::resampler`]).
//!
//! Every function is panic-proof: unwinds are caught at the boundary
//! and surface as [`SPINFV1_ERR_PANIC`] instead of crossing into the
//! host. Functions taking a handle are safe to call from the audio
//! thread once the handle exists; creation and program loading
//! allocate and belong on the main thread.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::resampler::HostedFv1;
use crate::{Fv1, Program, assemble};

/// Success.
pub const SPINFV1_OK: i32 = 0;
/// A required pointer argument was null.
pub const SPINFV1_ERR_NULL: i32 = -1;
/// The program bytes or source were rejected; see `spinfv1_last_error`.
pub const SPINFV1_ERR_PROGRAM: i32 = -2;
/// An internal panic was caught at the FFI boundary.
pub const SPINFV1_ERR_PANIC: i32 = -3;

enum Engine {
    /// Crystal-swap mode: the chip runs at the caller's call rate.
    Native(Fv1),
    /// Boundary-resampled to the authentic chip rate.
    Hosted(HostedFv1),
}

impl Engine {
    fn chip_mut(&mut self) -> &mut Fv1 {
        match self {
            Self::Native(chip) => chip,
            Self::Hosted(hosted) => hosted.chip_mut(),
        }
    }

    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        match self {
            Self::Native(chip) => chip.process(l, r),
            Self::Hosted(hosted) => hosted.process(l, r),
        }
    }
}

/// Opaque emulator handle (`struct SpinFv1` in the header).
pub struct SpinFv1 {
    engine: Engine,
    last_error: CString,
}

impl SpinFv1 {
    fn set_error(&mut self, message: &str) {
        self.last_error =
            CString::new(message.replace('\0', " ")).unwrap_or_else(|_| CString::default());
    }
}

/// Create an emulator.
///
/// `host_rate` > 0 selects boundary-resampled mode: the chip runs at
/// its native 32,768 Hz and `spinfv1_process` converts from/to the
/// given host rate. `host_rate` == 0 selects crystal-swap mode: the
/// chip processes one sample per call at whatever rate the host calls
/// it (officially supported chip behavior — time-based effects scale).
///
/// Returns null unless `host_rate` is 0 or within 1,000..=1,000,000 Hz
/// (any real audio host; extreme ratios would make the converter's
/// priming and buffering unbounded). Free with `spinfv1_destroy`.
#[unsafe(no_mangle)]
pub extern "C" fn spinfv1_create(host_rate: f64) -> *mut SpinFv1 {
    let engine = if host_rate == 0.0 {
        Engine::Native(Fv1::new())
    } else if (1_000.0..=1_000_000.0).contains(&host_rate) {
        match catch_unwind(|| HostedFv1::new(Fv1::new(), host_rate)) {
            Ok(hosted) => Engine::Hosted(hosted),
            Err(_) => return std::ptr::null_mut(),
        }
    } else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(SpinFv1 {
        engine,
        last_error: CString::default(),
    }))
}

/// Destroy a handle created by `spinfv1_create`. Null is a no-op.
///
/// # Safety
///
/// `handle` must be null or a pointer returned by `spinfv1_create`
/// that has not already been destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_destroy(handle: *mut SpinFv1) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

unsafe fn with_handle(handle: *mut SpinFv1, f: impl FnOnce(&mut SpinFv1) -> i32) -> i32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return SPINFV1_ERR_NULL;
    };
    catch_unwind(AssertUnwindSafe(|| f(handle))).unwrap_or(SPINFV1_ERR_PANIC)
}

/// Load a program from raw bank bytes (the 512-byte big-endian EEPROM
/// format). `len` may cover a multi-program bank; `program` selects a
/// 512-byte slot (0 for a single-program image). Resets DSP state like
/// a hardware program switch.
///
/// # Safety
///
/// `handle` must be a live handle; `bytes` must point to `len`
/// readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_load_bank(
    handle: *mut SpinFv1,
    bytes: *const u8,
    len: usize,
    program: u32,
) -> i32 {
    if bytes.is_null() {
        return SPINFV1_ERR_NULL;
    }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    unsafe {
        with_handle(handle, |h| {
            let slot = (program as usize)
                .checked_mul(512)
                .and_then(|offset| bytes.get(offset..offset.checked_add(512)?));
            let Some(slot) = slot else {
                h.set_error("bank too short for requested program slot");
                return SPINFV1_ERR_PROGRAM;
            };
            match Program::from_bytes(slot) {
                Ok(p) => {
                    h.engine.chip_mut().load_program(&p);
                    SPINFV1_OK
                }
                Err(e) => {
                    h.set_error(&e.to_string());
                    SPINFV1_ERR_PROGRAM
                }
            }
        })
    }
}

/// Assemble SpinASM source (NUL-terminated UTF-8) and load it. On
/// failure the message is available via `spinfv1_last_error`.
///
/// # Safety
///
/// `handle` must be a live handle; `source` must be a NUL-terminated
/// string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_load_asm(handle: *mut SpinFv1, source: *const c_char) -> i32 {
    if source.is_null() {
        return SPINFV1_ERR_NULL;
    }
    let source = unsafe { CStr::from_ptr(source) };
    unsafe {
        with_handle(handle, |h| {
            let Ok(source) = source.to_str() else {
                h.set_error("source is not valid UTF-8");
                return SPINFV1_ERR_PROGRAM;
            };
            match assemble(source) {
                Ok(p) => {
                    h.engine.chip_mut().load_program(&p);
                    SPINFV1_OK
                }
                Err(e) => {
                    h.set_error(&e.to_string());
                    SPINFV1_ERR_PROGRAM
                }
            }
        })
    }
}

/// The most recent load error message (empty string if none). The
/// pointer stays valid until the next load call on this handle or
/// `spinfv1_destroy`; returns null only for a null handle.
///
/// # Safety
///
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_last_error(handle: *const SpinFv1) -> *const c_char {
    match unsafe { handle.as_ref() } {
        Some(h) => h.last_error.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Set a pot (0..=2) to `value` (clamped to 0..=1). Out-of-range
/// indices are ignored. Real-time safe.
///
/// # Safety
///
/// `handle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_set_pot(handle: *mut SpinFv1, pot: u32, value: f32) -> i32 {
    unsafe {
        with_handle(handle, |h| {
            h.engine.chip_mut().set_pot(pot as usize, value);
            SPINFV1_OK
        })
    }
}

/// Toggle the chip's 14-bit compressed floating-point delay RAM model
/// (`enabled` != 0; enabled by default). Disable for pristine
/// full-precision delay storage.
///
/// # Safety
///
/// `handle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_set_delay_quantization(handle: *mut SpinFv1, enabled: i32) -> i32 {
    unsafe {
        with_handle(handle, |h| {
            h.engine.chip_mut().set_delay_quantization(enabled != 0);
            SPINFV1_OK
        })
    }
}

/// Fill delay RAM with a deterministic pseudo-random pattern, like the
/// uninitialized SRAM of a freshly powered chip. Programs that read
/// never-written regions as a noise source need this; call it after
/// loading such a program.
///
/// # Safety
///
/// `handle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_randomize_delay_ram(handle: *mut SpinFv1, seed: u32) -> i32 {
    unsafe {
        with_handle(handle, |h| {
            h.engine.chip_mut().randomize_delay_ram(seed);
            SPINFV1_OK
        })
    }
}

/// Process one stereo frame (inputs nominally in -1..=1). Real-time
/// safe. On error the outputs are written as silence.
///
/// # Safety
///
/// `handle` must be a live handle; `out_l` and `out_r` must be
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_process(
    handle: *mut SpinFv1,
    in_l: f32,
    in_r: f32,
    out_l: *mut f32,
    out_r: *mut f32,
) -> i32 {
    if out_l.is_null() || out_r.is_null() {
        return SPINFV1_ERR_NULL;
    }
    unsafe {
        *out_l = 0.0;
        *out_r = 0.0;
        with_handle(handle, |h| {
            let (l, r) = h.engine.process(in_l, in_r);
            *out_l = l;
            *out_r = r;
            SPINFV1_OK
        })
    }
}

/// Process `frames` stereo frames from/to separate channel buffers.
/// Real-time safe.
///
/// # Safety
///
/// `handle` must be a live handle; all four buffers must cover
/// `frames` readable/writable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_process_block(
    handle: *mut SpinFv1,
    in_l: *const f32,
    in_r: *const f32,
    out_l: *mut f32,
    out_r: *mut f32,
    frames: usize,
) -> i32 {
    if in_l.is_null() || in_r.is_null() || out_l.is_null() || out_r.is_null() {
        return SPINFV1_ERR_NULL;
    }
    unsafe {
        with_handle(handle, |h| {
            for i in 0..frames {
                let (l, r) = h.engine.process(*in_l.add(i), *in_r.add(i));
                *out_l.add(i) = l;
                *out_r.add(i) = r;
            }
            SPINFV1_OK
        })
    }
}

/// Reset DSP state (registers, delay RAM, LFOs, accumulator), as after
/// a program load. In resampled mode up to ~1 ms of in-flight audio in
/// the converters is unaffected.
///
/// # Safety
///
/// `handle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_reset(handle: *mut SpinFv1) -> i32 {
    unsafe {
        with_handle(handle, |h| {
            h.engine.chip_mut().reset();
            SPINFV1_OK
        })
    }
}

/// Pipeline latency in host samples: the resampling delay in
/// boundary-resampled mode, 0 in crystal-swap mode or for a null
/// handle. Report it to the host for delay compensation.
///
/// # Safety
///
/// `handle` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinfv1_latency(handle: *const SpinFv1) -> u32 {
    match unsafe { handle.as_ref() } {
        Some(SpinFv1 {
            engine: Engine::Hosted(hosted),
            ..
        }) => hosted.latency() as u32,
        _ => 0,
    }
}

/// The chip's native sample rate, 32,768 Hz.
#[unsafe(no_mangle)]
pub extern "C" fn spinfv1_native_rate() -> f64 {
    crate::resampler::CHIP_RATE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panic wall itself: a panicking operation must surface as
    /// [`SPINFV1_ERR_PANIC`] and leave the handle usable, never unwind
    /// across the boundary.
    #[test]
    fn with_handle_contains_panics_and_handle_survives() {
        let handle = spinfv1_create(0.0);
        assert!(!handle.is_null());
        let code = unsafe { with_handle(handle, |_| -> i32 { panic!("deliberate test panic") }) };
        assert_eq!(code, SPINFV1_ERR_PANIC);
        // The handle must still work after a contained panic.
        unsafe {
            let (mut l, mut r) = (1.0f32, 1.0f32);
            assert_eq!(
                spinfv1_process(handle, 0.0, 0.0, &raw mut l, &raw mut r),
                SPINFV1_OK
            );
            assert_eq!((l, r), (0.0, 0.0));
            spinfv1_destroy(handle);
        }
    }

    #[test]
    fn with_handle_rejects_null_before_running() {
        let code = unsafe { with_handle(std::ptr::null_mut(), |_| SPINFV1_OK) };
        assert_eq!(code, SPINFV1_ERR_NULL);
    }
}
