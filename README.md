# spinfv1

A [Spin Semiconductor FV-1](http://www.spinsemi.com/products.html) audio DSP
emulator in Rust: the complete 21-opcode instruction set on a bit-exact S.23
saturating fixed-point core, with full test coverage including audio
end-to-end tests. A pure library with zero dependencies and a
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
  Full 24-bit storage by default; optional 14-bit truncation to mimic the
  chip's physical RAM width (`Fv1::set_delay_quantization`).
- **LFOs**: both SIN LFOs (magic-circle oscillator, `rate/2^17` rad/sample)
  and both RMP LFOs (22-bit phase, `rate/16` per sample), with live
  register-driven rate/range (pot-controlled modulation works), `JAM`,
  and the full `CHO` flag set — `COS`, `REG`, `COMPC`, `COMPA`, `RPTR2`,
  `NA` — with the 8-bit (SIN) / 10-bit (RMP) interpolation fractions,
  matching LFO behavior that has been validated against real chips.
- **I/O**: stereo ADC/DAC registers, three pots, per-sample or block
  processing, and the 512-byte big-endian EEPROM/bank program format.

Zero runtime dependencies. The chip's native rate is 32,768 Hz
(`spinfv1::SAMPLE_RATE`); the emulator itself is one-sample-per-call and
rate-agnostic.

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

## Testing

`cargo test` runs 75 tests in five layers:

1. **Unit** (`src/fixed.rs`): saturation, sign extension, quantizers.
2. **Codec** (`tests/codec.rs`): golden instruction words checked against
   SpinASM encodings, exhaustive field sweeps, and a 2-million-word
   fuzz proving decode/encode is total and canonical-lossless.
3. **Semantics** (`tests/alu.rs`, `tests/delay.rs`, `tests/skp.rs`): every
   instruction verified against independently computed fixed-point
   expectations — including PACC pipeline timing, LR, saturation rails,
   truncation direction, and all skip conditions.
4. **LFO** (`tests/lfo.rs`): measured oscillator frequency vs. the datasheet
   formula, amplitude scaling, quadrature, ramp periods and direction,
   crossfade triangle, RPTR2 tap placement, interpolation-pair unity.
5. **Audio end-to-end** (`tests/audio_e2e.rs`): complete effect programs
   verified by signal analysis — sample-exact echoes, a one-pole filter
   matched against a double-precision reference, chorus whose measured
   delay modulation tracks the LFO in depth *and* period, and the classic
   dual-tap pitch shifter verified to shift 440 Hz by exact ratios
   (0.5×, 1.3×, 2×), plus a random-program robustness fuzz.

## Fidelity notes & divergences

- `WLDS`/`WLDR` load the LFO registers but do **not** reset oscillator
  phase (matching observed hardware behavior where pots can retune a
  running LFO); `JAM` and program load reset phase.
- The `CHO ... REG` latch flag is a no-op: LFOs advance between passes, so
  their value is already stable within a pass.
- `LOG`/`EXP` use double-precision math rounded to S.23 — at least as
  accurate as the silicon's piecewise approximation.
- Delay RAM is 24-bit by default vs. the chip's 14-bit; opt into 14-bit
  truncation for authentic noise floor.

## References

This is an original, from-scratch implementation based on Spin
Semiconductor's public documentation: the FV-1 datasheet, the Spin ASM
users manual, and *AN-0001: Basics of the LFOs in the FV-1*.

## License

MIT
