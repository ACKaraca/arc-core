//! The lexer: source text to tokens with byte spans.

use arc_base::Result;

use crate::Span;
use crate::error::{at, codes};

/// A token kind. Text and names are read back from the source through the span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Tok {
    /// A number; `integer` when it is written with digits only.
    Number {
        value: f64,
        integer: bool,
    },
    Ident,
    /// A double-quoted text; the lexer has checked its escapes.
    Text,
    /// `'` directly after a number.
    FootMark,
    /// `"` directly after a number.
    InchMark,
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,
    AndAnd,
    OrOr,
    Bang,
    LParen,
    RParen,
    LBracket,
    RBracket,
    /// The argument and element separator.
    Sep,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// Characters that may start a name or unit besides ASCII letters and `_`.
const UNIT_START: [char; 4] = ['°', '′', '″', 'µ'];
/// Characters that may continue a name or unit besides those that start one and ASCII digits.
const UNIT_CONTINUE: [char; 3] = ['²', '³', '·'];

fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || UNIT_START.contains(&c)
}

fn ident_continue(c: char) -> bool {
    ident_start(c) || c.is_ascii_digit() || UNIT_CONTINUE.contains(&c)
}

/// Fails with `solve.expr.separator_invalid` unless `separator` can serve as a decimal separator:
/// `.`, `,`, or a character outside ASCII that is not a letter, digit, space or unit character.
pub(crate) fn check_separator(separator: char) -> Result<()> {
    let usable = matches!(separator, '.' | ',')
        || !(separator.is_ascii()
            || separator.is_alphanumeric()
            || separator.is_whitespace()
            || ident_continue(separator));
    if usable {
        Ok(())
    } else {
        Err(at(codes::EXPR_SEPARATOR_INVALID, Span::default()))
    }
}

fn char_at(text: &str, index: usize) -> Option<char> {
    text.get(index..).and_then(|rest| rest.chars().next())
}

/// The operator, bracket, separator or mark starting with `c` at `start`, with its length.
fn symbol(
    text: &str,
    start: usize,
    c: char,
    decimal: char,
    after_number: bool,
) -> Option<(Tok, usize)> {
    let next = char_at(text, start + c.len_utf8());
    let pair = |second: char, long: Tok, short: Option<Tok>| {
        if next == Some(second) {
            Some((long, 2))
        } else {
            short.map(|tok| (tok, 1))
        }
    };
    let single = |tok| Some((tok, 1));
    match c {
        '+' => single(Tok::Plus),
        '-' => single(Tok::Minus),
        '*' => single(Tok::Star),
        '/' => single(Tok::Slash),
        '^' => single(Tok::Caret),
        '(' => single(Tok::LParen),
        ')' => single(Tok::RParen),
        '[' => single(Tok::LBracket),
        ']' => single(Tok::RBracket),
        ';' => single(Tok::Sep),
        ',' if decimal != ',' => single(Tok::Sep),
        '<' => pair('=', Tok::Le, Some(Tok::Lt)),
        '>' => pair('=', Tok::Ge, Some(Tok::Gt)),
        '=' => pair('=', Tok::EqEq, None),
        '!' => pair('=', Tok::Ne, Some(Tok::Bang)),
        '&' => pair('&', Tok::AndAnd, None),
        '|' => pair('|', Tok::OrOr, None),
        '\'' if after_number => single(Tok::FootMark),
        '"' if after_number => single(Tok::InchMark),
        _ => None,
    }
}

/// The number starting at `start`, with its end.
fn number(text: &str, start: usize, decimal: char) -> Result<(Tok, usize)> {
    let digit_at = |index| char_at(text, index).is_some_and(|c| c.is_ascii_digit());
    let mut ascii = String::new();
    let (mut point, mut exponent) = (false, false);
    let mut end = start;
    while let Some(c) = char_at(text, end) {
        let after = end + c.len_utf8();
        if c.is_ascii_digit() {
            ascii.push(c);
        } else if (c == '.' || c == decimal) && !point && !exponent && digit_at(after) {
            ascii.push('.');
            point = true;
        } else if matches!(c, 'e' | 'E') && !exponent {
            let sign = char_at(text, after).filter(|s| matches!(s, '+' | '-'));
            if !(digit_at(after) || (sign.is_some() && digit_at(after + 1))) {
                break;
            }
            ascii.push('e');
            exponent = true;
            if let Some(sign) = sign {
                ascii.push(sign);
                end += 1;
            }
        } else {
            break;
        }
        end += c.len_utf8();
    }
    let value = ascii
        .parse::<f64>()
        .map_err(|_| at(codes::EXPR_SYNTAX, Span { start, end }))?;
    let integer = !(point || exponent);
    Ok((Tok::Number { value, integer }, end))
}

/// The end of the text literal whose opening quote is at `start`.
fn text_end(text: &str, start: usize) -> Result<usize> {
    let mut chars = text.get(start + 1..).unwrap_or("").char_indices();
    while let Some((i, c)) = chars.next() {
        let here = start + 1 + i;
        match c {
            '"' => return Ok(here + 1),
            '\\' => match chars.next() {
                Some((_, '"' | '\\' | 'n' | 't')) => {}
                Some((_, e)) => {
                    let end = here + 1 + e.len_utf8();
                    return Err(at(codes::EXPR_SYNTAX, Span { start: here, end }));
                }
                None => break,
            },
            _ => {}
        }
    }
    let end = text.len();
    Err(at(codes::EXPR_SYNTAX, Span { start, end }))
}

