# spinfv1

A [Spin Semiconductor FV-1](https://www.spinsemi.com/products.html) audio DSP
emulator in Rust: the complete 21-opcode instruction set on a bit-exact S.23
saturating fixed-point core, a built-in SpinASM assembler, and full test
coverage including audio end-to-end tests. A pure library with zero
dependencies in every configuration (`no_std` included) and a
one-sample-per-call API, ready to embed in any real-time host.

## What's emulated

- **All instructions**: `RDA` `RMPA` `WRA` `WRAP` `RDAX` `RDFX` `WRAX` `WRHX`
  `WRLX` `MAXX` `MULX` `LOG` `EXP` `SOF` `AND` `OR` `XOR` `SKP` `WLDS` `WLDR`
  `JAM` `CHO RDA` `CHO SOF` `CHO RDAL`, plus the pseudo-ops (`NOP` `CLR` `NOT`
  `ABSA` `LDAX`). Decoding is total: any 32-bit word executes (undefined
  opcodes are no-ops), so arbitrary ROM images can't crash the VM.
- **The ALU as hardware does it**: 24-bit S.23 accumulator with saturation at
  every architectural write, truncating fixed-point multiplies (S1.14, S1.9,
  S.10, S4.6, S.15 coefficient formats), bitwise ops with bit-23 sign
  extension, and the one-instruction **PACC pipeline delay** that `WRHX`,
  `WRLX` and `SKP ZRC` depend on.
- **Delay RAM**: 32768 samples with the decrementing base pointer, `LR`
  last-read register, and `ADDR_PTR` indirect addressing for `RMPA`.
  Full 24-bit storage by default; optionally model the chip's 14-bit
  compressed floating-point RAM word (`Fv1::set_delay_quantization`).
- **LFOs**: both SIN LFOs (magic-circle oscillator, `rate/2^17` rad/sample)
  and both RMP LFOs (22-bit phase, `rate/16` per sample), with live
  register-driven rate/range (pot-controlled modulation works), `JAM`,
  and the full `CHO` flag set — `COS`, `REG`, `COMPC`, `COMPA`, `RPTR2`,
  `NA` — with the 10-bit interpolation fraction below the sample offset
  (the same datapath split for SIN and RMP). SIN address excursion is
  `amp / 4` samples, per AN-0001's amplitude formula
  `Ka = N * 32767 / 16385` and its worked chorus example
  (`Ka = 16384` sweeping ±4096 samples).
- **I/O**: stereo ADC/DAC registers, three pots, per-sample or block
  processing, and the 512-byte big-endian EEPROM/bank program format.

The chip's native rate is 32,768 Hz (`spinfv1::SAMPLE_RATE`); the
emulator itself is one-sample-per-call and rate-agnostic. Two ways to
host it at another rate, both officially grounded in chip behavior:

- **Boundary resampling** (opt-in `resampler` feature): keep the chip at
  32,768 Hz and convert at the edges, so delays, LFO rates and filter
  cutoffs are exactly as on hardware. `resampler::HostedFv1` wraps an
  `Fv1` behind a streaming windowed-sinc polyphase converter — one call
  per host sample, reported fixed latency, real-time safe after
  construction.
- **"Crystal swap"**: just call `process()` at the host rate. Everything
  time-based scales, exactly as the datasheet describes for an applied
  external clock ("the sample rate of the system will be at this applied
  rate").

For C/C++ hosts (VCV Rack, JUCE, …) the opt-in `ffi` feature exposes the
same two modes behind a panic-safe C ABI (`include/spinfv1.h`): an opaque
handle with create/destroy, bank or SpinASM program loading with error
messages, pot control, per-sample and block processing, reset, and
latency reporting. Build the archive with
`cargo rustc --release --features ffi --crate-type staticlib` and link
`target/release/libspinfv1.a`; the header carries the threading and
real-time-safety contract.

The boundary's safety is tested at three levels: Rust ABI tests
(lifecycle, null rejection, error messages, exact parity with the
direct API, panic containment — a caught panic surfaces as an error
code and leaves the handle usable), a header↔implementation
consistency test (the exported symbol set and the header's
declarations must match exactly), and a C-side hostile-host harness
(`tests/c/run.sh`) that compiles against the real header and static
library and runs create/destroy churn, bad programs, per-frame pot
automation, mid-stream reprogramming, and heap-exact block buffers
under AddressSanitizer + UBSan + LeakSanitizer, and again under
valgrind.

## Usage

```rust
use spinfv1::{Fv1, Instruction, Program, coeff, reg};

// A 100-sample echo at half gain, built programmatically.
let program = Program::from_instructions(&[
    Instruction::ldax(reg::ADCL),
    Instruction::Wra { addr: 0, c: 0 },
    Instruction::Rda { addr: 100, c: coeff::s1_9(0.5) },
    Instruction::Wrax { reg: reg::DACL, c: 0 },
]).unwrap();

// Or load a standard 512-byte bank image:
// let program = Program::from_bytes(&eeprom_bytes)?;

let mut fv1 = Fv1::new();
fv1.load_program(&program);
fv1.set_pot(0, 0.5);

// Real-time loop: one stereo sample per call.
let (out_l, out_r) = fv1.process(0.25, 0.25);
```

## Assembler

`spinfv1::assemble` compiles SpinASM source text straight to a [`Program`],
so you can write and load effects without hand-encoding instructions:

```rust
use spinfv1::{Fv1, assemble};

let source = "
    ; 100-sample echo at half gain
    DELAY   MEM 200

            LDAX  ADCL
            WRA   DELAY, 0
            RDA   DELAY^, 0.5
            WRAX  DACL, 0
";
let program = assemble(source).unwrap();
let mut fv1 = Fv1::new();
fv1.load_program(&program);
```

It supports the SpinASM user manual subset needed for real programs:
case-insensitive mnemonics/registers/symbols, `;` comments, labels with
forward references (`SKP` distances are computed automatically), `EQU`
and `MEM` directives (`MEM` defines `NAME`/`NAME#`/`NAME^` for a buffer's
start/end/midpoint), decimal/hex (`$3F`, `0x3F`)/binary (`%0101_1100`)/real
number literals (including exponent notation, `1e-3`), and full constant
expressions with C-like precedence: parentheses, unary `-`/`~`, `**`
(power), `*` `/`, `+` `-`, `<`/`<<` `>`/`>>` shifts, `&`, `^` (XOR — when
separated by whitespace; glued to an identifier, `buf^` is the `MEM`
midpoint suffix), and `|` flag combining (`SKP RUN|ZRC, ...`,
`CHO RDA, SIN0, COMPC|REG, ...`). Coefficient operands
are parsed as real numbers and quantized with round-to-nearest, accepting
SpinASM's conventional shorthand (`2.0`, `1.0`, `16.0`) for a format's
largest representable value. Errors ([`AsmError`]) carry the offending
1-based line number and, where available, the source line's text.

## `no_std`

The DSP core (`Fv1`, `Program`, `Instruction`, the `fixed` module) builds
`no_std`, for embedding on bare-metal/RTOS targets that run the effect but
have no operating system:

```sh
cargo build --no-default-features --features libm
```

Two build configurations select the environment, not the math — the
floating-point `log2`/`exp2`/`round` behind `LOG`/`EXP` are implemented
in-crate from nothing but IEEE-754 arithmetic and integer bit
manipulation, so every platform and configuration computes bit-identical
results (no platform libm is involved anywhere):

- `std` (default): enables the assembler, the `resampler`/`ffi`
  features, and the file-based test suites.
- `libm`: marker feature for `no_std` builds
  (`--no-default-features --features libm`), exposing the VM and program
  types for pre-assembled programs (`Program::from_words`,
  `Program::from_bytes`, or `Instruction`s built by hand). The name is
  historical — no libm crate is pulled in.

Exactly one of the two must be enabled; enabling neither is a compile
error. Both configurations still require a global allocator
(`extern crate alloc`), for the 32K-word delay RAM.

## Testing

`cargo test --features ffi` runs about 160 tests in nine layers
(plus the C-side sanitizer battery, `tests/c/run.sh`):

1. **Spec conformance** (`tests/spec_conformance.rs`): a deliberate
   per-instruction matrix. Every `Instruction` variant has a case that
   quotes the SpinASM user manual's formula and checks the emulator
   against an independent in-test reference implementation of the spec
   arithmetic (never the crate's own helpers). Completeness is enforced
   structurally: an exhaustive `match` makes a new instruction variant a
   compile error until it is named, and the matrix fails until the new
   variant has a spec case.
2. **Unit** (`src/fixed.rs`, `src/asm.rs`): saturation, sign extension,
   quantizers, and assembler tokenizing/expression evaluation.
3. **Codec** (`tests/codec.rs`): golden instruction words checked against
   SpinASM encodings, exhaustive field sweeps, and a 2-million-word
   fuzz proving decode/encode is total and canonical-lossless.
4. **Semantics** (`tests/alu.rs`, `tests/delay.rs`, `tests/skp.rs`): every
   instruction verified against independently computed fixed-point
   expectations — including PACC pipeline timing, LR, saturation rails,
   truncation direction, and all skip conditions.
5. **LFO** (`tests/lfo.rs`): measured oscillator frequency vs. the datasheet
   formula, amplitude scaling, quadrature, ramp periods and direction,
   crossfade envelope, RPTR2 tap placement, interpolation-pair unity.
6. **Audio end-to-end** (`tests/audio_e2e.rs`): complete effect programs
   verified by signal analysis — sample-exact echoes, a one-pole filter
   matched against a double-precision reference, chorus whose measured
   delay modulation tracks the LFO in depth *and* period, and the classic
   dual-tap pitch shifter verified to shift 440 Hz by exact ratios
   (0.5×, 1.3×, 2×), plus a random-program robustness fuzz.
7. **Assembler** (`tests/asm.rs`): golden SpinASM programs (echo, feedback
   delay, one-pole filter, chorus, pitch shifter) checked word-for-word
   against the equivalent hand-built `Instruction`s, every instruction and
   pseudo-op, `MEM`/`EQU`/label semantics, coefficient range edges
   including the shorthand rule, every documented error case pinned to its
   line number, and an assembled echo program run through `Fv1` for a
   sample-exact result.
8. **Golden vectors** (`tests/golden.rs`, `tests/log_exp_values.rs`):
   whole-program bit-exactness. A deterministic integer-only test signal
   streams through datapath micro-programs and the full factory corpus;
   an FNV-1a hash of every raw output sample is asserted, freezing
   behavior that was cross-validated instruction by instruction against
   independent FV-1 implementations (bit-identical on every
   shared-datapath probe: ALU/SOF, register filters, bit ops, SKP/PACC,
   delay/WRAP/LR, ADDR_PTR/RMPA, ramp phase generation). `LOG`/`EXP`
   are additionally pinned to an 84-point exact-value table.
9. **WAV captures** (`tests/reference_wav.rs`, `tests/data/`): committed
   24-bit WAVs at the chip rate, bit-compared sample for sample with no
   tolerance. Two tiers: datapath micro-programs captured from an
   independent FV-1 virtual machine (external cross-check), and the
   full standard Spin factory corpus (`tests/data/corpus/`) captured
   from this emulator after cross-validation — inspectable, listenable
   artifacts freezing whole-program behavior.

## Benchmarks

`cargo bench` runs a dependency-free throughput benchmark
(`benches/throughput.rs`, hand-rolled with `std::time::Instant`, no
`criterion`) over six representative programs: the all-`NOP` dispatch floor,
a stereo passthrough, a pot-controlled feedback echo, a sine-LFO chorus
using the `CHO RDA` interpolation pair, the classic dual-tap `RMP`
pitch shifter, and a dense 128-instruction program that exercises every
opcode at least once. Each processes ~2,000,000 samples of a deterministic
pseudo-random stereo signal (best of 3 runs) and reports samples/sec, the
ratio against the chip's native 32,768 Hz rate, and a checksum of the raw
output stream so runs stay comparable across changes.

```sh
cargo bench
```

Sample results from an actual run on this machine (4-core Intel Xeon @
2.10GHz, release build):

```
program                             samples/sec   vs real-time             checksum
-----------------------------------------------------------------------------------
all-NOP (dispatch floor)                4154470           127x                    0
passthrough                             4116755           126x     8583606845757203
feedback echo                           4005817           122x     4289439265781251
sine-LFO chorus (CHO RDA pair)          4029811           123x     4289185750015631
dual-tap RMP pitch shifter              3697425           113x     4288279044024075
dense 128-op (every opcode)             2573797            79x     4291750378700635
```

Even the densest 128-instruction program that touches every opcode runs at
close to 80× the chip's real-time rate, and light effects sit well above
100×, leaving headroom for oversampling or many simultaneous instances.

## Fidelity notes & divergences

- `WLDS`/`WLDR` load the LFO registers but do **not** reset oscillator
  phase (matching observed hardware behavior where pots can retune a
  running LFO); `JAM` and program load reset phase.
- The `CHO ... REG` latch flag is a no-op: LFOs advance between passes, so
  their value is already stable within a pass.
- The `CHO ... NA` cross-fade is the clamped trapezoid
  `clamp(4 * min(p, 1 - p) - 0.5)`: flat 0 across the ramp wrap, slope-4
  ramps, flat 1.0 through the middle. Hardware observations reported for
  this envelope show the flat-topped shape, and the flat zero is what
  lets AN-0001's pitch shifter mute a tap for the whole wrap glitch.
- `LOG`/`EXP` use double-precision math rounded to S.23 — at least as
  accurate as the silicon's piecewise approximation, and computed by
  in-crate deterministic arithmetic so results are bit-identical on
  every platform. Their behavior is pinned bit-for-bit by an
  exact-value table (`tests/log_exp_values.rs`) cross-validated against
  an independent implementation.
- Delay RAM is 24-bit by default vs. the chip's 14-bit compressed
  floating-point word; opt into the compressed-float model for an
  authentic noise floor (the exact mantissa/exponent split is
  undocumented, so the model keeps 10 significant bits across a
  level-scaled quantization step).

## References

This is an original, from-scratch implementation based on Spin
Semiconductor's public documentation: the FV-1 datasheet, the SpinASM
user manual, and *AN-0001: Basics of the LFOs in the FV-1*.

## Roadmap

Future work — the VCV Rack module, remaining emulation-fidelity
decisions, and crates.io publication — is documented in
[ROADMAP.md](ROADMAP.md).

## License

Source-available (see [LICENSE](LICENSE)): free to use, modify and
distribute **within open-source projects and for noncommercial purposes,
with attribution** to this repository. **Commercial use requires prior
written permission** from the author. The Spin Semiconductor factory
programs under `examples/programs/` are third-party material included
for interoperability testing and remain under their original copyright.
