//! Render audio through the emulator and write WAV files you can listen to.
//!
//! ```sh
//! cargo run --example render_wav [output_dir]
//! ```
//!
//! Generates a short melody of plucked tones, then renders it through four
//! FV-1 programs: a feedback echo, a sine-LFO chorus, and the classic
//! dual-tap pitch shifter an octave down and a fifth up.

use std::fs::File;
use std::io::{BufWriter, Result, Write};
use std::path::Path;

use spinfv1::{
    ChoFlags, Fv1, Instruction, LfoSel, Program, RampAmp, SAMPLE_RATE, SkpCond, coeff, reg,
};

/// Minimal mono 16-bit PCM WAV writer.
fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    let data_len = (samples.len() * 2) as u32;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // chunk size
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&1u16.to_le_bytes())?; // mono
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&(sample_rate * 2).to_le_bytes())?; // byte rate
    w.write_all(&2u16.to_le_bytes())?; // block align
    w.write_all(&16u16.to_le_bytes())?; // bits per sample
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

/// A little melody of plucked tones (decaying sines) as the dry signal.
fn melody() -> Vec<f32> {
    let sr = SAMPLE_RATE;
    let notes = [220.0, 277.18, 329.63, 440.0, 329.63, 277.18, 220.0, 0.0];
    let note_len = (sr * 0.4) as usize;
    let mut out = Vec::with_capacity(notes.len() * note_len);
    for &freq in &notes {
        for i in 0..note_len {
            let t = i as f64 / sr;
            let env = (-6.0 * t).exp();
            let s = if freq > 0.0 {
                0.5 * env * (core::f64::consts::TAU * freq * t).sin()
            } else {
                0.0
            };
            out.push(s as f32);
        }
    }
    out
}

fn feedback_echo() -> Program {
    Program::from_instructions(&[
        Instruction::Rda {
            addr: 12000,
            c: coeff::s1_9(0.55),
        },
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(1.0),
        },
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::Rda {
            addr: 12000,
            c: coeff::s1_9(0.8),
        },
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(1.0),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .expect("program fits")
}

fn chorus() -> Program {
    Program::from_instructions(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wlds {
            lfo: false,
            freq: 35,
            amp: 320,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        // Interpolated modulated tap, mixed with the dry signal.
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::COMPC,
            addr: 700,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Sin0,
            flags: ChoFlags::SIN,
            addr: 701,
        },
        Instruction::Sof {
            c: coeff::s1_14(0.7),
            d: 0,
        },
        Instruction::Rdax {
            reg: reg::ADCL,
            c: coeff::s1_14(0.7),
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .expect("program fits")
}

fn pitch_shift(rate: i16) -> Program {
    let temp = 16384u16;
    Program::from_instructions(&[
        Instruction::Skp {
            cond: SkpCond::RUN,
            n: 1,
        },
        Instruction::Wldr {
            lfo: false,
            freq: rate,
            amp: RampAmp::Amp4096,
        },
        Instruction::ldax(reg::ADCL),
        Instruction::Wra { addr: 0, c: 0 },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::REG | ChoFlags::COMPC,
            addr: 0,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::SIN,
            addr: 1,
        },
        Instruction::Wra { addr: temp, c: 0 },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::RPTR2 | ChoFlags::COMPC,
            addr: 0,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::RPTR2,
            addr: 1,
        },
        Instruction::ChoSof {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::NA | ChoFlags::COMPC,
            d: 0,
        },
        Instruction::ChoRda {
            lfo: LfoSel::Rmp0,
            flags: ChoFlags::NA,
            addr: temp,
        },
        Instruction::Wrax {
            reg: reg::DACL,
            c: 0,
        },
    ])
    .expect("program fits")
}

fn render(program: &Program, input: &[f32]) -> Vec<f32> {
    let mut fv1 = Fv1::new();
    fv1.load_program(program);
    input.iter().map(|&x| fv1.process(x, x).0).collect()
}

fn main() -> Result<()> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir)?;

    let dry = melody();
    let sr = SAMPLE_RATE as u32;

    let renders: [(&str, Vec<f32>); 5] = [
        ("dry.wav", dry.clone()),
        ("echo.wav", render(&feedback_echo(), &dry)),
        ("chorus.wav", render(&chorus(), &dry)),
        ("pitch_octave_down.wav", render(&pitch_shift(-8192), &dry)),
        ("pitch_fifth_up.wav", render(&pitch_shift(8192), &dry)),
    ];
    for (name, samples) in renders {
        let path = dir.join(name);
        write_wav(&path, &samples, sr)?;
        println!("wrote {} ({} samples)", path.display(), samples.len());
    }
    Ok(())
}
