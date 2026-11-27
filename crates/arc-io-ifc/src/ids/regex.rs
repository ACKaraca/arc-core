//! The pattern language of XML Schema (`xs:pattern`), the subset the IDS restrictions use:
//! alternation, groups, the quantifiers `? * + {n} {n,} {n,m}`, `.`, character classes with
//! ranges, negation and subtraction, the escapes `\d \D \s \S \w \W \i \I \c \C \p{..} \P{..}` and
//! the single-character escapes. A pattern matches a whole string. Matching is by backtracking
//! with a step budget, so a hostile pattern cannot run unbounded.
//!
//! Character categories (`\p{L}`, `\p{Lu}`, `\p{N}`, `\p{P}`, `\p{S}`, `\p{Z}` and their
//! two-letter forms for the common ones) use the Unicode properties of the standard library, and
//! the blocks `IsBasicLatin` and `IsLatin-1Supplement`; other categories are refused when the
//! pattern is compiled.

/// The most characters of a pattern that are compiled.
const MAX_PATTERN: usize = 4096;
/// The most steps of one match before it is abandoned and fails.
const STEP_BUDGET: u32 = 2_000_000;
/// The most repetitions one `{n,m}` quantifier allows.
const MAX_REPEAT: u32 = 1000;
/// The deepest nesting of groups.
const MAX_DEPTH: u32 = 64;

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Char(char),
    Range(char, char),
    /// A shorthand or category escape, and whether it is negated.
    Short(Short, bool),
}

#[derive(Debug, Clone, PartialEq)]
enum Short {
    Digit,
    Space,
    Word,
    NameStart,
    NameChar,
    Category(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Class {
    negated: bool,
    items: Vec<Item>,
    subtract: Option<Box<Class>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Any,
    Char(char),
    Class(Class),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, u32, Option<u32>),
}

/// A compiled pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    root: Node,
}

/// Why a pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub &'static str);

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

type ParseResult<T> = Result<T, PatternError>;

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn alternation(&mut self, depth: u32) -> ParseResult<Node> {
        if depth > MAX_DEPTH {
            return Err(PatternError("pattern nesting is too deep"));
        }
        let mut branches = vec![self.sequence(depth)?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.sequence(depth)?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Node::Alt(branches)
        })
    }

    fn sequence(&mut self, depth: u32) -> ParseResult<Node> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom(depth)?;
            items.push(self.quantified(atom)?);
        }
        Ok(Node::Seq(items))
    }

    fn quantified(&mut self, atom: Node) -> ParseResult<Node> {
        let (min, max) = match self.peek() {
            Some('?') => (0, Some(1)),
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('{') => {
                self.pos += 1;
                let min = self.number()?;
                let max = if self.peek() == Some(',') {
                    self.pos += 1;
                    if self.peek() == Some('}') {
                        None
                    } else {
                        Some(self.number()?)
                    }
                } else {
                    Some(min)
                };
                if self.peek() != Some('}') {
                    return Err(PatternError("malformed quantifier"));
                }
                if max.is_some_and(|m| m < min)
                    || min > MAX_REPEAT
                    || max.is_some_and(|m| m > MAX_REPEAT)
                {
                    return Err(PatternError("quantifier out of range"));
                }
                (min, max)
            }
            _ => return Ok(atom),
        };
        self.pos += 1;
        // A second quantifier on the same atom is not in the language.
        if matches!(self.peek(), Some('?' | '*' | '+' | '{')) {
            return Err(PatternError("repeated quantifier"));
        }
        Ok(Node::Repeat(Box::new(atom), min, max))
    }

    fn number(&mut self) -> ParseResult<u32> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(PatternError("malformed quantifier"));
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse()
            .map_err(|_| PatternError("quantifier out of range"))
    }

    fn atom(&mut self, depth: u32) -> ParseResult<Node> {
        match self.next() {
            Some('(') => {
                if self.peek() == Some('?') {
                    return Err(PatternError(
                        "group extensions are not in the pattern language",
                    ));
                }
                let inner = self.alternation(depth + 1)?;
                if self.next() != Some(')') {
                    return Err(PatternError("unbalanced parenthesis"));
                }
                Ok(inner)
            }
            Some('.') => Ok(Node::Any),
            Some('[') => Ok(Node::Class(self.class()?)),
            Some('\\') => self.escape().map(|item| match item {
                Item::Char(c) => Node::Char(c),
                other => Node::Class(Class {
                    negated: false,
                    items: vec![other],
                    subtract: None,
                }),
            }),
            Some('*' | '+' | '?' | '{') => Err(PatternError("quantifier without an atom")),
            Some(c) => Ok(Node::Char(c)),
            None => Err(PatternError("unexpected end of pattern")),
        }
    }

    /// The escape after a backslash.
    fn escape(&mut self) -> ParseResult<Item> {
        let c = self.next().ok_or(PatternError("dangling backslash"))?;
        let single = |c| Ok(Item::Char(c));
        match c {
            'n' => single('\n'),
            'r' => single('\r'),
            't' => single('\t'),
            '\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}' | '(' | ')' | '[' | ']' => {
                single(c)
            }
            'd' | 'D' | 's' | 'S' | 'w' | 'W' | 'i' | 'I' | 'c' | 'C' => Ok(Item::Short(
                match c.to_ascii_lowercase() {
                    'd' => Short::Digit,
                    's' => Short::Space,
                    'w' => Short::Word,
                    'i' => Short::NameStart,
                    _ => Short::NameChar,
                },
                c.is_ascii_uppercase(),
            )),
            'p' | 'P' => {
                if self.next() != Some('{') {
                    return Err(PatternError("malformed category escape"));
                }
                let mut name = String::new();
                loop {
                    match self.next() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => return Err(PatternError("malformed category escape")),
                    }
                }
                if !known_category(&name) {
                    return Err(PatternError("unsupported character category"));
                }
                Ok(Item::Short(Short::Category(name), c == 'P'))
            }
            // The language lists the escapes above; a backslash before any other punctuation
            // character is taken as that character, as common implementations do.
            c if c.is_ascii_punctuation() => single(c),
            _ => Err(PatternError("unknown escape")),
        }
    }

    fn class(&mut self) -> ParseResult<Class> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut items = Vec::new();
        let mut subtract = None;
        loop {
            let Some(c) = self.next() else {
                return Err(PatternError("unterminated character class"));
            };
            match c {
                ']' => break,
                '[' => return Err(PatternError("nested class outside a subtraction")),
                '-' if self.peek() == Some('[') => {
                    self.pos += 1;
                    subtract = Some(Box::new(self.class()?));
                    if self.next() != Some(']') {
                        return Err(PatternError("a subtraction ends the class"));
                    }
                    break;
                }
                _ => {
                    let first = if c == '\\' {
                        self.escape()?
                    } else {
                        Item::Char(c)
                    };
                    if let Item::Char(lo) = first
                        && self.peek() == Some('-')
                        && self
                            .chars
                            .get(self.pos + 1)
                            .is_some_and(|n| *n != ']' && *n != '[')
                    {
                        self.pos += 1;
                        let hi = match self.next() {
                            Some('\\') => match self.escape()? {
                                Item::Char(h) => h,
                                _ => return Err(PatternError("malformed range")),
                            },
                            Some(h) => h,
                            None => return Err(PatternError("unterminated character class")),
                        };
                        if hi < lo {
                            return Err(PatternError("range is out of order"));
                        }
                        items.push(Item::Range(lo, hi));
                    } else {
                        items.push(first);
                    }
                }
            }
        }
        Ok(Class {
            negated,
            items,
            subtract,
        })
    }
}

