# Roadmap

Future work for `spinfv1`. Nothing below is committed work — entries
record design intent so decisions aren't re-litigated later. Current
capabilities and test coverage are documented in the README.

## 1. Sample-rate options

- **"Crystal swap" as a control**: expose running the VM directly at a
  chosen rate as a continuous "clock" parameter in a host — smooth
  crystal-swap is something real hardware can't do.
- **Time-compensated native-rate mode — rejected.** Rescaling time
  constants at the host rate can only ever be approximate: one-pole
  filter coefficients (`RDFX`/`WRLX`) are baked into program words as
  per-sample constants and cannot be rescaled without changing what the
  program computes, and interpolated delay reads add coloration inside
  reverb feedback loops. Recorded so the idea isn't re-explored without
  new information.

## 2. Emulation-fidelity options

- Decide whether the 14-bit compressed-float delay model
  (`set_delay_quantization`) should become the default (the datasheet
  documents the compressed format's existence but not its
  mantissa/exponent split; ours is a documented model).
- Decide on pot quantization: independent implementations model the pot
  inputs as 10-bit values (floor to 1024 steps, matching the SpinASM
  user manual's "approximately a 10-bit resolution"); we keep full S.23 pot
  resolution for now, which also suits smooth virtual knobs.

## 3. Publication

- Publish to crates.io once the API has settled.
