# Roadmap

Planned work for `spinfv1`. The emulator core (full instruction set,
bit-exact S.23 ALU, SpinASM assembler, `no_std` support, benchmarks, and
the factory-program listening corpus) is complete and tested; items here
are future extensions. Nothing below is committed work — entries record
design intent so decisions aren't re-litigated later.

## 1. Sample-rate options

The FV-1 has no fixed sample rate in silicon: it processes one sample per
external crystal tick, with 32,768 Hz being the nominal crystal. The
emulator inherits this — `process()` is rate-agnostic — so three distinct
modes fall out, two of which already work today:

- **A. Boundary resampling (chip stays at 32,768 Hz)** — the host runs at
  48 kHz (or anything) and resamples to chip rate and back around the VM.
  Programs sound exactly like hardware: delay times, LFO rates and filter
  cutoffs all authentic. This is what `examples/support` does and what a
  plugin wrapper should default to.
  - *Planned*: promote the example resampler into an optional crate module
    (e.g. a `resampler` feature) with a proper windowed-sinc polyphase
    kernel, so hosts get this turnkey instead of hand-rolling it.
- **B. "Crystal swap" mode (run the VM at the host rate)** — zero cost,
  already works: call `process()` at any rate. Everything time-based
  scales (delays shorten by `32768/fs`, LFOs and decays speed up). This
  is officially supported chip behavior, not a mod: the FV-1 datasheet
  states "any logic level clock source can be attached ... the sample
  rate of the system will be at this applied rate" and headlines
  operation at Fs=48KHz.
  - *Planned*: document this as a supported mode, and expose it in the
    VCV module as a continuous "clock" control (smooth crystal-swap is
    something real hardware can't do).
- **C. Time-compensated native-rate mode — rejected.** Running at the
  host rate while rescaling time constants (LFO increments by `32768/fs`,
  delay addresses by `fs/32768` with interpolated reads) can only ever be
  approximate: one-pole filter coefficients (`RDFX`/`WRLX`) are baked
  into program words as per-sample constants and cannot be rescaled
  without changing what the program computes, and interpolated delay
  reads add coloration inside reverb feedback loops. Recorded here so the
  idea isn't re-explored without new information.

## 2. Hardware validation

Record a real FV-1 (any pedal with pot access) playing the corpus dry
phrase through the factory programs, and diff against
`examples/render_corpus.rs` output. The emulator's renders are
deterministic, so this pins down the remaining folklore areas: exact
LOG/EXP approximation shape, SIN LFO interpolation fraction width, and
delay-RAM quantization behavior.

## 3. Emulation-fidelity options

- Decide whether 14-bit delay quantization (`set_delay_quantization`)
  should become the default once hardware comparisons exist.
- Investigate the chip's actual LOG/EXP approximation (currently
  double-precision math, at least as accurate as silicon but not proven
  bit-identical).

## 4. Assembler completeness

The assembler handles every factory program in the corpus, and the
expression grammar is complete: parentheses, unary `-`/`~`, `**`, `*` `/`,
`+` `-`, shifts, `&`, `^` (XOR, whitespace-separated; `buf^` stays the
MEM midpoint suffix), `|`, and exponent-notation reals. Remaining policy
rather than known gaps: when a third-party program surfaces a construct
we reject, close it together with that program as a regression test, as
was done for `-kap`, `1/64` and `sym < 8`.

## 5. Hosting

- VCV Rack module — separate repository; consumes this crate unchanged.
  Uses sample-rate option A by default, option B as a "clock" control.
- Publish to crates.io once the API has settled (after the VCV module
  proves it out).
