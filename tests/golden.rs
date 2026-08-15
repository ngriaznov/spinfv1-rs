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

use spinfv1::{Fv1, assemble};

/// Deterministic stereo test signal, pure integer math so every platform
/// generates identical raw samples: an impulse, DC steps, an LCG noise
/// burst, and a silent tail for decays.
fn test_signal(frames: usize) -> Vec<(i32, i32)> {
    let mut state: u32 = 0x1234_5678;
    let mut lcg = move || {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345) & 0x7FFF_FFFF;
        state
    };
    (0..frames)
        .map(|n| {
            let l: i32 = match n {
                0 => 0x40_0000,
                1..=999 => 0,
                1000..=8999 => {
                    // Integer triangle sweep standing in for a tone.
                    let p = ((n - 1000) % 149) as i32;
                    (p - 74) * 40_000
                }
                9000..=10499 => 0x20_0000,
                10500..=11999 => -0x20_0000,
                12000..=29999 => (lcg() % 5_033_165) as i32 - 2_516_582,
                _ => 0,
            };
            let r = if (20_000..30_000).contains(&n) {
                -l
            } else {
                l / 2
            };
            (l, r)
        })
        .collect()
}

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

/// Micro-programs, one per datapath family, written for this suite.
const MICRO: &[(&str, &str)] = &[
    (
        "gain_sof",
        "ldax ADCL\nsof 0.75, 0.1\nwrax DACL, 0\nldax ADCR\nsof -1.0, -0.001\nsof 1.99993896484375, 0\nwrax DACR, 0\n",
    ),
    (
        "register_filters",
        "ldax ADCL\nrdfx REG0, 0.15\nwrlx REG0, -0.4375\nwrax REG10, 1.0\nldax ADCR\nrdfx REG1, 0.02\nwrhx REG1, -0.75\nmulx POT2\nmaxx REG10, 0.5\nwrax DACL, 0.5\nwrax DACR, 0\n",
    ),
    (
        "log_exp",
        "ldax ADCL\nabsa\nlog -0.5, -1.0\nexp 0.8, 0.2\nwrax DACL, 0\nldax ADCR\nabsa\nlog 1.0, 0.5\nwrax DACR, 0\n",
    ),
    (
        "bitops_skp",
        "ldax ADCL\nand $F00000\nor $000FF0\nxor $0F000F\nwrax REG5, 1.0\nskp GEZ, 2\nsof 0, 0.25\nskp 0, 1\nsof 0, -0.25\nwrax DACL, 0\nldax ADCR\nskp ZRC, 1\nabsa\nwrax DACR, 0\n",
    ),
    (
        "delay_feedback",
        "del MEM 2000\ntap MEM 1000\nldax ADCL\nrda del#, 0.5\nwra del, 0.25\nrda tap^, 1.0\nwrap tap, 0.4\nwrax DACL, 0\nrmpa 0.9\nwrax DACR, 0\n",
    ),
    (
        "addr_ptr",
        "buf MEM 4096\nldax ADCL\nwra buf, 0\nldax ADCR\nand $7FFF00\nwrax ADDR_PTR, 0\nrmpa 1.0\nwrax DACL, 0\nldax ADDR_PTR\nwrax DACR, 0\n",
    ),
    (
        "sin_chorus",
        "buf MEM 8193\nskp RUN, 2\nwlds SIN0, 40, 16384\nwlds SIN1, 200, 320\nldax ADCL\nwra buf, 0\ncho rda, SIN0, SIN|REG|COMPC, buf^\ncho rda, SIN0, SIN, buf^+1\nwrax DACL, 0\ncho rdal, SIN1\nwrax DACR, 0\n",
    ),
    (
        "rmp_pitch",
        "buf MEM 4096\ntmp MEM 1\nskp RUN, 1\nwldr RMP0, -8192, 4096\nldax ADCL\nwra buf, 0\ncho rda, RMP0, REG|COMPC, buf\ncho rda, RMP0, 0, buf+1\nwra tmp, 0\ncho rda, RMP0, RPTR2|COMPC, buf\ncho rda, RMP0, RPTR2, buf+1\ncho sof, RMP0, NA|COMPC, 0\ncho rda, RMP0, NA, tmp\nwrax DACL, 0\ncho rdal, RMP0\nwrax DACR, 0\n",
    ),
    (
        "cho_sof_live_rate",
        "skp RUN, 1\nwlds SIN0, 100, 32767\nldax POT0\nsof 0.05, 0.002\nwrax SIN0_RATE, 0\nldax ADCL\ncho sof, SIN0, SIN|REG, 0\nwrax DACL, 0\ncho rdal, SIN0\nwrax DACR, 0\n",
    ),
];

/// (stem, expected hash) — regenerate via SPINFV1_PRINT_HASHES=1.
const MICRO_HASHES: &[(&str, u64)] = &[
    ("gain_sof", 0xF8AA93EAB84ADECE),
    ("register_filters", 0x5241A7D77DD1EB8E),
    ("log_exp", 0x3FB57C6DB2F873D2),
    ("bitops_skp", 0xD452947D0885CD79),
    ("delay_feedback", 0xA3FA97E4C0203EC5),
    ("addr_ptr", 0xF851EFC298546FBC),
    ("sin_chorus", 0x4525E6515028CC92),
    ("rmp_pitch", 0x488A46F9D9D02346),
    ("cho_sof_live_rate", 0x64B634BB9D359776),
];

const CORPUS_HASHES: &[(&str, u64)] = &[
    ("GA_DEMO_CHORUS", 0x8FCA34628A65C05B),
    ("GA_DEMO_FLANGE", 0x0A042329668FFE64),
    ("GA_DEMO_PHASE", 0x3FB9125D948A931D),
    ("GA_DEMO_TREM", 0x87A13ECD394D5DD8),
    ("octave_down", 0x284427521F59E9DD),
    ("rom_chor_rev", 0x26FC23DB26C79101),
    ("rom_fla_rev", 0xFE57FBD85C67A1CD),
    ("rom_pitch", 0xCB5648095DF5C4A0),
    ("rom_pt_echo", 0xBF758A2015F62F55),
    ("rom_rev1", 0x5E449C46B5C2EAF1),
    ("rom_rev2", 0x5F9A8A11F2518093),
    ("rom_trem_rev", 0x57C1C4C874B83791),
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
