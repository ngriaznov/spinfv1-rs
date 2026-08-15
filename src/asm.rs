//! A SpinASM assembler: turns FV-1 assembly source text into a [`Program`].
//!
//! This implements the subset of the SpinASM user manual syntax needed to
//! write real FV-1 programs by hand: mnemonics and pseudo-ops, `EQU` and
//! `MEM` directives, labels (with forward references for `SKP`), numeric
//! literals in decimal/hex/binary/real form (including exponent notation),
//! `NAME+n`/`NAME#-n`/`NAME^+n` offset expressions, full constant
//! expressions with C-like precedence (parentheses, unary `-`/`~`, `**`,
//! `*` `/`, `+` `-`, `<`/`>` shifts, `&`, whitespace-separated `^` XOR,
//! `|` flag combining), and the full predefined symbol table (register
//! names, `REG0..REG31`, `SIN0..RMP1`, `SKP` and `CHO` flag names).
//!
//! # Coefficient literals
//!
//! A coefficient operand (the `C`/`D` fields of `RDAX`, `RDA`, `SOF`, `LOG`,
//! `EXP`, `CHO SOF`, ...) is always parsed as a real number: a bare integer
//! literal like `RDAX REG0, 1` means `1.0`, not the raw fixed-point bit `1`.
//! The literal is quantized to the field's fixed-point format with
//! round-to-nearest (via [`crate::fixed::coeff`]) and must fall within the
//! format's representable range — *with one exception*: SpinASM lets you
//! write the conventional round-number spelling of a format's upper bound
//! even though it isn't exactly representable (e.g. `2.0` for an S1.14 or
//! S1.9 field whose true maximum is `1.99993896484375`/`1.998046875`,
//! `1.0` for S.10/S.15, `16.0` for LOG's S4.6 offset). This assembler follows that
//! convention: a literal exactly equal to `2.0`/`1.0`/`16.0` (as
//! appropriate for the field) is accepted and mapped to the format's
//! largest representable value; any other out-of-range literal is an error.
//!
//! # Example
//!
//! ```
//! use spinfv1::{Fv1, assemble};
//!
//! let source = "
//!     ; 100-sample echo at half gain
//!     DELAY   MEM 200
//!
//!             LDAX  ADCL
//!             WRA   DELAY, 0
//!             RDA   DELAY^, 0.5
//!             WRAX  DACL, 0
//! ";
//! let program = assemble(source).unwrap();
//! let mut fv1 = Fv1::new();
//! fv1.load_program(&program);
//! ```

use core::fmt;
use std::collections::{HashMap, HashSet};

use crate::{
    ChoFlags, DELAY_LEN, Instruction, LfoSel, PROGRAM_LEN, Program, RampAmp, SkpCond, coeff, reg,
};

/// An error produced while assembling SpinASM source.
///
/// Carries the 1-based source line the problem was found on, a descriptive
/// message, and (when available) the offending line's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsmError {
    line: usize,
    message: String,
    source_line: String,
}

impl AsmError {
    /// The 1-based source line the error was reported at.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// The error message, without the line number or source text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    fn at(line: usize, source_line: &str, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
            source_line: source_line.to_string(),
        }
    }
}

impl fmt::Display for AsmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.source_line.is_empty() {
            write!(f, "line {}: {}", self.line, self.message)
        } else {
            write!(
                f,
                "line {}: {} (`{}`)",
                self.line, self.message, self.source_line
            )
        }
    }
}

impl std::error::Error for AsmError {}