fn known_category(name: &str) -> bool {
    matches!(
        name,
        "L" | "Lu"
            | "Ll"
            | "Lt"
            | "Lm"
            | "Lo"
            | "N"
            | "Nd"
            | "Nl"
            | "No"
            | "P"
            | "S"
            | "Z"
            | "Zs"
            | "IsBasicLatin"
            | "IsLatin-1Supplement"
    )
}

fn category(name: &str, c: char) -> bool {
    match name {
        "L" | "Lt" | "Lm" | "Lo" => c.is_alphabetic(),
        "Lu" => c.is_uppercase(),
        "Ll" => c.is_lowercase(),
        "N" | "Nl" | "No" => c.is_numeric(),
        "Nd" => c.is_ascii_digit(),
        "P" => is_punctuation(c),
        "S" => "$+<=>^`|~".contains(c),
        "Z" | "Zs" => c == ' ' || c == '\u{a0}' || (c.is_whitespace() && !c.is_control()),
        "IsBasicLatin" => c <= '\u{7f}',
        "IsLatin-1Supplement" => ('\u{80}'..='\u{ff}').contains(&c),
        _ => false,
    }
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)
}

fn shorthand(short: &Short, c: char) -> bool {
    match short {
        Short::Digit => c.is_ascii_digit(),
        Short::Space => matches!(c, ' ' | '\t' | '\n' | '\r'),
        Short::Word => !(is_punctuation(c) || c.is_whitespace() || c.is_control()),
        Short::NameStart => c.is_alphabetic() || c == '_' || c == ':',
        Short::NameChar => c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'),
        Short::Category(name) => category(name, c),
    }
}

impl Class {
    fn contains(&self, c: char) -> bool {
        let inside = self.items.iter().any(|item| match item {
            Item::Char(x) => *x == c,
            Item::Range(a, b) => (*a..=*b).contains(&c),
            Item::Short(short, negated) => shorthand(short, c) != *negated,
        });
        let kept = inside && !self.subtract.as_ref().is_some_and(|s| s.contains(c));
        kept != self.negated
    }
}

struct Matcher<'a> {
    text: &'a [char],
    steps: u32,
}

