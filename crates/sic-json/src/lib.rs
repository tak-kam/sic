//! A JSON parser.
//!
//! A model answers with text, and turning that into a value takes a parser. It
//! is written here rather than depended on, like everything else in this
//! workspace: JSON is small, and a parser that reads untrusted output from a
//! model is exactly the kind of code worth being able to read in full.
//!
//! What it accepts is RFC 8259 and nothing else. No trailing commas, no
//! comments, no `NaN`, no duplicate keys. A model that produces those has
//! produced invalid JSON, and saying so is more useful than guessing what was
//! meant.
//!
//! The input is untrusted, so the limits below are part of the contract rather
//! than a detail: a document cannot exhaust memory or the stack.
//!
//! This crate performs no I/O and reads no clock.

/// The largest document that will be parsed.
pub const MAX_LEN: usize = 1 << 20;
/// How deeply arrays and objects may nest.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// A number that is exactly an integer.
    ///
    /// JSON has one number type, but sic has two, and deciding which one a
    /// value is at parse time is what lets `1.0` be refused where an `Int` is
    /// required instead of being quietly truncated.
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    /// Members in the order they appeared. Objects are small, and preserving
    /// the order keeps an error message pointing at the right place.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// What this value is, for a message about what was expected.
    pub fn kind(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "a boolean",
            Json::Int(_) => "an integer",
            Json::Float(_) => "a number",
            Json::Str(_) => "a string",
            Json::Array(_) => "an array",
            Json::Object(_) => "an object",
        }
    }

    pub fn member(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub message: String,
    /// Byte offset where the problem is.
    pub offset: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.offset)
    }
}

type Result<T> = std::result::Result<T, JsonError>;

pub fn parse(text: &str) -> Result<Json> {
    if text.len() > MAX_LEN {
        return Err(JsonError {
            message: format!(
                "the document is {} bytes, over the {MAX_LEN} byte limit",
                text.len()
            ),
            offset: 0,
        });
    }
    let mut p = Parser {
        bytes: text.as_bytes(),
        text,
        pos: 0,
        depth: 0,
    };
    p.skip_whitespace();
    let value = p.value()?;
    p.skip_whitespace();
    if p.pos != p.bytes.len() {
        return Err(p.error("trailing characters after the document"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, message: impl Into<String>) -> JsonError {
        JsonError {
            message: message.into(),
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        // The four the grammar allows, and no others.
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected `{}`", byte as char)))
        }
    }

    fn value(&mut self) -> Result<Json> {
        match self.peek() {
            None => Err(self.error("the document is empty")),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(byte) => Err(self.error(format!("unexpected `{}`", byte as char))),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json> {
        if self.text[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error(format!("expected `{word}`")))
        }
    }

    fn array(&mut self) -> Result<Json> {
        self.enter()?;
        self.pos += 1; // `[`
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Json::Array(items));
                }
                // A trailing comma lands here, and is refused rather than
                // tolerated.
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
    }

    fn object(&mut self) -> Result<Json> {
        self.enter()?;
        self.pos += 1; // `{`
        let mut members: Vec<(String, Json)> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_whitespace();
            let at = self.pos;
            let name = self.string()?;
            if members.iter().any(|(n, _)| *n == name) {
                // Last-wins would make the meaning depend on the order, and
                // two answers with the same keys in different orders would
                // parse differently.
                return Err(JsonError {
                    message: format!("duplicate key `{name}`"),
                    offset: at,
                });
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value = self.value()?;
            members.push((name, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Json::Object(members));
                }
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(format!("nested deeper than {MAX_DEPTH}")));
        }
        Ok(())
    }

    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("the string is not closed"));
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    self.escape(&mut out)?;
                }
                // The grammar forbids unescaped control characters.
                0x00..=0x1F => return Err(self.error("a control character must be escaped")),
                _ => {
                    let start = self.pos;
                    self.pos += 1;
                    while self.pos < self.bytes.len() && !self.text.is_char_boundary(self.pos) {
                        self.pos += 1;
                    }
                    out.push_str(&self.text[start..self.pos]);
                }
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<()> {
        let Some(byte) = self.peek() else {
            return Err(self.error("the escape is not finished"));
        };
        self.pos += 1;
        match byte {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{0008}'),
            b'f' => out.push('\u{000C}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let first = self.hex4()?;
                let ch = if (0xD800..0xDC00).contains(&first) {
                    // A high surrogate has to be followed by its low half.
                    if !self.text[self.pos..].starts_with("\\u") {
                        return Err(self.error("a high surrogate needs a low surrogate"));
                    }
                    self.pos += 2;
                    let second = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&second) {
                        return Err(self.error("expected a low surrogate"));
                    }
                    let combined = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                    char::from_u32(combined)
                } else if (0xDC00..0xE000).contains(&first) {
                    return Err(self.error("a low surrogate without a high one"));
                } else {
                    char::from_u32(first)
                };
                match ch {
                    Some(ch) => out.push(ch),
                    None => return Err(self.error("not a Unicode scalar value")),
                }
            }
            other => return Err(self.error(format!("unknown escape `\\{}`", other as char))),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.error("a `\\u` escape needs four hex digits"));
        }
        let digits = &self.text[self.pos..self.pos + 4];
        let value = u32::from_str_radix(digits, 16)
            .map_err(|_| self.error("a `\\u` escape needs four hex digits"))?;
        self.pos += 4;
        Ok(value)
    }

    fn number(&mut self) -> Result<Json> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        // A leading zero cannot be followed by more digits.
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.error("expected a digit")),
        }

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("expected a digit after `.`"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("expected a digit in the exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let text = &self.text[start..self.pos];
        if is_float {
            let value: f64 = text
                .parse()
                .map_err(|_| self.error("the number cannot be represented"))?;
            if !value.is_finite() {
                return Err(self.error("the number is too large"));
            }
            Ok(Json::Float(value))
        } else {
            match text.parse::<i64>() {
                Ok(value) => Ok(Json::Int(value)),
                // An integer too large for i64 is still a number, and a value
                // that cannot be represented is better refused than rounded.
                Err(_) => Err(self.error("the integer is outside the range of i64")),
            }
        }
    }
}

#[cfg(test)]
mod tests;
