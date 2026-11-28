//! Tokens of ISO 10303-11 (EXPRESS) text: keywords and identifiers (case-insensitive), literals,
//! punctuation and operators. Remarks, both `(* ... *)` (nested) and `-- ...` to the end of the
//! line, are skipped.

use std::fmt;

/// Nesting limit of embedded remarks.
const MAX_REMARK_DEPTH: usize = 64;

/// A position in the source, 1-based.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.column)
    }
}

/// A syntax error with its position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub pos: Pos,
    pub message: String,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.message)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A keyword or identifier as written; compare keywords with [`Token::is`].
    Word(String),
    Integer(u64),
    Real(f64),
    /// A simple string literal with `''` already folded to `'`.
    Str(String),
    /// An encoded string literal (`"..."`), hexadecimal octets as written.
    Encoded(String),
    /// A binary literal (`%0101`), the bits as written.
    Binary(String),
    /// Punctuation or an operator, such as `;`, `:=`, `<*` or `:<>:`.
    Sym(&'static str),
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: Kind,
    pub pos: Pos,
    /// Byte range in the source.
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// Whether the token is the keyword or identifier `word` (ASCII case-insensitive).
    pub fn is(&self, word: &str) -> bool {
        matches!(&self.kind, Kind::Word(w) if w.eq_ignore_ascii_case(word))
    }

    pub fn is_sym(&self, sym: &str) -> bool {
        matches!(self.kind, Kind::Sym(s) if s == sym)
    }
}

/// Longest first, so that `:<>:` wins over `:` and `<*` over `<`.
const SYMBOLS: &[&str] = &[
    ":<>:", ":=:", "<=", ">=", "<>", ":=", "**", "||", "<*", ";", ":", ",", ".", "(", ")", "[",
    "]", "{", "}", "=", "<", ">", "+", "-", "*", "/", "\\", "|", "?",
];

/// Splits `source` into tokens, ending with [`Kind::End`].
pub fn tokenize(source: &str) -> Result<Vec<Token>, SyntaxError> {
    let mut lexer = Lexer {
        bytes: source.as_bytes(),
        at: 0,
        pos: Pos { line: 1, column: 1 },
    };
    let mut tokens = Vec::new();
    loop {
        lexer.skip_space_and_remarks()?;
        let token = lexer.token(source)?;
        let end = token.kind == Kind::End;
        tokens.push(token);
        if end {
            return Ok(tokens);
        }
    }
}

struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    pos: Pos,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.at + ahead).copied()
    }

    fn bump(&mut self) {
        if let Some(byte) = self.peek(0) {
            self.at += 1;
            if byte == b'\n' {
                self.pos.line += 1;
                self.pos.column = 1;
            } else if byte & 0xC0 != 0x80 {
                // Columns count characters, not UTF-8 continuation bytes.
                self.pos.column += 1;
            }
        }
    }

    fn error(&self, pos: Pos, message: impl Into<String>) -> SyntaxError {
        SyntaxError {
            pos,
            message: message.into(),
        }
    }

    fn skip_space_and_remarks(&mut self) -> Result<(), SyntaxError> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b), _) if b.is_ascii_whitespace() => self.bump(),
                (Some(b'('), Some(b'*')) => self.skip_remark()?,
                (Some(b'-'), Some(b'-')) => {
                    while self.peek(0).is_some_and(|b| b != b'\n') {
                        self.bump();
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// Skips an embedded remark; remarks nest (ISO 10303-11, 7.1.6.1).
    fn skip_remark(&mut self) -> Result<(), SyntaxError> {
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b'('), Some(b'*')) => {
                    depth += 1;
                    if depth > MAX_REMARK_DEPTH {
                        return Err(self.error(self.pos, "remarks nested too deeply"));
                    }
                    self.bump();
                    self.bump();
                }
                (Some(b'*'), Some(b')')) => {
                    self.bump();
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                (Some(_), _) => self.bump(),
                (None, _) => return Err(self.error(start, "remark is not closed")),
            }
        }
    }

    fn token(&mut self, source: &str) -> Result<Token, SyntaxError> {
        let pos = self.pos;
        let start = self.at;
        let kind = match self.peek(0) {
            None => Kind::End,
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => {
                while self
                    .peek(0)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    self.bump();
                }
                Kind::Word(source[start..self.at].to_owned())
            }
            Some(b) if b.is_ascii_digit() => self.number(source, pos)?,
            Some(b'\'') => self.string(pos)?,
            Some(b'"') => self.delimited(pos, b'"', "encoded string", Kind::Encoded)?,
            Some(b'%') => {
                self.bump();
                let from = self.at;
                while self.peek(0).is_some_and(|b| b == b'0' || b == b'1') {
                    self.bump();
                }
                if from == self.at {
                    return Err(self.error(pos, "binary literal without bits"));
                }
                Kind::Binary(source[from..self.at].to_owned())
            }
            Some(_) => {
                let rest = &self.bytes[self.at..];
                let Some(sym) = SYMBOLS.iter().find(|s| rest.starts_with(s.as_bytes())) else {
                    let ch = source[start..].chars().next().unwrap_or('?');
                    return Err(self.error(pos, format!("unexpected character {ch:?}")));
                };
                for _ in 0..sym.len() {
                    self.bump();
                }
                Kind::Sym(sym)
            }
        };
        Ok(Token {
            kind,
            pos,
            start,
            end: self.at,
        })
    }

    /// An integer or real literal: `digits [ . digits* ] [ E [+-] digits ]`.
    fn number(&mut self, source: &str, pos: Pos) -> Result<Kind, SyntaxError> {
        let start = self.at;
        let digits = |lexer: &mut Self| {
            while lexer.peek(0).is_some_and(|b| b.is_ascii_digit()) {
                lexer.bump();
            }
        };
        digits(self);
        // `E` starts an exponent only before a digit or a sign and a digit, so that `x[1].Exponent`
        // stays an attribute reference.
        let exponent_at = |lexer: &Self, at: usize| {
            matches!(lexer.peek(at), Some(b'e' | b'E'))
                && match lexer.peek(at + 1) {
                    Some(b'+' | b'-') => lexer.peek(at + 2).is_some_and(|b| b.is_ascii_digit()),
                    Some(b) => b.is_ascii_digit(),
                    None => false,
                }
        };
        let mut real = false;
        if self.peek(0) == Some(b'.')
            && (!self.peek(1).is_some_and(|b| b.is_ascii_alphabetic()) || exponent_at(self, 1))
        {
            real = true;
            self.bump();
            digits(self);
        }
        if exponent_at(self, 0) {
            real = true;
            self.bump();
            if matches!(self.peek(0), Some(b'+' | b'-')) {
                self.bump();
            }
            digits(self);
        }
        let text = &source[start..self.at];
        if real {
            text.parse()
                .map(Kind::Real)
                .map_err(|_| self.error(pos, format!("invalid real literal {text}")))
        } else {
            text.parse()
                .map(Kind::Integer)
                .map_err(|_| self.error(pos, format!("integer literal {text} out of range")))
        }
    }

    /// A simple string literal; `''` stands for one quote.
    fn string(&mut self, pos: Pos) -> Result<Kind, SyntaxError> {
        self.bump();
        let mut value = Vec::new();
        loop {
            match self.peek(0) {
                None => return Err(self.error(pos, "string literal is not closed")),
                Some(b'\'') if self.peek(1) == Some(b'\'') => {
                    value.push(b'\'');
                    self.bump();
                    self.bump();
                }
                Some(b'\'') => {
                    self.bump();
                    return Ok(Kind::Str(String::from_utf8_lossy(&value).into_owned()));
                }
                Some(b) => {
                    value.push(b);
                    self.bump();
                }
            }
        }
    }

    fn delimited(
        &mut self,
        pos: Pos,
        quote: u8,
        what: &str,
        make: fn(String) -> Kind,
    ) -> Result<Kind, SyntaxError> {
        self.bump();
        let from = self.at;
        while self.peek(0).is_some_and(|b| b != quote) {
            self.bump();
        }
        if self.peek(0).is_none() {
            return Err(self.error(pos, format!("{what} literal is not closed")));
        }
        let value = String::from_utf8_lossy(&self.bytes[from..self.at]).into_owned();
        self.bump();
        Ok(make(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Kind> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn remarks_nest_and_tail_remarks_end_at_the_line() {
        let k = kinds("a (* x (* y *) z *) b -- c\nd");
        assert_eq!(
            k,
            vec![
                Kind::Word("a".into()),
                Kind::Word("b".into()),
                Kind::Word("d".into()),
                Kind::End
            ]
        );
    }

    #[test]
    fn strings_fold_doubled_quotes() {
        assert_eq!(kinds("'it''s'")[0], Kind::Str("it's".into()));
        assert_eq!(kinds("''")[0], Kind::Str(String::new()));
        assert_eq!(kinds("\"00000041\"")[0], Kind::Encoded("00000041".into()));
    }

    #[test]
    fn numbers_and_operators() {
        assert_eq!(
            kinds("1.E-5 2. 0.5 12 a.b x<*y :<>: :=:"),
            vec![
                Kind::Real(1e-5),
                Kind::Real(2.0),
                Kind::Real(0.5),
                Kind::Integer(12),
                Kind::Word("a".into()),
                Kind::Sym("."),
                Kind::Word("b".into()),
                Kind::Word("x".into()),
                Kind::Sym("<*"),
                Kind::Word("y".into()),
                Kind::Sym(":<>:"),
                Kind::Sym(":=:"),
                Kind::End
            ]
        );
        // `1.E` must not swallow an enumeration reference such as `x[1].Exponent`.
        assert_eq!(kinds("1.Exponent")[0], Kind::Integer(1));
    }

    #[test]
    fn errors_carry_positions() {
        let error = tokenize("a\n  (* open").unwrap_err();
        assert_eq!(error.pos, Pos { line: 2, column: 3 });
        let error = tokenize("a\n 'open").unwrap_err();
        assert_eq!(
            error.to_string(),
            "line 2, column 2: string literal is not closed"
        );
        assert!(tokenize("a # b").is_err());
    }
}
