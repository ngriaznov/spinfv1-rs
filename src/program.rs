//! FV-1 programs: 128 instruction words, loadable from raw words,
//! decoded instructions, or the 512-byte big-endian bank format.

use core::fmt;

use crate::instruction::Instruction;

/// Number of instruction slots; the chip executes all of them every sample.
pub const PROGRAM_LEN: usize = 128;

/// The canonical `NOP` word (`SKP 0,0`) used to pad short programs, matching
/// SpinASM's padding.
pub const NOP_WORD: u32 = 0x0000_0011;

/// A complete 128-word FV-1 program.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Program {
    words: [u32; PROGRAM_LEN],
}

/// Error building a [`Program`] from user input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProgramError {
    /// More than 128 instructions were supplied.
    TooLong(usize),
    /// A byte buffer whose length is not a multiple of 4.
    UnalignedBytes(usize),
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooLong(n) => write!(f, "program has {n} instructions, maximum is {PROGRAM_LEN}"),
            Self::UnalignedBytes(n) => {
                write!(f, "program byte length {n} is not a multiple of 4")
            }
        }
    }
}

impl core::error::Error for ProgramError {}

impl Program {
    /// Build from up to 128 raw words; shorter programs are padded with `NOP`.
    ///
    /// # Errors
    /// [`ProgramError::TooLong`] if more than 128 words are supplied.
    pub fn from_words(words: &[u32]) -> Result<Self, ProgramError> {
        if words.len() > PROGRAM_LEN {
            return Err(ProgramError::TooLong(words.len()));
        }
        let mut all = [NOP_WORD; PROGRAM_LEN];
        all[..words.len()].copy_from_slice(words);
        Ok(Self { words: all })
    }

    /// Build from up to 128 decoded instructions; padded with `NOP`.
    ///
    /// # Errors
    /// [`ProgramError::TooLong`] if more than 128 instructions are supplied.
    pub fn from_instructions(instructions: &[Instruction]) -> Result<Self, ProgramError> {
        if instructions.len() > PROGRAM_LEN {
            return Err(ProgramError::TooLong(instructions.len()));
        }
        let mut all = [NOP_WORD; PROGRAM_LEN];
        for (slot, ins) in all.iter_mut().zip(instructions) {
            *slot = ins.encode();
        }
        Ok(Self { words: all })
    }

    /// Build from the standard bank format: big-endian 32-bit words, at most
    /// 512 bytes (EEPROM images are exactly 512).
    ///
    /// # Errors
    /// [`ProgramError::UnalignedBytes`] if the length is not a multiple of 4,
    /// [`ProgramError::TooLong`] if there are more than 128 words.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProgramError> {
        if bytes.len() % 4 != 0 {
            return Err(ProgramError::UnalignedBytes(bytes.len()));
        }
        if bytes.len() > PROGRAM_LEN * 4 {
            return Err(ProgramError::TooLong(bytes.len() / 4));
        }
        let mut all = [NOP_WORD; PROGRAM_LEN];
        for (slot, chunk) in all.iter_mut().zip(bytes.chunks_exact(4)) {
            *slot = u32::from_be_bytes(chunk.try_into().expect("chunks_exact(4) yields 4 bytes"));
        }
        Ok(Self { words: all })
    }

    /// The raw program words.
    #[must_use]
    pub fn words(&self) -> &[u32; PROGRAM_LEN] {
        &self.words
    }

    /// Serialize to the 512-byte big-endian bank format.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; PROGRAM_LEN * 4] {
        let mut out = [0u8; PROGRAM_LEN * 4];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.words) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Decode all 128 slots.
    #[must_use]
    pub fn instructions(&self) -> [Instruction; PROGRAM_LEN] {
        self.words.map(Instruction::decode)
    }
}

impl Default for Program {
    /// An all-`NOP` program (silence: DAC registers stay at zero).
    fn default() -> Self {
        Self {
            words: [NOP_WORD; PROGRAM_LEN],
        }
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Program [")?;
        for (i, word) in self.words.iter().enumerate() {
            let ins = Instruction::decode(*word);
            if ins != Instruction::NOP {
                writeln!(f, "  {i:3}: {ins}")?;
            }
        }
        write!(f, "]")
    }
}
