/* Hostile-host safety harness for the spinfv1 C ABI.
 *
 * Exercises the boundary the way a buggy or aggressive C++ host might:
 * create/destroy churn, repeated program loads (good and bad), pot
 * automation every frame, mid-stream reprogramming, both hosting
 * modes, heap-backed block buffers, and error-path probing — so that
 * AddressSanitizer/UBSan/LeakSanitizer or valgrind (see run.sh) can
 * observe any invalid access, undefined behavior, or leak.
 *
 * Exits 0 on success; any contract violation is a nonzero exit (and
 * memory errors abort under the sanitizers).
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "spinfv1.h"

#define CHECK(cond)                                                  \
    do {                                                             \
        if (!(cond)) {                                               \
            fprintf(stderr, "FAILED at %s:%d: %s\n", __FILE__,       \
                    __LINE__, #cond);                                \
            return 1;                                                \
        }                                                            \
    } while (0)

static const char *ECHO =
    "DELAY MEM 1000\n"
    "ldax ADCL\n"
    "wra DELAY, 0\n"
    "rda DELAY#, 0.5\n"
    "wrax DACL, 0\n"
    "ldax ADCR\n"
    "wrax DACR, 0\n";

static int lifecycle_churn(void) {
    for (int i = 0; i < 200; i++) {
        SpinFv1 *h = spinfv1_create((i % 2) ? 48000.0 : 0.0);
        CHECK(h != NULL);
        if (i % 3 == 0) {
            CHECK(spinfv1_load_asm(h, ECHO) == SPINFV1_OK);
        }
        float l, r;
        CHECK(spinfv1_process(h, 0.5f, -0.5f, &l, &r) == SPINFV1_OK);
        spinfv1_destroy(h);
    }
    spinfv1_destroy(NULL); /* explicit no-op */
    return 0;
}

static int error_paths(void) {
    SpinFv1 *h = spinfv1_create(0.0);
    CHECK(h != NULL);

    /* Bad source: error code plus a readable message. */
    CHECK(spinfv1_load_asm(h, "NOT_AN_OPCODE 1,2,3\n") == SPINFV1_ERR_PROGRAM);
    const char *msg = spinfv1_last_error(h);
    CHECK(msg != NULL && strlen(msg) > 0);

    /* Bank too short for the requested slot: rejected, not read OOB. */
    unsigned char short_bank[100];
    memset(short_bank, 0xAB, sizeof short_bank);
    CHECK(spinfv1_load_bank(h, short_bank, sizeof short_bank, 0) ==
          SPINFV1_ERR_PROGRAM);
    CHECK(spinfv1_load_bank(h, short_bank, sizeof short_bank, 7) ==
          SPINFV1_ERR_PROGRAM);

    /* Null misuse allowed by the contract: error codes, no crash. */
    float l, r;
    CHECK(spinfv1_process(NULL, 0, 0, &l, &r) == SPINFV1_ERR_NULL);
    CHECK(spinfv1_process(h, 0, 0, NULL, NULL) == SPINFV1_ERR_NULL);
    CHECK(spinfv1_load_asm(h, NULL) == SPINFV1_ERR_NULL);
    CHECK(spinfv1_load_bank(h, NULL, 512, 0) == SPINFV1_ERR_NULL);
    CHECK(spinfv1_last_error(NULL) == NULL);
    CHECK(spinfv1_latency(NULL) == 0);

    /* A failed load leaves the previous program running. */
    CHECK(spinfv1_load_asm(h, ECHO) == SPINFV1_OK);
    CHECK(spinfv1_load_asm(h, "garbage\n") == SPINFV1_ERR_PROGRAM);
    CHECK(spinfv1_process(h, 0.5f, 0.0f, &l, &r) == SPINFV1_OK);

    spinfv1_destroy(h);
    return 0;
}

static int audio_thread_simulation(void) {
    SpinFv1 *h = spinfv1_create(48000.0);
    CHECK(h != NULL);
    CHECK(spinfv1_load_asm(h, ECHO) == SPINFV1_OK);
    CHECK(spinfv1_latency(h) > 0);

    /* Per-frame pot automation plus periodic reprogramming, as a live
     * host would do while the user turns knobs and swaps patches. */
    double peak = 0.0;
    for (int i = 0; i < 96000; i++) {
        spinfv1_set_pot(0, 0, 0.0f); /* wrong-order call: null handle idx */
        CHECK(spinfv1_set_pot(h, (uint32_t)(i % 4), /* idx 3 ignored */
                              (float)(i % 1000) / 1000.0f) == SPINFV1_OK);
        float x = 0.5f * (float)sin(2.0 * M_PI * 440.0 * i / 48000.0);
        float l, r;
        CHECK(spinfv1_process(h, x, -x, &l, &r) == SPINFV1_OK);
        if (fabsf(l) > peak) peak = fabsf(l);
        if (i == 48000) {
            CHECK(spinfv1_reset(h) == SPINFV1_OK);
            CHECK(spinfv1_load_asm(h, ECHO) == SPINFV1_OK);
            CHECK(spinfv1_randomize_delay_ram(h, 0x12345u) == SPINFV1_OK);
            CHECK(spinfv1_randomize_registers(h, 0x12345u) == SPINFV1_OK);
            CHECK(spinfv1_set_adc_noise(h, 1, 0x12345u) == SPINFV1_OK);
            CHECK(spinfv1_set_clear_delay_on_load(h, 0) == SPINFV1_OK);
            CHECK(spinfv1_load_asm(h, ECHO) == SPINFV1_OK);
        }
    }
    CHECK(peak > 0.1 && peak < 1.01);
    spinfv1_destroy(h);
    return 0;
}

static int heap_block_buffers(void) {
    SpinFv1 *h = spinfv1_create(44100.0);
    CHECK(h != NULL);
    CHECK(spinfv1_load_asm(h, ECHO) == SPINFV1_OK);
    CHECK(spinfv1_set_delay_quantization(h, 1) == SPINFV1_OK);

    /* Exactly-sized heap buffers so ASan catches any off-by-one. */
    size_t frames = 4096;
    float *in_l = malloc(frames * sizeof *in_l);
    float *in_r = malloc(frames * sizeof *in_r);
    float *out_l = malloc(frames * sizeof *out_l);
    float *out_r = malloc(frames * sizeof *out_r);
    CHECK(in_l && in_r && out_l && out_r);
    for (size_t i = 0; i < frames; i++) {
        in_l[i] = (float)((int)(i % 200) - 100) / 100.0f;
        in_r[i] = -in_l[i];
    }
    for (int round = 0; round < 8; round++) {
        CHECK(spinfv1_process_block(h, in_l, in_r, out_l, out_r, frames) ==
              SPINFV1_OK);
    }
    CHECK(spinfv1_process_block(h, in_l, in_r, out_l, out_r, 0) == SPINFV1_OK);
    CHECK(spinfv1_process_block(h, NULL, in_r, out_l, out_r, frames) ==
          SPINFV1_ERR_NULL);
    free(in_l);
    free(in_r);
    free(out_l);
    free(out_r);
    spinfv1_destroy(h);
    return 0;
}

int main(void) {
    CHECK(fabs(spinfv1_native_rate() - 32768.0) < 1e-9);
    if (lifecycle_churn()) return 1;
    if (error_paths()) return 1;
    if (audio_thread_simulation()) return 1;
    if (heap_block_buffers()) return 1;
    printf("ffi_safety: all checks passed\n");
    return 0;
}
