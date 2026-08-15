//! Golden-vector regression: whole-program bit-exactness.
//!
//! Each case streams a deterministic, integer-only test signal through a
//! program and asserts an FNV-1a hash of every raw output sample. The
//! hashes freeze behavior that has been cross-validated instruction by
//! instruction against the chip documentation and independent
//! implementations — any change to the VM's arithmetic, the LFO
//! datapaths, the delay layout, or the assembler's address allocation
//! shows up here as a hash mismatch, even if every targeted unit test
//! still passes.
//!
//! If a deliberate behavior change lands (with documentation for why),
//! regenerate the constants with:
//! `SPINFV1_PRINT_HASHES=1 cargo test --test golden -- --nocapture`

// The assembler is `std`-only, so this file is compiled out under
// `--no-default-features --features libm`.
#![cfg(feature = "std")]

mod common;

use common::{MICRO, test_signal};
use spinfv1::{Fv1, assemble};

fn fnv1a(hash: &mut u64, v: i32) {
    for b in v.to_le_bytes() {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

fn run_hash(source: &str, pots: [f32; 3], frames: usize) -> u64 {
    let program = assemble(source).expect("golden program must assemble");
    let mut fv1 = Fv1::new();
    fv1.load_program(&program);
    for (i, p) in pots.iter().enumerate() {
        fv1.set_pot(i, *p);
    }
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for (l, r) in test_signal(frames) {
        let (ol, or) = fv1.process_raw(l, r);
        fnv1a(&mut hash, ol);
        fnv1a(&mut hash, or);
    }
    hash
}

const FRAMES: usize = 32_768;
const POTS: [f32; 3] = [0.5, 0.25, 1.0];

/// (stem, expected hash) — regenerate via SPINFV1_PRINT_HASHES=1.
const MICRO_HASHES: &[(&str, u64)] = &[
    ("gain_sof", 0xF8AA93EAB84ADECE),
    ("register_filters", 0x5241A7D77DD1EB8E),
    ("log_exp", 0x3FB57C6DB2F873D2),
    ("bitops_skp", 0xD452947D0885CD79),
    ("delay_feedback", 0xA3FA97E4C0203EC5),
    ("addr_ptr", 0xF851EFC298546FBC),
    ("sin_chorus", 0xFD9871871FBE3F08),
    ("rmp_pitch", 0x488A46F9D9D02346),
    ("cho_sof_live_rate", 0xDAFA2C37F0208609),
];

const CORPUS_HASHES: &[(&str, u64)] = &[
    ("GA_DEMO_CHORUS", 0xAEB560EADC467A88),
    ("GA_DEMO_FLANGE", 0xD05145A17B0D3C79),
    ("GA_DEMO_PHASE", 0xA7BB5E686A0F0521),
    ("GA_DEMO_TREM", 0x707A45D6CB7DA099),
    ("octave_down", 0x284427521F59E9DD),
    ("rom_chor_rev", 0x3010EA5EE3C9EAD1),
    ("rom_fla_rev", 0xE7BB3CC40F68D1A5),
    ("rom_pitch", 0xCB5648095DF5C4A0),
    ("rom_pt_echo", 0xBF758A2015F62F55),
    ("rom_rev1", 0x035A547A7A7E6F64),
    ("rom_rev2", 0xAE1314567F67BFA8),
    ("rom_trem_rev", 0x8EF678B3FB0B32B1),
];

fn print_mode() -> bool {
    std::env::var_os("SPINFV1_PRINT_HASHES").is_some()
}

#[test]
fn micro_programs_are_bit_stable() {
    let mut failures = Vec::new();
    for (stem, source) in MICRO {
        let hash = run_hash(source, POTS, FRAMES);
        if print_mode() {
            println!("    (\"{stem}\", 0x{hash:016X}),");
            continue;
        }
        let expected = MICRO_HASHES
            .iter()
            .find(|(s, _)| s == stem)
            .expect("hash entry")
            .1;
        if hash != expected {
            failures.push(format!(
                "{stem}: got 0x{hash:016X}, expected 0x{expected:016X}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatch:\n{}",
        failures.join("\n")
    );
}

#[test]
fn corpus_programs_are_bit_stable() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/programs");
    let mut failures = Vec::new();
    for (stem, expected) in CORPUS_HASHES {
        let source = std::fs::read_to_string(dir.join(format!("{stem}.spn")))
            .unwrap_or_else(|e| panic!("{stem}: {e}"));
        let hash = run_hash(&source, POTS, FRAMES);
        if print_mode() {
            println!("    (\"{stem}\", 0x{hash:016X}),");
            continue;
        }
        if hash != *expected {
            failures.push(format!(
                "{stem}: got 0x{hash:016X}, expected 0x{expected:016X}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatch:\n{}",
        failures.join("\n")
    );
}