/// Assemble SpinASM source text into a [`Program`].
///
/// # Errors
/// Returns [`AsmError`] for any lexical, syntactic, or semantic problem:
/// unknown mnemonics or symbols, wrong operand counts, out-of-range values,
/// duplicate symbol/label definitions, a program longer than
/// [`PROGRAM_LEN`] instructions, `MEM` allocation overflow, malformed
/// numbers, or an out-of-range `SKP` target.
pub fn assemble(source: &str) -> Result<Program, AsmError> {
    let mut symtab = predefined_symbols();
    // Names defined as `NAME:` labels, so `SKP`'s second operand can
    // tell a jump target from an `EQU` skip-count constant.
    let mut labels: HashSet<String> = HashSet::new();
    let mut mem_end: i64 = 0;
    let mut pending: Vec<Pending> = Vec::new();

    for (row, raw_line) in source.lines().enumerate() {
        let line = row + 1;
        let code = raw_line.split(';').next().unwrap_or("");
        let text = code.trim();
        if text.is_empty() {
            continue;
        }
        let mut tokens = tokenize(text).map_err(|m| AsmError::at(line, text, m))?;

        // A leading `NAME:` is a label; it names the index of whatever
        // instruction comes next (possibly later on this same line).
        if let [Token::Ident(name), Token::Colon, ..] = tokens.as_slice() {
            define_symbol(
                &mut symtab,
                name,
                Num::Int(pending.len() as i64),
                line,
                text,
            )?;
            labels.insert(name.to_ascii_uppercase());
            tokens.drain(0..2);
        }
        if tokens.is_empty() {
            continue;
        }

        let Token::Ident(first) = &tokens[0] else {
            return Err(AsmError::at(line, text, "expected a mnemonic or directive"));
        };
        let first_upper = first.to_ascii_uppercase();
        // `EQU`/`MEM` are only recognized as directive keywords when the
        // line isn't already a real instruction: an instruction's first
        // operand sits at exactly the token position a name-first
        // `NAME EQU/MEM ...` directive would put its keyword, so a mnemonic
        // like `MULX` or `JAM` with an operand/symbol spelled `MEM`/`EQU`
        // (e.g. `JAM MEM 5`) must never be reinterpreted as a directive line.
        let is_directive_candidate = !is_mnemonic(&first_upper);
        let second_upper = match tokens.get(1) {
            Some(Token::Ident(s)) if is_directive_candidate => Some(s.to_ascii_uppercase()),
            _ => None,
        };

        if is_directive_candidate
            && (first_upper == "EQU" || second_upper.as_deref() == Some("EQU"))
        {
            let (name, rest) = directive_name(&tokens, first_upper == "EQU", line, text)?;
            if rest.is_empty() {
                return Err(AsmError::at(line, text, "EQU requires a value"));
            }
            let value = eval(rest, &symtab).map_err(|m| AsmError::at(line, text, m))?;
            define_symbol(&mut symtab, &name, value, line, text)?;
            continue;
        }
        if is_directive_candidate
            && (first_upper == "MEM" || second_upper.as_deref() == Some("MEM"))
        {
            let (name, rest) = directive_name(&tokens, first_upper == "MEM", line, text)?;
            if rest.is_empty() {
                return Err(AsmError::at(line, text, "MEM requires a size"));
            }
            let size = eval(rest, &symtab)
                .and_then(|v| {
                    v.as_int()
                        .ok_or_else(|| "MEM size must be an integer".to_string())
                })
                .map_err(|m| AsmError::at(line, text, m))?;
            if size < 0 {
                return Err(AsmError::at(line, text, "MEM size must not be negative"));
            }
            let start = mem_end;
            let end = start + size;
            if end > DELAY_LEN as i64 {
                return Err(AsmError::at(
                    line,
                    text,
                    format!("MEM allocation overflows delay memory: {end} exceeds {DELAY_LEN}"),
                ));
            }
            // SpinASM reserves `size + 1` locations per block — `name` is the
            // first and `name#` (= start + size) the last, with the next
            // block starting one word later. Spin's factory programs were
            // assembled with that layout, so tap spacings depend on it.
            mem_end = end + 1;
            define_symbol(&mut symtab, &name, Num::Int(start), line, text)?;
            define_symbol(&mut symtab, &format!("{name}#"), Num::Int(end), line, text)?;
            define_symbol(
                &mut symtab,
                &format!("{name}^"),
                Num::Int(start + size / 2),
                line,
                text,
            )?;
            continue;
        }

        if pending.len() >= PROGRAM_LEN {
            return Err(AsmError::at(
                line,
                text,
                format!("program exceeds {PROGRAM_LEN} instructions"),
            ));
        }
        pending.push(Pending {
            line,
            source_line: text.to_string(),
            mnemonic: first_upper,
            operands: tokens[1..].to_vec(),
        });
    }

    let instructions = pending
        .iter()
        .enumerate()
        .map(|(index, p)| parse_instruction(index, p, &symtab, &labels))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Program::from_instructions(&instructions).expect("bounded to PROGRAM_LEN during pass 1"))
}

/// A not-yet-resolved instruction line: mnemonic plus operand tokens, with
/// its source position kept for error reporting.
struct Pending {
    line: usize,
    source_line: String,
    mnemonic: String,
    operands: Vec<Token>,
}

/// Every real mnemonic and pseudo-op this assembler recognizes, upper-cased.
///
/// Used to decide whether a line's first token starts an instruction (in
/// which case `EQU`/`MEM` must never be reinterpreted as a directive
/// keyword, even if they appear as an operand token further along the
/// line) rather than a label/symbol definition.
const MNEMONICS: &[&str] = &[
    "RDA", "RMPA", "WRA", "WRAP", "RDAX", "RDFX", "WRAX", "WRHX", "WRLX", "MAXX", "MULX", "LOG",
    "EXP", "SOF", "AND", "OR", "XOR", "SKP", "WLDS", "WLDR", "JAM", "CHO", "NOP", "CLR", "NOT",
    "ABSA", "LDAX", "RAW",
];

/// Whether `upper` (already upper-cased) names a real mnemonic or pseudo-op.
fn is_mnemonic(upper: &str) -> bool {
    MNEMONICS.contains(&upper)
}

/// Split the `NAME EQU value` / `EQU NAME value` (and `MEM`) forms into the
/// defined name and the remaining value tokens.
fn directive_name<'a>(
    tokens: &'a [Token],
    keyword_first: bool,
    line: usize,
    text: &str,
) -> Result<(String, &'a [Token]), AsmError> {
    if keyword_first {
        match tokens.get(1) {
            Some(Token::Ident(name)) => Ok((name.clone(), &tokens[2..])),
            _ => Err(AsmError::at(
                line,
                text,
                "expected a name after the directive",
            )),
        }
    } else {
        let Token::Ident(name) = &tokens[0] else {
            unreachable!("caller already matched an identifier")
        };
        Ok((name.clone(), &tokens[2..]))
    }
}

fn define_symbol(
    symtab: &mut HashMap<String, Num>,
    name: &str,
    value: Num,
    line: usize,
    text: &str,
) -> Result<(), AsmError> {
    let key = name.to_ascii_uppercase();
    if symtab.contains_key(&key) {
        return Err(AsmError::at(
            line,
            text,
            format!("duplicate symbol `{name}`"),
        ));
    }
    symtab.insert(key, value);
    Ok(())
}

// ---------------------------------------------------------------------
// Lexing
// ---------------------------------------------------------------------

/// A numeric value: an integer (decimal/hex/binary, or a real with no
/// fractional part typed) or a real (a literal with a decimal point).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Int(i64),
    Real(f64),
}

impl Num {
    fn negate(self) -> Self {
        match self {
            Self::Int(n) => Self::Int(-n),
            Self::Real(x) => Self::Real(-x),
        }
    }

