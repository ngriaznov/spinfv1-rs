# Roadmap

Future work for `spinfv1`. Nothing below is committed work — entries
record design intent so decisions aren't re-litigated later. Current
capabilities and test coverage are documented in the README.

## 1. Sample-rate options

- **Resampler module**: promote the example resampler
  (`examples/support`) into an optional crate module (e.g. a `resampler`
  feature) with a proper windowed-sinc polyphase kernel, so hosts get
  boundary resampling around the 32,768 Hz chip rate turnkey instead of
  hand-rolling it.
- **"Crystal swap" mode**: document running the VM directly at the host
  rate as a supported mode (per the datasheet, the sample rate follows
  the applied clock), and expose it in the VCV module as a continuous
  "clock" control — smooth crystal-swap is something real hardware
  can't do.
- **Time-compensated native-rate mode — rejected.** Rescaling time
  constants at the host rate can only ever be approximate: one-pole
  filter coefficients (`RDFX`/`WRLX`) are baked into program words as
  per-sample constants and cannot be rescaled without changing what the
  program computes, and interpolated delay reads add coloration inside
  reverb feedback loops. Recorded so the idea isn't re-explored without
  new information.

## 2. Hardware validation

Record a real FV-1 (any pedal with pot access) playing the corpus dry
phrase through the factory programs, and diff against
`examples/render_corpus.rs` output. The emulator's renders are
deterministic, so this pins down the areas Spin's documents leave open:
the exact LOG/EXP approximation shape and the mantissa/exponent split
of the 14-bit compressed floating-point delay word (the datasheet
documents the format's existence but not its layout).

## 3. Emulation-fidelity options

- Decide whether the 14-bit compressed-float delay model
  (`set_delay_quantization`) should become the default once hardware
  comparisons pin down the word's actual mantissa/exponent split.
- Investigate the chip's actual LOG/EXP approximation (currently
  double-precision math, at least as accurate as silicon but not proven
  bit-identical).
- Decide on pot quantization: independent implementations model the pot
  inputs as 10-bit values (floor to 1024 steps); we keep full S.23 pot
  resolution for now. Revisit alongside hardware validation, since pot
  smoothing/quantization audibly interacts with programs that derive
  LFO rates from pots.

## 4. Assembler policy

When a third-party program surfaces a construct the assembler rejects,
close the gap together with that program as a regression test, as was
done for `-kap`, `1/64` and `sym < 8`.

## 5. Hosting

- VCV Rack module — separate repository; consumes this crate unchanged.
  Uses boundary resampling by default, crystal-swap as a "clock"
  control.
- Publish to crates.io once the API has settled (after the VCV module
  proves it out).
