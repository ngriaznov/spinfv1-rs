/* spinfv1 — Spin FV-1 DSP emulator, C API.
 *
 * Mirrors src/ffi.rs one to one. Build the static library with:
 *
 *     cargo rustc --release --features ffi --crate-type staticlib
 *
 * then link target/release/libspinfv1.a (add -lm on some platforms;
 * on Windows link the usual Rust system libs or use the MSVC .lib).
 *
 * Threading: a handle is not internally synchronized. Create/load on
 * the main thread; process/set_pot are allocation-free and real-time
 * safe from a single audio thread. All functions catch internal
 * panics and return SPINFV1_ERR_PANIC instead of unwinding into the
 * host.
 */
#ifndef SPINFV1_H
#define SPINFV1_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque emulator handle. */
typedef struct SpinFv1 SpinFv1;

enum {
    SPINFV1_OK = 0,
    SPINFV1_ERR_NULL = -1,    /* a required pointer was null */
    SPINFV1_ERR_PROGRAM = -2, /* program rejected; see spinfv1_last_error */
    SPINFV1_ERR_PANIC = -3,   /* internal panic caught at the boundary */
};

/* Create an emulator.
 *
 * host_rate > 0: boundary-resampled mode — the chip runs at its native
 * 32,768 Hz and spinfv1_process converts from/to host_rate, so delays,
 * LFO rates and filter cutoffs are exactly as on hardware. Report
 * spinfv1_latency() to the host for delay compensation.
 *
 * host_rate == 0: crystal-swap mode — the chip processes one sample
 * per call at whatever rate you call it (officially supported chip
 * behavior; time-based effects scale with the rate).
 *
 * Returns NULL unless host_rate is 0 or within 1000..=1000000 Hz. */
SpinFv1 *spinfv1_create(double host_rate);

/* Destroy a handle. NULL is a no-op. */
void spinfv1_destroy(SpinFv1 *handle);

/* Load a program from raw bank bytes (512-byte big-endian EEPROM
 * format). len may cover a multi-program bank; program selects a
 * 512-byte slot (0 for a single-program image). Resets DSP state like
 * a hardware program switch. */
int32_t spinfv1_load_bank(SpinFv1 *handle, const uint8_t *bytes, size_t len,
                          uint32_t program);

/* Assemble SpinASM source (NUL-terminated UTF-8) and load it. On
 * SPINFV1_ERR_PROGRAM the message is available via spinfv1_last_error. */
int32_t spinfv1_load_asm(SpinFv1 *handle, const char *source);

/* Most recent load error message ("" if none). Valid until the next
 * load call on this handle or spinfv1_destroy. NULL only for a NULL
 * handle. */
const char *spinfv1_last_error(const SpinFv1 *handle);

/* Set pot 0..=2 to value (clamped to 0..=1). Real-time safe. */
int32_t spinfv1_set_pot(SpinFv1 *handle, uint32_t pot, float value);

/* Toggle the chip's 14-bit compressed floating-point delay RAM model
 * (enabled != 0; enabled by default). */
int32_t spinfv1_set_delay_quantization(SpinFv1 *handle, int32_t enabled);

/* Fill delay RAM with a deterministic pseudo-random pattern, like the
 * uninitialized SRAM of a freshly powered chip. Programs that read
 * never-written regions as a noise source need this after loading. */
int32_t spinfv1_randomize_delay_ram(SpinFv1 *handle, uint32_t seed);

/* Fill the general-purpose registers (REG0..=REG31) with a deterministic
 * pseudo-random pattern, like uninitialized power-up state. Programs
 * seeding a software noise generator from a register they never wrote
 * need this after loading. */
int32_t spinfv1_randomize_registers(SpinFv1 *handle, uint32_t seed);

/* Model the ADC noise floor (enabled != 0): a deterministic dither of up
 * to a few hundred counts added to each input sample. Programs that
 * expand the input's low bits as a noise source need this against
 * digitally clean hosts. Off by default. */
int32_t spinfv1_set_adc_noise(SpinFv1 *handle, int32_t enabled, uint32_t seed);

/* Process one stereo frame (inputs nominally in -1..1). Real-time
 * safe. On error the outputs are written as silence. */
int32_t spinfv1_process(SpinFv1 *handle, float in_l, float in_r,
                        float *out_l, float *out_r);

/* Process frames stereo frames from/to separate channel buffers. */
int32_t spinfv1_process_block(SpinFv1 *handle, const float *in_l,
                              const float *in_r, float *out_l, float *out_r,
                              size_t frames);

/* Reset DSP state (registers, delay RAM, LFOs, accumulator). */
int32_t spinfv1_reset(SpinFv1 *handle);

/* Pipeline latency in host samples (0 in crystal-swap mode). */
uint32_t spinfv1_latency(const SpinFv1 *handle);

/* The chip's native sample rate, 32768.0 Hz. */
double spinfv1_native_rate(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SPINFV1_H */