impl Matcher<'_> {
    fn node(
        &mut self,
        node: &Node,
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            return false;
        }
        match node {
            Node::Any => match self.text.get(pos) {
                Some(c) if *c != '\n' && *c != '\r' => k(self, pos + 1),
                _ => false,
            },
            Node::Char(x) => self.text.get(pos) == Some(x) && k(self, pos + 1),
            Node::Class(class) => {
                self.text.get(pos).is_some_and(|c| class.contains(*c)) && k(self, pos + 1)
            }
            Node::Seq(items) => self.seq(items, pos, k),
            Node::Alt(branches) => branches.iter().any(|b| self.node(b, pos, k)),
            Node::Repeat(inner, min, max) => self.repeat(inner, *min, *max, 0, pos, k),
        }
    }

    fn seq(
        &mut self,
        items: &[Node],
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        match items.split_first() {
            None => k(self, pos),
            Some((first, rest)) => self.node(first, pos, &mut |m: &mut Self, p| m.seq(rest, p, k)),
        }
    }

    fn repeat(
        &mut self,
        inner: &Node,
        min: u32,
        max: Option<u32>,
        count: u32,
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        if max.is_none_or(|m| count < m)
            && self.node(inner, pos, &mut |m: &mut Self, p| {
                // An iteration that consumes nothing cannot make progress.
                p != pos && m.repeat(inner, min, max, count + 1, p, k)
            })
        {
            return true;
        }
        count >= min && k(self, pos)
    }
}

impl Pattern {
    /// Compiles a pattern.
    ///
    /// # Errors
    ///
    /// A [`PatternError`] for a construct that is not in the subset, an unbalanced bracket or a
    /// pattern longer than 4096 characters.
    pub fn new(source: &str) -> Result<Self, PatternError> {
        if source.chars().count() > MAX_PATTERN {
            return Err(PatternError("pattern is too long"));
        }
        let mut parser = Parser {
            chars: source.chars().collect(),
            pos: 0,
        };
        let root = parser.alternation(0)?;
        if parser.pos < parser.chars.len() {
            return Err(PatternError("unbalanced parenthesis"));
        }
        Ok(Self { root })
    }

    /// Whether the whole of `text` matches. A match that exceeds the step budget fails.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut matcher = Matcher {
            text: &chars,
            steps: 0,
        };
        matcher.node(&self.root, 0, &mut |_, p| p == len)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    fn m(pattern: &str, text: &str) -> bool {
        Pattern::new(pattern).unwrap().is_match(text)
    }

    #[test]
    fn matches_the_whole_string() {
        assert!(m("DT[0-9]{2}", "DT01"));
        assert!(!m("DT[0-9]{2}", "DT012"));
        assert!(!m("DT", "xDT"));
        assert!(m(".*DT.*", "ABCDT01"));
        assert!(m("a|bc", "bc") && !m("a|bc", "ab"));
        assert!(m("(ab)+c?", "ababc") && m("(ab)+c?", "ab") && !m("(ab)+c?", "a"));
        assert!(m("x{2,}", "xxxx") && !m("x{2,}", "x") && m("x{0,1}", ""));
    }

    #[test]
    fn classes_and_escapes() {
        assert!(m("[a-c]+", "abcba") && !m("[a-c]+", "abd"));
        assert!(m("[^0-9]+", "ab") && !m("[^0-9]+", "a1"));
        assert!(m("[a-z-[aeiou]]+", "bcd") && !m("[a-z-[aeiou]]+", "bad"));
        assert!(m(r"\d+\.\d+", "3.14") && !m(r"\d+\.\d+", "3x14"));
        assert!(m(r"\w+", "Ab1") && !m(r"\w+", "a b") && !m(r"\w+", "a_b"));
        assert!(m(r"\p{Lu}\p{Ll}+", "Hello") && !m(r"\p{Lu}\p{Ll}+", "hello"));
        assert!(m(r"\P{N}+", "abc") && !m(r"\P{N}+", "a1"));
        assert!(m(r"(-|[0-9]{2,3})\/(-|[0-9]{2,3})", "-/120"));
        assert!(m(".", "x") && !m(".", "\n"));
        assert!(m(r"\s\S", " x") && m(r"[\-a]", "-"));
    }

    #[test]
    fn refuses_what_is_not_in_the_language() {
        for bad in [
            "(?:a)", "a**", "(a", "a)", "[a", "[z-a]", r"\q", "{2}", r"\p{Foo}", "a{3,2}",
            "a{1001}",
        ] {
            assert!(Pattern::new(bad).is_err(), "{bad}");
        }
        assert!(Pattern::new(&"a".repeat(5000)).is_err());
        assert!(Pattern::new(&format!("{}a{}", "(".repeat(100), ")".repeat(100))).is_err());
    }

    #[test]
    fn a_catastrophic_pattern_stops_at_the_budget() {
        let p = Pattern::new("(a+)+b").unwrap();
        let start = std::time::Instant::now();
        assert!(!p.is_match(&"a".repeat(60)));
        assert!(start.elapsed().as_secs() < 10);
    }
}