/// Splits `text` into tokens, ending with [`Tok::End`]. `decimal` is a decimal separator accepted
/// in addition to `.`; when it is `,`, only `;` separates arguments.
pub(crate) fn lex(text: &str, decimal: char) -> Result<Vec<Token>> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut start = 0;
    while let Some(c) = char_at(text, start) {
        if c.is_whitespace() {
            start += c.len_utf8();
            continue;
        }
        let after_number = matches!(
            tokens.last(),
            Some(Token { tok: Tok::Number { .. }, span }) if span.end == start
        );
        let digit_next = char_at(text, start + c.len_utf8()).is_some_and(|n| n.is_ascii_digit());
        let (tok, end) = if let Some((tok, len)) = symbol(text, start, c, decimal, after_number) {
            (tok, start + len)
        } else if c.is_ascii_digit() || ((c == '.' || c == decimal) && digit_next) {
            number(text, start, decimal)?
        } else if ident_start(c) {
            let rest = text.get(start..).unwrap_or("");
            let len = rest.find(|d| !ident_continue(d)).unwrap_or(rest.len());
            (Tok::Ident, start + len)
        } else if c == '"' {
            (Tok::Text, text_end(text, start)?)
        } else {
            let end = start + c.len_utf8();
            return Err(at(codes::EXPR_SYNTAX, Span { start, end }));
        };
        tokens.push(Token {
            tok,
            span: Span { start, end },
        });
        start = end;
    }
    let end = Span {
        start: text.len(),
        end: text.len(),
    };
    tokens.push(Token {
        tok: Tok::End,
        span: end,
    });
    Ok(tokens)
}

/// The content of a text token's source, quotes included, with escapes resolved.
pub(crate) fn decode_text(raw: &str) -> String {
    let inner = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(raw);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Tok> {
        lex(text, '.').unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn number(value: f64, integer: bool) -> Tok {
        Tok::Number { value, integer }
    }

    #[test]
    fn numbers_take_decimals_and_exponents() {
        assert_eq!(kinds("12"), [number(12.0, true), Tok::End]);
        assert_eq!(kinds("1.5e-3"), [number(1.5e-3, false), Tok::End]);
        assert_eq!(kinds(".5E2"), [number(50.0, false), Tok::End]);
        // An `e` without exponent digits starts a name.
        assert_eq!(kinds("2e"), [number(2.0, true), Tok::Ident, Tok::End]);
        assert_eq!(kinds("2em"), [number(2.0, true), Tok::Ident, Tok::End]);
        // A second point starts another number.
        assert_eq!(
            kinds("1.2.3"),
            [number(1.2, false), number(0.3, false), Tok::End]
        );
        assert_eq!(kinds("1e+3"), [number(1000.0, false), Tok::End]);
        assert_eq!(kinds("1e+x")[0], number(1.0, true));
    }

    #[test]
    fn marks_follow_numbers_and_quotes_elsewhere_start_text() {
        assert_eq!(
            kinds("8'6\""),
            [
                number(8.0, true),
                Tok::FootMark,
                number(6.0, true),
                Tok::InchMark,
                Tok::End
            ]
        );
        assert_eq!(kinds("\"a\\\"b\""), [Tok::Text, Tok::End]);
        assert_eq!(decode_text("\"a\\\"b\\n\""), "a\"b\n");
        assert!(lex("'a'", '.').is_err());
        assert!(lex("\"open", '.').is_err());
        assert!(lex("\"bad \\q\"", '.').is_err());
    }

    #[test]
    fn comma_decimal_separator_moves_arguments_to_semicolons() {
        assert_eq!(lex("1,5;2", ',').unwrap()[0].tok, number(1.5, false));
        assert_eq!(kinds("1,5")[1], Tok::Sep);
        assert!(lex("a,b", ',').is_err());
        assert_eq!(lex("0٫25", '٫').unwrap()[0].tok, number(0.25, false));
    }

    #[test]
    fn operators_and_unit_characters() {
        assert_eq!(
            kinds("a<=b!=c&&!d||e==f"),
            [
                Tok::Ident,
                Tok::Le,
                Tok::Ident,
                Tok::Ne,
                Tok::Ident,
                Tok::AndAnd,
                Tok::Bang,
                Tok::Ident,
                Tok::OrOr,
                Tok::Ident,
                Tok::EqEq,
                Tok::Ident,
                Tok::End
            ]
        );
        assert_eq!(kinds("90°"), [number(90.0, true), Tok::Ident, Tok::End]);
        assert_eq!(kinds("ft·lbf m²"), [Tok::Ident, Tok::Ident, Tok::End]);
        assert!(lex("a = b", '.').is_err());
        assert!(lex("a & b", '.').is_err());
        assert!(lex("#", '.').is_err());
    }

    #[test]
    fn separators_are_checked() {
        for ok in ['.', ',', '٫'] {
            assert!(check_separator(ok).is_ok());
        }
        for bad in ['-', 'a', ' ', '·', '1', ';'] {
            assert!(check_separator(bad).is_err(), "{bad:?}");
        }
    }
}
