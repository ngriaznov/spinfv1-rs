//! Shared helpers for integration tests.
//!
//! Each test binary compiles this module separately and uses a different
//! subset of the helpers, so unused-function warnings are expected noise.
#![allow(dead_code)]

use spinfv1::{Fv1, Instruction, Program};

/// Build a VM with the given instructions loaded (padded with NOP).
///
/// Delay quantization is disabled so datapath tests compare against
/// exact fixed-point expectations; the chip-faithful default is
/// exercised by the golden-vector and WAV suites.
pub fn vm(instructions: &[Instruction]) -> Fv1 {
    let program = Program::from_instructions(instructions).expect("test program fits in 128 slots");
    let mut fv1 = Fv1::new();
    fv1.set_delay_quantization(false);
    fv1.load_program(&program);
    fv1
}

/// Run `samples` mono samples (left channel), returning left outputs.
pub fn run_mono(fv1: &mut Fv1, input: impl IntoIterator<Item = f32>) -> Vec<f32> {
    input.into_iter().map(|x| fv1.process(x, 0.0).0).collect()
}

/// Count positive-going zero crossings, ignoring exact zeros.
pub fn positive_crossings(signal: &[f32]) -> usize {
    signal
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count()
}

/// Mean frequency in cycles/sample from positive-going zero crossings,
/// measured between the first and last crossing.
pub fn crossing_frequency(signal: &[f32]) -> f64 {
    let crossings: Vec<usize> = signal
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, _)| i)
        .collect();
    assert!(crossings.len() >= 2, "need at least two zero crossings");
    let cycles = crossings.len() - 1;
    let span = crossings[crossings.len() - 1] - crossings[0];
    cycles as f64 / span as f64
}

/// Deterministic stereo test signal, pure integer math so every platform
/// generates identical raw samples: an impulse, a triangle sweep, DC
/// steps, an LCG noise burst, and a silent tail for decays. Shared by
/// the golden-hash and reference-WAV regression suites — the committed
/// reference captures were produced from exactly this sequence.
pub fn test_signal(frames: usize) -> Vec<(i32, i32)> {
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

/// Micro-programs, one per datapath family, written for this suite.
pub const MICRO: &[(&str, &str)] = &[
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

/// Minimal reader for the exact WAV shape we commit: RIFF/WAVE, PCM
/// `fmt ` chunk (24-bit stereo), then a `data` chunk of little-endian
/// signed 24-bit frames, returned as raw S.23 (l, r) pairs.
pub fn read_wav_s23(path: &std::path::Path) -> Vec<(i32, i32)> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(&b[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&b[8..12], b"WAVE", "not a WAVE file");
    let mut pos = 12;
    let mut fmt_ok = false;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..pos + 8 + len];
        match id {
            b"fmt " => {
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                assert_eq!((channels, bits), (2, 24), "expected 24-bit stereo PCM");
                fmt_ok = true;
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    assert!(fmt_ok, "missing fmt chunk");
    let data = data.expect("missing data chunk");
    data.chunks_exact(6)
        .map(|f| {
            let s24 = |c: &[u8]| (i32::from_le_bytes([0, c[0], c[1], c[2]])) >> 8;
            (s24(&f[0..3]), s24(&f[3..6]))
        })
        .collect()
}

/// Writer mirroring `read_wav_s23`: 24-bit stereo PCM at the chip rate.
pub fn write_wav_s23(path: &std::path::Path, frames: &[(i32, i32)]) {
    let mut data = Vec::with_capacity(frames.len() * 6);
    for &(l, r) in frames {
        data.extend_from_slice(&l.to_le_bytes()[0..3]);
        data.extend_from_slice(&r.to_le_bytes()[0..3]);
    }
    let mut b = Vec::with_capacity(44 + data.len());
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&2u16.to_le_bytes()); // stereo
    b.extend_from_slice(&32_768u32.to_le_bytes()); // chip rate
    b.extend_from_slice(&(32_768u32 * 6).to_le_bytes());
    b.extend_from_slice(&6u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(path, b).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

pub fn compare_frames(stem: &str, got: &[(i32, i32)], expected: &[(i32, i32)]) {
    assert_eq!(got.len(), expected.len(), "{stem}: frame count");
    for (n, (g, e)) in got.iter().zip(expected).enumerate() {
        assert_eq!(g, e, "{stem}: sample {n} diverges from the committed WAV");
    }
}
