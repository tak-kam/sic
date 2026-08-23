//! Token definitions.
//!
//! Literal values are computed by the lexer and carried in the token, so the
//! parser never has to reinterpret the source text.

use sic_core::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Int(i64),
    Float(f64),
    Str(String),

    Ident(String),
    Kw(Keyword),

    // Punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Eq,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    AmpAmp,
    PipePipe,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Dot,
    Arrow,

    Eof,
}

impl TokenKind {
    /// How the token is named in a diagnostic: its kind, not its value.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(_) => "an integer literal".into(),
            TokenKind::Float(_) => "a float literal".into(),
            TokenKind::Str(_) => "a string literal".into(),
            TokenKind::Ident(n) => format!("identifier `{n}`"),
            TokenKind::Kw(k) => format!("`{}`", k.text()),
            TokenKind::Eof => "end of file".into(),
            other => format!("`{}`", other.punct_text()),
        }
    }

    fn punct_text(&self) -> &'static str {
        match self {
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::Bang => "!",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::AmpAmp => "&&",
            TokenKind::PipePipe => "||",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Colon => ":",
            TokenKind::Semi => ";",
            TokenKind::Dot => ".",
            TokenKind::Arrow => "->",
            _ => "?",
        }
    }
}

/// Keywords that mean something in v0.1, plus words reserved for later.
///
/// Reserving words now is what lets `agent`, `parallel`, `retry` and friends be
/// added later without breaking existing programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    // In use in v0.1
    Fn,
    Let,
    Return,
    If,
    Else,
    True,
    False,
    Null,
    Allow,
    Spawn,
    Await,
    Type,
    Agent,
    Import,
    Requires,
    Retry,
    Timeout,
    /// Reserved only. Using one produces a diagnostic.
    Reserved(&'static str),
}

/// Words planned for later phases. They cannot be used as identifiers today.
///
/// `process` and `budget` are deliberately absent: the first is the namespace
/// of the `process.exec` capability, the second is a setting inside an `agent`
/// body, and both have to lex as ordinary identifiers.
const RESERVED: &[&str] = &[
    "approval",
    "as",
    "capability",
    "const",
    "enum",
    "for",
    "in",
    "loop",
    "match",
    "mut",
    "parallel",
    "secret",
    "struct",
    "trust",
    "use",
    "while",
    "workflow",
    "yield",
];

impl Keyword {
    pub fn from_ident(s: &str) -> Option<Keyword> {
        Some(match s {
            "fn" => Keyword::Fn,
            "let" => Keyword::Let,
            "return" => Keyword::Return,
            "if" => Keyword::If,
            "else" => Keyword::Else,
            "true" => Keyword::True,
            "false" => Keyword::False,
            "null" => Keyword::Null,
            "allow" => Keyword::Allow,
            "type" => Keyword::Type,
            "agent" => Keyword::Agent,
            "import" => Keyword::Import,
            "requires" => Keyword::Requires,
            "spawn" => Keyword::Spawn,
            "await" => Keyword::Await,
            "retry" => Keyword::Retry,
            "timeout" => Keyword::Timeout,
            other => {
                let found = RESERVED.iter().find(|r| **r == other)?;
                Keyword::Reserved(found)
            }
        })
    }

    pub fn text(self) -> &'static str {
        match self {
            Keyword::Fn => "fn",
            Keyword::Let => "let",
            Keyword::Return => "return",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::True => "true",
            Keyword::False => "false",
            Keyword::Null => "null",
            Keyword::Allow => "allow",
            Keyword::Type => "type",
            Keyword::Agent => "agent",
            Keyword::Import => "import",
            Keyword::Requires => "requires",
            Keyword::Spawn => "spawn",
            Keyword::Await => "await",
            Keyword::Retry => "retry",
            Keyword::Timeout => "timeout",
            Keyword::Reserved(s) => s,
        }
    }

    pub fn is_reserved(self) -> bool {
        matches!(self, Keyword::Reserved(_))
    }
}
