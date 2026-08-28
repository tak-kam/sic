//! A hand-written lexer.
//!
//! Rules:
//! - The only accepted whitespace is space, tab, CR and LF. Other Unicode
//!   whitespace is rejected so that characters which look alike but are not
//!   cannot slip into a program.
//! - Identifiers are ASCII only. Non-ASCII text is allowed inside string
//!   literals and comments, nowhere else.
//! - Errors do not stop lexing: diagnostics are collected and scanning resumes.

use sic_core::{Diagnostic, Label, Span};

use crate::token::{Keyword, Token, TokenKind};

/// Turns a whole source text into tokens. The result always ends with `Eof`.
pub fn tokenize(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    tokenize_at(src, 0)
}

/// The same, for a file that sits at `base` in a `SourceMap`'s offset space.
///
/// Positions inside the lexer stay relative to this file; only the spans it
/// hands out are shifted. That is what lets a `Span` remain a range of bytes
/// while a program spans several files.
pub fn tokenize_at(src: &str, base: u32) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lx = Lexer::new(src, base);
    let tokens = lx.run();
    (tokens, lx.diags)
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: u32,
    /// Where this file begins in the shared offset space.
    base: u32,
    diags: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, base: u32) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            base,
            diags: Vec::new(),
        }
    }

    /// A span in the shared offset space.
    fn span(&self, lo: u32, hi: u32) -> Span {
        Span::new(lo + self.base, hi + self.base)
    }

    fn run(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(b) = self.peek() else {
                let at = self.span(self.pos, self.pos);
                tokens.push(Token::new(TokenKind::Eof, at));
                return tokens;
            };
            let kind = match b {
                b'0'..=b'9' => self.number(),
                b'"' => self.string(),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.ident_or_keyword(),
                _ => match self.punct() {
                    Some(k) => k,
                    None => continue, // already reported; resume at the next character
                },
            };
            let span = self.span(start, self.pos);
            tokens.push(Token::new(kind, span));
        }
    }

    // ---- character level helpers ----

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos as usize).copied()
    }

    fn peek_at(&self, n: u32) -> Option<u8> {
        self.bytes.get((self.pos + n) as usize).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    /// Advances to the next character boundary so the position never lands in
    /// the middle of a UTF-8 sequence.
    fn advance_char(&mut self) {
        self.pos += 1;
        while self.pos < self.src.len() as u32 && !self.src.is_char_boundary(self.pos as usize) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn error(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, msg, Label::new(span, label)));
    }

    // ---- whitespace and comments ----

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.pos += 1;
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => self.block_comment(),
                _ => return,
            }
        }
    }

    /// Block comments nest.
    fn block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek() {
                None => {
                    self.error(
                        "E0104",
                        "unterminated block comment",
                        self.span(start, start + 2),
                        "no matching `*/`",
                    );
                    return;
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    self.pos += 2;
                    depth += 1;
                }
                Some(b'*') if self.peek_at(1) == Some(b'/') => {
                    self.pos += 2;
                    depth -= 1;
                }
                _ => self.advance_char(),
            }
        }
    }

    // ---- numbers ----

    fn number(&mut self) -> TokenKind {
        let start = self.pos;
        self.eat_digits();

        // A `.` starts a fraction only when a digit follows; `1.foo` is a field
        // access.
        let mut is_float = false;
        if self.peek() == Some(b'.') && matches!(self.peek_at(1), Some(b'0'..=b'9')) {
            is_float = true;
            self.pos += 1;
            self.eat_digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let next = self.peek_at(1);
            let has_exp = match next {
                Some(b'0'..=b'9') => true,
                Some(b'+' | b'-') => matches!(self.peek_at(2), Some(b'0'..=b'9')),
                _ => false,
            };
            if has_exp {
                is_float = true;
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                self.eat_digits();
            }
        }

        let span = self.span(start, self.pos);
        let raw: String = self.src[start as usize..self.pos as usize]
            .chars()
            .filter(|c| *c != '_')
            .collect();

        if is_float {
            match raw.parse::<f64>() {
                Ok(v) if v.is_finite() => TokenKind::Float(v),
                _ => {
                    self.error(
                        "E0102",
                        "float literal is out of range",
                        span,
                        "does not fit in f64",
                    );
                    TokenKind::Float(0.0)
                }
            }
        } else {
            match raw.parse::<i64>() {
                Ok(v) => TokenKind::Int(v),
                Err(_) => {
                    self.error(
                        "E0102",
                        "integer literal is out of range",
                        span,
                        "does not fit in i64",
                    );
                    TokenKind::Int(0)
                }
            }
        }
    }

    fn eat_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9' | b'_')) {
            self.pos += 1;
        }
    }

    // ---- strings ----

    fn string(&mut self) -> TokenKind {
        let start = self.pos;
        self.pos += 1; // opening quote
        let mut value = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => {
                    self.error(
                        "E0103",
                        "unterminated string literal",
                        self.span(start, self.pos),
                        "no closing `\"`",
                    );
                    return TokenKind::Str(value);
                }
                Some(b'"') => {
                    self.pos += 1;
                    return TokenKind::Str(value);
                }
                Some(b'\\') => self.escape(&mut value),
                _ => {
                    let from = self.pos as usize;
                    self.advance_char();
                    value.push_str(&self.src[from..self.pos as usize]);
                }
            }
        }
    }

    fn escape(&mut self, out: &mut String) {
        let start = self.pos;
        self.pos += 1; // backslash
        let Some(b) = self.bump() else {
            self.error(
                "E0103",
                "escape sequence ends abruptly",
                self.span(start, self.pos),
                "",
            );
            return;
        };
        match b {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'n' => out.push('\n'),
            b't' => out.push('\t'),
            b'r' => out.push('\r'),
            b'0' => out.push('\0'),
            b'u' => self.unicode_escape(start, out),
            // A backslash at the end of a line joins it to the next one, and
            // eats the whitespace that indents it. Nothing else in this
            // language lets a long string be written down: two strings cannot
            // be joined, so without this a command or a prompt is one physical
            // line however long it is - `workflows/ci.sic` had one of 286
            // characters in a repository that wraps its own Rust at 80.
            //
            // Whitespace *including* further newlines, which is what makes a
            // blank line inside a continued string mean nothing. A leading
            // space that has to survive is `\u{20}`.
            b'\r' | b'\n' => {
                if b == b'\r' && self.peek() == Some(b'\n') {
                    self.pos += 1;
                }
                while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    self.pos += 1;
                }
            }
            _ => {
                self.error(
                    "E0105",
                    "unknown escape sequence",
                    self.span(start, self.pos),
                    "the escapes are \\\" \\\\ \\n \\t \\r \\0 \\u{...}, \
                     and a backslash at the end of a line",
                );
            }
        }
    }

    fn unicode_escape(&mut self, start: u32, out: &mut String) {
        if !self.eat(b'{') {
            self.error(
                "E0105",
                "`\\u` must be followed by `{`",
                self.span(start, self.pos),
                "",
            );
            return;
        }
        let digits_start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F')) {
            self.pos += 1;
        }
        let digits = &self.src[digits_start as usize..self.pos as usize];
        let ok = !digits.is_empty() && digits.len() <= 6 && self.eat(b'}');
        if !ok {
            self.error(
                "E0105",
                "malformed `\\u{...}` escape",
                self.span(start, self.pos),
                "expected 1 to 6 hex digits inside `{}`",
            );
            return;
        }
        match u32::from_str_radix(digits, 16)
            .ok()
            .and_then(char::from_u32)
        {
            Some(c) => out.push(c),
            None => self.error(
                "E0105",
                "not a valid Unicode scalar value",
                self.span(start, self.pos),
                "surrogates and values above 0x10FFFF are not allowed",
            ),
        }
    }

    // ---- identifiers ----

    fn ident_or_keyword(&mut self) -> TokenKind {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        ) {
            self.pos += 1;
        }
        let text = &self.src[start as usize..self.pos as usize];
        match Keyword::from_ident(text) {
            Some(kw) => TokenKind::Kw(kw),
            None => TokenKind::Ident(text.to_string()),
        }
    }

    // ---- punctuation ----

    /// Reads one punctuation token, or reports the character and returns `None`.
    fn punct(&mut self) -> Option<TokenKind> {
        let start = self.pos;
        let b = self.bump()?;
        let kind = match b {
            b'+' => TokenKind::Plus,
            b'-' if self.eat(b'>') => TokenKind::Arrow,
            b'-' => TokenKind::Minus,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,
            b'!' if self.eat(b'=') => TokenKind::BangEq,
            b'!' => TokenKind::Bang,
            b'=' if self.eat(b'=') => TokenKind::EqEq,
            b'=' => TokenKind::Eq,
            b'<' if self.eat(b'=') => TokenKind::Le,
            b'<' => TokenKind::Lt,
            b'>' if self.eat(b'=') => TokenKind::Ge,
            b'>' => TokenKind::Gt,
            b'&' if self.eat(b'&') => TokenKind::AmpAmp,
            b'|' if self.eat(b'|') => TokenKind::PipePipe,
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b',' => TokenKind::Comma,
            b':' => TokenKind::Colon,
            b';' => TokenKind::Semi,
            b'.' if self.eat(b'.') => TokenKind::DotDot,
            b'.' => TokenKind::Dot,
            b'?' => TokenKind::Question,
            _ => {
                // Rewind and advance by a whole character so the span covers one
                // character rather than one byte.
                self.pos = start;
                self.advance_char();
                let span = self.span(start, self.pos);
                let text = &self.src[start as usize..self.pos as usize];
                let (msg, hint): (String, String) = match b {
                    b'&' => (
                        "`&` is not an operator".into(),
                        "logical and is `&&`".into(),
                    ),
                    b'|' => ("`|` is not an operator".into(), "logical or is `||`".into()),
                    _ if !b.is_ascii() => (
                        format!("`{text}` cannot be used here"),
                        "non-ASCII characters are allowed only in strings and comments".into(),
                    ),
                    _ => (format!("unexpected character `{text}`"), String::new()),
                };
                self.error("E0101", msg, span, hint);
                return None;
            }
        };
        Some(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, diags) = tokenize(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {:?}", diags);
        toks.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn milestone_program() {
        let src = "fn main() {\n    let x = 10;\n    let y = x + 20;\n    return y;\n}\n";
        let k = kinds(src);
        assert_eq!(k.first(), Some(&TokenKind::Kw(Keyword::Fn)));
        assert_eq!(k.last(), Some(&TokenKind::Eof));
        assert!(k.contains(&TokenKind::Int(20)));
        assert!(k.contains(&TokenKind::Ident("main".into())));
    }

    #[test]
    fn multi_char_operators() {
        let k = kinds("== != <= >= && || -> = < > ! -");
        assert_eq!(
            k,
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::Le,
                TokenKind::Ge,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::Arrow,
                TokenKind::Eq,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Bang,
                TokenKind::Minus,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("1_000")[0], TokenKind::Int(1000));
        assert_eq!(kinds("1.5")[0], TokenKind::Float(1.5));
        assert_eq!(kinds("1e3")[0], TokenKind::Float(1000.0));
        assert_eq!(kinds("1.5e-3")[0], TokenKind::Float(0.0015));
        // `1.foo` is a field access, not a float.
        assert_eq!(
            kinds("1.foo"),
            vec![
                TokenKind::Int(1),
                TokenKind::Dot,
                TokenKind::Ident("foo".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn integer_overflow_is_reported() {
        let (_, diags) = tokenize("9223372036854775808");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, Some("E0102"));
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(
            kinds(r#""あ\n\t\u{1F600}\"""#)[0],
            TokenKind::Str("あ\n\t\u{1F600}\"".into())
        );
        let (_, d) = tokenize(r#""unterminated"#);
        assert_eq!(d[0].code, Some("E0103"));
        let (_, d) = tokenize(r#""bad \q escape""#);
        assert_eq!(d[0].code, Some("E0105"));
    }

    /// A backslash at the end of a line joins it to the next and eats the
    /// whitespace that indents it, so a long literal can be written down.
    #[test]
    fn a_backslash_at_the_end_of_a_line_joins_it_to_the_next() {
        assert_eq!(
            kinds("\"one \\\n            two\"")[0],
            TokenKind::Str("one two".into())
        );
        // The space before the backslash is part of the string; the
        // indentation after it is not.
        assert_eq!(
            kinds("\"one\\\n            two\"")[0],
            TokenKind::Str("onetwo".into())
        );
        // Whitespace including further newlines, so a blank line inside a
        // continued string means nothing.
        assert_eq!(
            kinds("\"one \\\n\n     two\"")[0],
            TokenKind::Str("one two".into())
        );
        // A leading space that has to survive is written as an escape.
        assert_eq!(
            kinds("\"one\\\n     \\u{20}two\"")[0],
            TokenKind::Str("one two".into())
        );
        // A file written with CRLF joins the same way.
        assert_eq!(
            kinds("\"one \\\r\n            two\"")[0],
            TokenKind::Str("one two".into())
        );
        // And a string still cannot simply run past the end of its line.
        let (_, d) = tokenize("\"one\n            two\"");
        assert_eq!(d[0].code, Some("E0103"));
    }

    #[test]
    fn comments_nest() {
        assert_eq!(
            kinds("1 /* a /* b */ c */ 2"),
            vec![TokenKind::Int(1), TokenKind::Int(2), TokenKind::Eof]
        );
        assert_eq!(
            kinds("1 // to end of line\n2"),
            vec![TokenKind::Int(1), TokenKind::Int(2), TokenKind::Eof]
        );
        let (_, d) = tokenize("/* never closed");
        assert_eq!(d[0].code, Some("E0104"));
    }

    #[test]
    fn reserved_words_lex_as_keywords() {
        assert_eq!(
            kinds("parallel")[0],
            TokenKind::Kw(Keyword::Reserved("parallel"))
        );
        // A word that has since become real lexes as itself.
        assert_eq!(kinds("agent")[0], TokenKind::Kw(Keyword::Agent));
        assert_eq!(kinds("import")[0], TokenKind::Kw(Keyword::Import));
        // An identifier that merely starts with a keyword stays an identifier.
        assert_eq!(kinds("agent_id")[0], TokenKind::Ident("agent_id".into()));
    }

    #[test]
    fn non_ascii_outside_string_is_rejected_and_recovers() {
        let (toks, diags) = tokenize("let あ = 1;");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, Some("E0101"));
        // Only the offending character is dropped; lexing continues.
        assert!(toks.iter().any(|t| t.kind == TokenKind::Int(1)));
        assert_eq!(toks.last().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn spans_point_at_source() {
        let src = "let x = 42;";
        let (toks, _) = tokenize(src);
        let t = toks.iter().find(|t| t.kind == TokenKind::Int(42)).unwrap();
        assert_eq!(&src[t.span.lo as usize..t.span.hi as usize], "42");
    }
}