    fn add(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Int(a), Self::Int(b)) => Self::Int(a + b),
            (a, b) => Self::Real(a.as_f64() + b.as_f64()),
        }
    }

    fn sub(self, rhs: Self) -> Self {
        self.add(rhs.negate())
    }

    fn mul(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Int(a), Self::Int(b)) => Self::Int(a * b),
            (a, b) => Self::Real(a.as_f64() * b.as_f64()),
        }
    }

    /// Division stays an integer only when it is exact (`4096/2`); anything
    /// else (`1/64`) becomes a real, matching SpinASM's float expressions.
    fn div(self, rhs: Self) -> Result<Self, String> {
        if rhs.as_f64() == 0.0 {
            return Err("division by zero".to_string());
        }
        Ok(match (self, rhs) {
            (Self::Int(a), Self::Int(b)) if a % b == 0 => Self::Int(a / b),
            (a, b) => Self::Real(a.as_f64() / b.as_f64()),
        })
    }

    /// `**`: exponentiation. Stays an integer for integer base and small
    /// non-negative integer exponent; otherwise evaluates as a real.
    fn pow(self, rhs: Self) -> Result<Self, String> {
        match (self, rhs) {
            (Self::Int(a), Self::Int(b)) if (0..=63).contains(&b) => a
                .checked_pow(b as u32)
                .map(Self::Int)
                .ok_or_else(|| "integer overflow in '**'".to_string()),
            (a, b) => Ok(Self::Real(a.as_f64().powf(b.as_f64()))),
        }
    }

    fn as_int(self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(n),
            Self::Real(_) => None,
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Self::Int(n) => n as f64,
            Self::Real(x) => x,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Number(Num),
    Comma,
    Colon,
    Plus,
    Minus,
    Star,
    /// `**`: exponentiation.
    Pow,
    Slash,
    /// `<` or `<<`: left shift.
    Shl,
    /// `>` or `>>`: right shift.
    Shr,
    Pipe,
    /// `&`: bitwise AND.
    Amp,
    /// `^`: bitwise XOR. Only produced when `^` is *not* glued to the end
    /// of an identifier (`buf^` is the MEM midpoint suffix; `a ^ b` is XOR).
    Caret,
    /// `~`: bitwise complement (24-bit).
    Tilde,
    LParen,
    RParen,
}

/// Tokenize one already comment-stripped, non-blank source line.
fn tokenize(s: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            ':' => {
                out.push(Token::Colon);
                i += 1;
            }
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                if chars.get(i + 1) == Some(&'*') {
                    out.push(Token::Pow);
                    i += 2;
                } else {
                    out.push(Token::Star);
                    i += 1;
                }
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '&' => {
                out.push(Token::Amp);
                i += 1;
            }
            '^' => {
                out.push(Token::Caret);
                i += 1;
            }
            '~' => {
                out.push(Token::Tilde);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '<' => {
                // SpinASM writes left shift as `<` (Spin's rom_fla_rev uses
                // `fladel_138 < 8` to build an ADDR_PTR value); accept `<<` too.
                i += if chars.get(i + 1) == Some(&'<') { 2 } else { 1 };
                out.push(Token::Shl);
            }
            '>' => {
                i += if chars.get(i + 1) == Some(&'>') { 2 } else { 1 };
                out.push(Token::Shr);
            }
            '|' => {
                out.push(Token::Pipe);
                i += 1;
            }
            '$' => {
                i += 1;
                let start = i;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                if start == i {
                    return Err("expected hex digits after '$'".to_string());
                }
                out.push(Token::Number(Num::Int(parse_radix(&chars[start..i], 16)?)));
            }
            '%' => {
                i += 1;
                let start = i;
                let mut digits = String::new();
                while i < chars.len() && matches!(chars[i], '0' | '1' | '_') {
                    if chars[i] != '_' {
                        digits.push(chars[i]);
                    }
                    i += 1;
                }
                if start == i || digits.is_empty() {
                    return Err("expected binary digits after '%'".to_string());
                }
                let n = i64::from_str_radix(&digits, 2)
                    .map_err(|_| "binary literal too large".to_string())?;
                out.push(Token::Number(Num::Int(n)));
            }
            '0' if matches!(chars.get(i + 1), Some('x' | 'X')) => {
                i += 2;
                let start = i;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                if start == i {
                    return Err("expected hex digits after '0x'".to_string());
                }
                out.push(Token::Number(Num::Int(parse_radix(&chars[start..i], 16)?)));
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                let mut has_dot = false;
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || (chars[i] == '.' && !has_dot))
                {
                    has_dot |= chars[i] == '.';
                    i += 1;
                }
                // Exponent notation: `1e-3`, `1.5E2`.
                let mut has_exp = false;
                if matches!(chars.get(i), Some('e' | 'E')) {
                    let after_sign = if matches!(chars.get(i + 1), Some('+' | '-')) {
                        i + 2
                    } else {
                        i + 1
                    };
                    if chars.get(after_sign).is_some_and(char::is_ascii_digit) {
                        has_exp = true;
                        i = after_sign + 1;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().collect();
                if has_dot || has_exp {
                    let v: f64 = text
                        .parse()
                        .map_err(|_| format!("invalid number `{text}`"))?;
                    out.push(Token::Number(Num::Real(v)));
                } else {
                    let v: i64 = text
                        .parse()
                        .map_err(|_| format!("invalid number `{text}`"))?;
                    out.push(Token::Number(Num::Int(v)));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let mut text: String = chars[start..i].iter().collect();
                if matches!(chars.get(i), Some('#' | '^')) {
                    text.push(chars[i]);
                    i += 1;
                }
                out.push(Token::Ident(text));
            }
            other => return Err(format!("unexpected character '{other}'")),
        }
    }
    Ok(out)
}

fn parse_radix(digits: &[char], radix: u32) -> Result<i64, String> {
    let text: String = digits.iter().collect();
    i64::from_str_radix(&text, radix).map_err(|_| format!("numeric literal `{text}` too large"))
}

// ---------------------------------------------------------------------
// Expression evaluation, loosest to tightest binding:
//
//   expr    := xor    ( '|' xor )*                   (integers only)
//   xor     := and    ( '^' and )*                   (integers only)
//   and     := shift  ( '&' shift )*                 (integers only)
//   shift   := sum    ( ('<' | '>') sum )*           (integers only)
//   sum     := product( ('+' | '-') product )*
//   product := power  ( ('*' | '/') power )*
//   power   := unary  ( '**' power )?                (right-associative)
//   unary   := ('-' | '~') unary | number | symbol | '(' expr ')'
//
// Arithmetic follows SpinASM: values are integers until a real literal or
// an inexact division makes them real. Note the `^` disambiguation: glued
// to an identifier (`buf^`) it is the MEM midpoint suffix; separated by
// whitespace (`a ^ b`) it is XOR.
// ---------------------------------------------------------------------

fn eval(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<Num, String> {
    if tokens.is_empty() {
        return Err("expected a value".to_string());
    }
    let mut pos = 0;
    let v = eval_or(tokens, &mut pos, symtab)?;
    if pos < tokens.len() {
        return Err("unexpected token in expression".to_string());
    }
    Ok(v)
}

/// A precedence level's evaluator function.
type LevelFn = fn(&[Token], &mut usize, &HashMap<String, Num>) -> Result<Num, String>;

/// Shared shape of the integer-only bitwise levels (`|`, `^`, `&`).
fn eval_bitwise(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
    op_token: &Token,
    op_name: char,
    apply: fn(i64, i64) -> i64,
    next: LevelFn,
) -> Result<Num, String> {
    let mut acc = next(tokens, pos, symtab)?;
    while tokens.get(*pos) == Some(op_token) {
        *pos += 1;
        let rhs = next(tokens, pos, symtab)?;
        let a = acc
            .as_int()
            .ok_or_else(|| format!("cannot combine a real number with '{op_name}'"))?;
        let b = rhs
            .as_int()
            .ok_or_else(|| format!("cannot combine a real number with '{op_name}'"))?;
        acc = Num::Int(apply(a, b));
    }
    Ok(acc)
}

fn eval_or(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    eval_bitwise(
        tokens,
        pos,
        symtab,
        &Token::Pipe,
        '|',
        |a, b| a | b,
        eval_xor,
    )
}

fn eval_xor(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    eval_bitwise(
        tokens,
        pos,
        symtab,
        &Token::Caret,
        '^',
        |a, b| a ^ b,
        eval_and,
    )
}

fn eval_and(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    eval_bitwise(
        tokens,
        pos,
        symtab,
        &Token::Amp,
        '&',
        |a, b| a & b,
        eval_shift,
    )
}

fn eval_shift(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    let mut acc = eval_sum(tokens, pos, symtab)?;
    loop {
        let left = match tokens.get(*pos) {
            Some(Token::Shl) => true,
            Some(Token::Shr) => false,
            _ => break,
        };
        *pos += 1;
        let rhs = eval_sum(tokens, pos, symtab)?;
        let a = acc.as_int().ok_or("cannot shift a real number")?;
        let b = rhs
            .as_int()
            .filter(|n| (0..64).contains(n))
            .ok_or("shift amount must be an integer in 0..64")?;
        acc = Num::Int(if left { a << b } else { a >> b });
    }
    Ok(acc)
}

fn eval_sum(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    let mut v = eval_product(tokens, pos, symtab)?;
    loop {
        let minus = match tokens.get(*pos) {
            Some(Token::Plus) => false,
            Some(Token::Minus) => true,
            _ => break,
        };
        *pos += 1;
        let rhs = eval_product(tokens, pos, symtab)?;
        v = if minus { v.sub(rhs) } else { v.add(rhs) };
    }
    Ok(v)
}

fn eval_product(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    let mut v = eval_power(tokens, pos, symtab)?;
    loop {
        let div = match tokens.get(*pos) {
            Some(Token::Star) => false,
            Some(Token::Slash) => true,
            _ => break,
        };
        *pos += 1;
        let rhs = eval_power(tokens, pos, symtab)?;
        v = if div { v.div(rhs)? } else { v.mul(rhs) };
    }
    Ok(v)
}

fn eval_power(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    let base = eval_unary(tokens, pos, symtab)?;
    if tokens.get(*pos) == Some(&Token::Pow) {
        *pos += 1;
        // Right-associative: 2**2**3 = 2**(2**3).
        let exp = eval_power(tokens, pos, symtab)?;
        base.pow(exp)
    } else {
        Ok(base)
    }
}

fn eval_unary(
    tokens: &[Token],
    pos: &mut usize,
    symtab: &HashMap<String, Num>,
) -> Result<Num, String> {
    match tokens.get(*pos) {
        Some(Token::Minus) => {
            *pos += 1;
            Ok(eval_unary(tokens, pos, symtab)?.negate())
        }
        Some(Token::Tilde) => {
            *pos += 1;
            let v = eval_unary(tokens, pos, symtab)?;
            let a = v.as_int().ok_or("cannot complement a real number")?;
            // 24-bit complement, matching the chip's word width (SpinASM
            // documents ~ as inverting the 24 mask bits).
            Ok(Num::Int(!a & 0xFF_FFFF))
        }
        Some(Token::LParen) => {
            *pos += 1;
            let v = eval_or(tokens, pos, symtab)?;
            if tokens.get(*pos) != Some(&Token::RParen) {
                return Err("missing closing ')'".to_string());
            }
            *pos += 1;
            Ok(v)
        }
        Some(&Token::Number(n)) => {
            *pos += 1;
            Ok(n)
        }
        Some(Token::Ident(name)) => {
            *pos += 1;
            symtab
                .get(&name.to_ascii_uppercase())
                .copied()
                .ok_or_else(|| format!("unknown symbol `{name}`"))
        }
        _ => Err("expected a number, symbol, or '('".to_string()),
    }
}

fn split_commas(tokens: &[Token]) -> Vec<Vec<Token>> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut groups = Vec::new();
    let mut cur = Vec::new();
    for t in tokens {
        if *t == Token::Comma {
            groups.push(std::mem::take(&mut cur));
        } else {
            cur.push(t.clone());
        }
    }
    groups.push(cur);
    groups
}

// ---------------------------------------------------------------------
// Field parsers
// ---------------------------------------------------------------------

fn int_value(tokens: &[Token], symtab: &HashMap<String, Num>, what: &str) -> Result<i64, String> {
    eval(tokens, symtab)?
        .as_int()
        .ok_or_else(|| format!("{what} must be an integer, not a real number"))
}

fn int_in_range(
    tokens: &[Token],
    symtab: &HashMap<String, Num>,
    lo: i64,
    hi: i64,
    what: &str,
) -> Result<i64, String> {
    let n = int_value(tokens, symtab, what)?;
    if (lo..=hi).contains(&n) {
        Ok(n)
    } else {
        Err(format!("{what} {n} is out of range {lo}..={hi}"))
    }
}

fn addr_value(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<u16, String> {
    int_in_range(tokens, symtab, 0, 32767, "address").map(|n| n as u16)
}

fn reg_value(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<u8, String> {
    int_in_range(tokens, symtab, 0, 63, "register").map(|n| n as u8)
}

fn mask_value(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<u32, String> {
    let n = eval(tokens, symtab)?.as_int().ok_or_else(|| {
        "mask must be an integer (hex, binary, or decimal), not a real number".to_string()
    })?;
    if (-0x80_0000..=0xFF_FFFF).contains(&n) {
        Ok((n as u32) & 0xFF_FFFF)
    } else {
        Err(format!("mask value {n} is out of range for a 24-bit field"))
    }
}

fn raw_word(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<u32, String> {
    int_in_range(tokens, symtab, 0, 0xFFFF_FFFF, "RAW word").map(|n| n as u32)
}

/// Quantize a real-valued coefficient operand into a fixed-point field.
///
/// Accepts any value that rounds into `raw_min..=raw_max` once scaled, plus
/// the exact `shorthand` value (SpinASM's conventional spelling of the
/// format's upper bound) as an alias for the largest representable value.
fn quantize(
    x: f64,
    scale: f64,
    raw_min: i64,
    raw_max: i64,
    shorthand: f64,
    name: &str,
    quant: fn(f64) -> i16,
) -> Result<i16, String> {
    let raw = (x * scale).round();
    if (raw_min as f64..=raw_max as f64).contains(&raw) || x == shorthand {
        Ok(quant(x))
    } else {
        Err(format!(
            "{name} coefficient {x} is out of range [{:.6}, {:.6}] (or exactly {shorthand} as shorthand for the maximum)",
            raw_min as f64 / scale,
            raw_max as f64 / scale,
        ))
    }
}

fn coeff_value(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<f64, String> {
    Ok(eval(tokens, symtab)?.as_f64())
}

fn coeff_s1_14(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<i16, String> {
    quantize(
        coeff_value(tokens, symtab)?,
        16384.0,
        -32768,
        32767,
        2.0,
        "S1.14",
        coeff::s1_14,
    )
}

fn coeff_s1_9(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<i16, String> {
    quantize(
        coeff_value(tokens, symtab)?,
        512.0,
        -1024,
        1023,
        2.0,
        "S1.9",
        coeff::s1_9,
    )
}

fn coeff_s_10(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<i16, String> {
    quantize(
        coeff_value(tokens, symtab)?,
        1024.0,
        -1024,
        1023,
        1.0,
        "S.10",
        coeff::s_10,
    )
}

fn coeff_s4_6(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<i16, String> {
    quantize(
        coeff_value(tokens, symtab)?,
        64.0,
        -1024,
        1023,
        16.0,
        "S4.6",
        coeff::s4_6,
    )
}

fn coeff_s_15(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<i16, String> {
    quantize(
        coeff_value(tokens, symtab)?,
        32768.0,
        -32768,
        32767,
        1.0,
        "S.15",
        coeff::s_15,
    )
}

#[derive(Clone, Copy)]
enum LfoKind {
    Sin,
    Rmp,
}

/// The single-bit LFO selector used by `WLDS`/`WLDR`/`JAM`: `SIN0`/`SIN1`
/// or `RMP0`/`RMP1` by name, or a bare `0`/`1`.
fn lfo_bit(tokens: &[Token], symtab: &HashMap<String, Num>, kind: LfoKind) -> Result<bool, String> {
    if let [Token::Ident(name)] = tokens {
        match (kind, name.to_ascii_uppercase().as_str()) {
            (LfoKind::Sin, "SIN0") | (LfoKind::Rmp, "RMP0") => return Ok(false),
            (LfoKind::Sin, "SIN1") | (LfoKind::Rmp, "RMP1") => return Ok(true),
            _ => {}
        }
    }
    match int_value(tokens, symtab, "LFO selector")? {
        0 => Ok(false),
        1 => Ok(true),
        n => Err(format!("LFO selector {n} out of range (expected 0 or 1)")),
    }
}

/// The 2-bit LFO selector used by `CHO`: any of the four LFO names, or an
/// integer `0..=3`.
fn cho_lfo(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<LfoSel, String> {
    if let [Token::Ident(name)] = tokens {
        match name.to_ascii_uppercase().as_str() {
            "SIN0" => return Ok(LfoSel::Sin0),
            "SIN1" => return Ok(LfoSel::Sin1),
            "RMP0" => return Ok(LfoSel::Rmp0),
            "RMP1" => return Ok(LfoSel::Rmp1),
            _ => {}
        }
    }
    match int_value(tokens, symtab, "CHO LFO selector")? {
        0 => Ok(LfoSel::Sin0),
        1 => Ok(LfoSel::Sin1),
        2 => Ok(LfoSel::Rmp0),
        3 => Ok(LfoSel::Rmp1),
        n => Err(format!("CHO LFO selector {n} out of range 0..=3")),
    }
}

fn cho_flags(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<ChoFlags, String> {
    // Spin's own program listings write an empty flags operand
    // (`cho rda, RMP0, , Delay+1`), meaning "no flags".
    if tokens.is_empty() {
        return Ok(ChoFlags(0));
    }
    let n = eval(tokens, symtab)?
        .as_int()
        .ok_or_else(|| "CHO flags must be an integer or combination of flag names".to_string())?;
    if (0..=0x3F).contains(&n) {
        Ok(ChoFlags(n as u8))
    } else {
        Err(format!("CHO flags {n:#X} out of range for a 6-bit field"))
    }
}

fn skp_cond(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<SkpCond, String> {
    let n = eval(tokens, symtab)?.as_int().ok_or_else(|| {
        "SKP condition must be an integer or combination of flag names".to_string()
    })?;
    if (0..=0x1F).contains(&n) {
        Ok(SkpCond(n as u8))
    } else {
        Err(format!(
            "SKP condition {n:#X} out of range for a 5-bit field"
        ))
    }
}

/// `SKP`'s second operand: a label name (resolved to the relative skip
/// distance), or an integer skip count `0..=63` — including a count
/// spelled as an `EQU` constant, which SpinASM treats as the count `N`
/// itself, never as a target index. Only names defined with `NAME:`
/// take the label path.
fn skp_target(
    tokens: &[Token],
    symtab: &HashMap<String, Num>,
    labels: &HashSet<String>,
    this_index: usize,
) -> Result<u8, String> {
    match tokens {
        [Token::Ident(name)] if labels.contains(&name.to_ascii_uppercase()) => {
            let target = symtab
                .get(&name.to_ascii_uppercase())
                .ok_or_else(|| format!("unknown label `{name}`"))?
                .as_int()
                .ok_or_else(|| format!("`{name}` is not a label or integer"))?;
            if target <= this_index as i64 {
                return Err(format!(
                    "SKP target `{name}` (instruction {target}) must be after this instruction ({this_index})"
                ));
            }
            let dist = target - this_index as i64 - 1;
            if dist > 63 {
                return Err(format!("SKP distance to `{name}` is {dist}, maximum is 63"));
            }
            Ok(dist as u8)
        }
        [Token::Number(Num::Int(n))] if (0..=63).contains(n) => Ok(*n as u8),
        [Token::Number(Num::Int(n))] => Err(format!("SKP distance {n} out of range 0..=63")),
        [Token::Number(Num::Real(_))] => {
            Err("SKP distance must be an integer, not a real number".to_string())
        }
        // A non-label symbol (EQU constant) or expression is a count.
        _ => match int_value(tokens, symtab, "SKP distance")? {
            n @ 0..=63 => Ok(n as u8),
            n => Err(format!("SKP distance {n} out of range 0..=63")),
        },
    }
}

fn wldr_amp(tokens: &[Token], symtab: &HashMap<String, Num>) -> Result<RampAmp, String> {
    match int_value(tokens, symtab, "WLDR amplitude")? {
        4096 => Ok(RampAmp::Amp4096),
        2048 => Ok(RampAmp::Amp2048),
        1024 => Ok(RampAmp::Amp1024),
        512 => Ok(RampAmp::Amp512),
        n => Err(format!(
            "WLDR amplitude {n} must be one of 512, 1024, 2048, 4096"
        )),
    }
}

// ---------------------------------------------------------------------
// Instruction dispatch
// ---------------------------------------------------------------------

#[allow(
    clippy::too_many_lines,
    reason = "one match arm per mnemonic, each a single field parse"
)]
fn parse_instruction(
    index: usize,
    p: &Pending,
    symtab: &HashMap<String, Num>,
    labels: &HashSet<String>,
) -> Result<Instruction, AsmError> {
    let err = |m: String| AsmError::at(p.line, &p.source_line, m);
    let groups = split_commas(&p.operands);
    let need = |n: usize| -> Result<(), AsmError> {
        if groups.len() == n {
            Ok(())
        } else {
            let plural = if n == 1 { "" } else { "s" };
            Err(err(format!(
                "{} expects {n} operand{plural}, found {}",
                p.mnemonic,
                groups.len()
            )))
        }
    };
    match p.mnemonic.as_str() {
        "RDA" => {
            need(2)?;
            Ok(Instruction::Rda {
                addr: addr_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_9(&groups[1], symtab).map_err(&err)?,
            })
        }
        "RMPA" => {
            need(1)?;
            Ok(Instruction::Rmpa {
                c: coeff_s1_9(&groups[0], symtab).map_err(&err)?,
            })
        }
        "WRA" => {
            need(2)?;
            Ok(Instruction::Wra {
                addr: addr_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_9(&groups[1], symtab).map_err(&err)?,
            })
        }
        "WRAP" => {
            need(2)?;
            Ok(Instruction::Wrap {
                addr: addr_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_9(&groups[1], symtab).map_err(&err)?,
            })
        }
        "RDAX" => {
            need(2)?;
            Ok(Instruction::Rdax {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_14(&groups[1], symtab).map_err(&err)?,
            })
        }
        "RDFX" => {
            need(2)?;
            Ok(Instruction::Rdfx {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_14(&groups[1], symtab).map_err(&err)?,
            })
        }
        "WRAX" => {
            need(2)?;
            Ok(Instruction::Wrax {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_14(&groups[1], symtab).map_err(&err)?,
            })
        }
        "WRHX" => {
            need(2)?;
            Ok(Instruction::Wrhx {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_14(&groups[1], symtab).map_err(&err)?,
            })
        }
        "WRLX" => {
            need(2)?;
            Ok(Instruction::Wrlx {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_14(&groups[1], symtab).map_err(&err)?,
            })
        }
        "MAXX" => {
            need(2)?;
            Ok(Instruction::Maxx {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
                c: coeff_s1_14(&groups[1], symtab).map_err(&err)?,
            })
        }
        "MULX" => {
            need(1)?;
            Ok(Instruction::Mulx {
                reg: reg_value(&groups[0], symtab).map_err(&err)?,
            })
        }
        "LOG" => {
            need(2)?;
            Ok(Instruction::Log {
                c: coeff_s1_14(&groups[0], symtab).map_err(&err)?,
                // SpinASM user manual, LOG: D is entered as Real(S4.6), range
                // -16 to +15.999998. (The knowledge-base cheat sheet lists
                // "-1 to +0.999023" for LOG's K2 — a copy of EXP's row;
                // the tool manual wins here.)
                d: coeff_s4_6(&groups[1], symtab).map_err(&err)?,
            })
        }
        "EXP" => {
            need(2)?;
            Ok(Instruction::Exp {
                c: coeff_s1_14(&groups[0], symtab).map_err(&err)?,
                d: coeff_s_10(&groups[1], symtab).map_err(&err)?,
            })
        }
        "SOF" => {
            need(2)?;
            Ok(Instruction::Sof {
                c: coeff_s1_14(&groups[0], symtab).map_err(&err)?,
                d: coeff_s_10(&groups[1], symtab).map_err(&err)?,
            })
        }
        "AND" => {
            need(1)?;
            Ok(Instruction::And {
                mask: mask_value(&groups[0], symtab).map_err(&err)?,
            })
        }
        "OR" => {
            need(1)?;
            Ok(Instruction::Or {
                mask: mask_value(&groups[0], symtab).map_err(&err)?,
            })
        }
        "XOR" => {
            need(1)?;
            Ok(Instruction::Xor {
                mask: mask_value(&groups[0], symtab).map_err(&err)?,
            })
        }
        "SKP" => {
            need(2)?;
            Ok(Instruction::Skp {
                cond: skp_cond(&groups[0], symtab).map_err(&err)?,
                n: skp_target(&groups[1], symtab, labels, index).map_err(&err)?,
            })
        }
        "WLDS" => {
            need(3)?;
            Ok(Instruction::Wlds {
                lfo: lfo_bit(&groups[0], symtab, LfoKind::Sin).map_err(&err)?,
                freq: int_in_range(&groups[1], symtab, 0, 511, "WLDS frequency").map_err(&err)?
                    as u16,
                amp: int_in_range(&groups[2], symtab, 0, 32767, "WLDS amplitude").map_err(&err)?
                    as u16,
            })
        }
        "WLDR" => {
            need(3)?;
            Ok(Instruction::Wldr {
                lfo: lfo_bit(&groups[0], symtab, LfoKind::Rmp).map_err(&err)?,
                freq: int_in_range(&groups[1], symtab, -32768, 32767, "WLDR frequency")
                    .map_err(&err)? as i16,
                amp: wldr_amp(&groups[2], symtab).map_err(&err)?,
            })
        }
        "JAM" => {
            need(1)?;
            Ok(Instruction::Jam {
                lfo: lfo_bit(&groups[0], symtab, LfoKind::Rmp).map_err(&err)?,
            })
        }
        "CHO" => parse_cho(&groups, symtab).map_err(&err),
        "NOP" => {
            need(0)?;
            Ok(Instruction::NOP)
        }
        "CLR" => {
            need(0)?;
            Ok(Instruction::CLR)
        }
        "NOT" => {
            need(0)?;
            Ok(Instruction::NOT)
        }
        "ABSA" => {
            need(0)?;
            Ok(Instruction::ABSA)
        }
        "LDAX" => {
            need(1)?;
            Ok(Instruction::ldax(
                reg_value(&groups[0], symtab).map_err(&err)?,
            ))
        }
        "RAW" => {
            need(1)?;
            Ok(Instruction::Raw(
                raw_word(&groups[0], symtab).map_err(&err)?,
            ))
        }
        other => Err(err(format!("unknown mnemonic `{other}`"))),
    }
}

fn parse_cho(groups: &[Vec<Token>], symtab: &HashMap<String, Num>) -> Result<Instruction, String> {
    let Some([Token::Ident(sub)]) = groups.first().map(Vec::as_slice) else {
        return Err("CHO requires a subtype: RDA, SOF, or RDAL".to_string());
    };
    match sub.to_ascii_uppercase().as_str() {
        "RDA" => {
            if groups.len() != 4 {
                return Err(format!(
                    "CHO RDA expects 4 operands, found {}",
                    groups.len()
                ));
            }
            Ok(Instruction::ChoRda {
                lfo: cho_lfo(&groups[1], symtab)?,
                flags: cho_flags(&groups[2], symtab)?,
                addr: addr_value(&groups[3], symtab)?,
            })
        }
        "SOF" => {
            if groups.len() != 4 {
                return Err(format!(
                    "CHO SOF expects 4 operands, found {}",
                    groups.len()
                ));
            }
            Ok(Instruction::ChoSof {
                lfo: cho_lfo(&groups[1], symtab)?,
                flags: cho_flags(&groups[2], symtab)?,
                d: coeff_s_15(&groups[3], symtab)?,
            })
        }
        "RDAL" => {
            if groups.len() != 2 && groups.len() != 3 {
                return Err(format!(
                    "CHO RDAL expects 2 or 3 operands, found {}",
                    groups.len()
                ));
            }
            // SpinASM also accepts COS0/COS1 here, naming the cosine
            // output of a SIN LFO (selector + COS flag in one token).
            let (lfo, extra) = match groups[1].as_slice() {
                [Token::Ident(name)] if name.eq_ignore_ascii_case("COS0") => {
                    (LfoSel::Sin0, ChoFlags::COS)
                }
                [Token::Ident(name)] if name.eq_ignore_ascii_case("COS1") => {
                    (LfoSel::Sin1, ChoFlags::COS)
                }
                _ => (cho_lfo(&groups[1], symtab)?, ChoFlags(0)),
            };
            // SpinASM assembles a bare `CHO RDAL, lfo` with the REG bit
            // set (it has no effect on RDAL's result); match that
            // canonical encoding when the flags operand is omitted.
            let flags = if groups.len() == 3 {
                cho_flags(&groups[2], symtab)?
            } else {
                ChoFlags::REG
            };
            Ok(Instruction::ChoRdal {
                lfo,
                flags: ChoFlags(flags.0 | extra.0),
            })
        }
        other => Err(format!(
            "unknown CHO subtype `{other}`, expected RDA, SOF, or RDAL"
        )),
    }
}

// ---------------------------------------------------------------------
// Predefined symbol table
// ---------------------------------------------------------------------

fn predefined_symbols() -> HashMap<String, Num> {
    let regs: &[(&str, u8)] = &[
        ("SIN0_RATE", reg::SIN0_RATE),
        ("SIN0_RANGE", reg::SIN0_RANGE),
        ("SIN1_RATE", reg::SIN1_RATE),
        ("SIN1_RANGE", reg::SIN1_RANGE),
        ("RMP0_RATE", reg::RMP0_RATE),
        ("RMP0_RANGE", reg::RMP0_RANGE),
        ("RMP1_RATE", reg::RMP1_RATE),
        ("RMP1_RANGE", reg::RMP1_RANGE),
        ("POT0", reg::POT0),
        ("POT1", reg::POT1),
        ("POT2", reg::POT2),
        ("ADCL", reg::ADCL),
        ("ADCR", reg::ADCR),
        ("DACL", reg::DACL),
        ("DACR", reg::DACR),
        ("ADDR_PTR", reg::ADDR_PTR),
    ];
    let mut m: HashMap<String, Num> = regs
        .iter()
        .map(|&(name, v)| (name.to_string(), Num::Int(i64::from(v))))
        .collect();
    for n in 0..32u8 {
        m.insert(format!("REG{n}"), Num::Int(i64::from(reg::user(n))));
    }
    // LFO selectors, as used by CHO's 2-bit field (0=SIN0, 1=SIN1, 2=RMP0, 3=RMP1).
    m.insert("SIN0".to_string(), Num::Int(0));
    m.insert("SIN1".to_string(), Num::Int(1));
    m.insert("RMP0".to_string(), Num::Int(2));
    m.insert("RMP1".to_string(), Num::Int(3));
    // SKP condition flags.
    m.insert("NEG".to_string(), Num::Int(i64::from(SkpCond::NEG.0)));
    m.insert("GEZ".to_string(), Num::Int(i64::from(SkpCond::GEZ.0)));
    m.insert("ZRO".to_string(), Num::Int(i64::from(SkpCond::ZRO.0)));
    m.insert("ZRC".to_string(), Num::Int(i64::from(SkpCond::ZRC.0)));
    m.insert("RUN".to_string(), Num::Int(i64::from(SkpCond::RUN.0)));
    // CHO flags.
    m.insert("SIN".to_string(), Num::Int(i64::from(ChoFlags::SIN.0)));
    m.insert("COS".to_string(), Num::Int(i64::from(ChoFlags::COS.0)));
    m.insert("REG".to_string(), Num::Int(i64::from(ChoFlags::REG.0)));
    m.insert("COMPC".to_string(), Num::Int(i64::from(ChoFlags::COMPC.0)));
    m.insert("COMPA".to_string(), Num::Int(i64::from(ChoFlags::COMPA.0)));
    m.insert("RPTR2".to_string(), Num::Int(i64::from(ChoFlags::RPTR2.0)));
    m.insert("NA".to_string(), Num::Int(i64::from(ChoFlags::NA.0)));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(pairs: &[(&str, i64)]) -> HashMap<String, Num> {
        pairs
            .iter()
            .map(|&(k, v)| (k.to_string(), Num::Int(v)))
            .collect()
    }

    #[test]
    fn tokenizes_numeric_forms() {
        let toks = tokenize("100 -5 1.0 -.75 0.5 $3F 0x3F %0101_1100").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Number(Num::Int(100)),
                Token::Minus,
                Token::Number(Num::Int(5)),
                Token::Number(Num::Real(1.0)),
                Token::Minus,
                Token::Number(Num::Real(0.75)),
                Token::Number(Num::Real(0.5)),
                Token::Number(Num::Int(0x3F)),
                Token::Number(Num::Int(0x3F)),
                Token::Number(Num::Int(0b0101_1100)),
            ]
        );
    }

    #[test]
    fn tokenizes_identifiers_with_mem_suffixes() {
        let toks = tokenize("DELAY DELAY# DELAY^").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Ident("DELAY".to_string()),
                Token::Ident("DELAY#".to_string()),
                Token::Ident("DELAY^".to_string()),
            ]
        );
    }

    #[test]
    fn expression_offsets_and_pipe() {
        let symtab = sym(&[
            ("DELAY", 10),
            ("DELAY#", 110),
            ("DELAY^", 60),
            ("SIN", 0),
            ("COMPC", 4),
        ]);
        let eval_str = |s: &str| eval(&tokenize(s).unwrap(), &symtab).unwrap();
        assert_eq!(eval_str("DELAY+100"), Num::Int(110));
        assert_eq!(eval_str("DELAY#-1"), Num::Int(109));
        assert_eq!(eval_str("DELAY^+3"), Num::Int(63));
        assert_eq!(eval_str("SIN|COMPC"), Num::Int(4));
        assert_eq!(eval_str("-2.0"), Num::Real(-2.0));
    }

    #[test]
    fn coeff_shorthand_maps_to_max_and_rejects_near_misses() {
        assert_eq!(
            coeff_s1_14(&tokenize("2.0").unwrap(), &HashMap::new()),
            Ok(32767)
        );
        assert_eq!(
            coeff_s1_14(&tokenize("-2.0").unwrap(), &HashMap::new()),
            Ok(-32768)
        );
        assert!(coeff_s1_14(&tokenize("2.5").unwrap(), &HashMap::new()).is_err());
        assert!(coeff_s1_14(&tokenize("1.99999").unwrap(), &HashMap::new()).is_err());
    }
}
