//! Source positions.
//!
//! A `Span` is a half-open byte range `[lo, hi)`. Translating it into a line and
//! column is the job of `SourceFile`; the AST stores spans only.

/// A byte range `[lo, hi)` in a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    pub const fn new(lo: u32, hi: u32) -> Self {
        debug_assert!(lo <= hi);
        Self { lo, hi }
    }

    /// A zero-length span, used during error recovery to point at "something is
    /// missing here".
    pub const fn empty(at: u32) -> Self {
        Self { lo: at, hi: at }
    }

    /// The smallest span covering both ends.
    pub fn to(self, other: Span) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    pub const fn len(self) -> u32 {
        self.hi - self.lo
    }

    pub const fn is_empty(self) -> bool {
        self.lo == self.hi
    }
}

/// A one-based line and column. Columns count characters, not UTF-8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

impl std::fmt::Display for LineCol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// One source file together with the byte offset of each line start.
#[derive(Debug, Clone)]
pub struct SourceFile {
    name: String,
    text: String,
    /// Byte offset of the start of each line. The first element is always 0.
    line_starts: Vec<u32>,
}

impl SourceFile {
    /// Takes ownership of a source text and builds the line table once.
    ///
    /// `text` must be UTF-8, which `String` already guarantees. Stripping a BOM
    /// and normalizing line endings are the caller's responsibility.
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        Self {
            name: name.into(),
            text,
            line_starts,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The text a span covers, or an empty string if the span is out of range.
    pub fn snippet(&self, span: Span) -> &str {
        let lo = span.lo as usize;
        let hi = (span.hi as usize).min(self.text.len());
        if lo > hi || !self.text.is_char_boundary(lo) || !self.text.is_char_boundary(hi) {
            return "";
        }
        &self.text[lo..hi]
    }

    /// Converts a byte offset into a line and column.
    pub fn line_col(&self, offset: u32) -> LineCol {
        let offset = offset.min(self.len());
        // line_starts is sorted; find the largest entry that is <= offset.
        let line_idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1, // line_starts[0] == 0 <= offset, so i >= 1
        };
        let line_start = self.line_starts[line_idx] as usize;
        let col = self.text[line_start..offset as usize].chars().count() as u32 + 1;
        LineCol {
            line: line_idx as u32 + 1,
            col,
        }
    }

    /// The contents of a one-based line, without its line terminator.
    pub fn line_text(&self, line: u32) -> &str {
        if line == 0 || line > self.line_count() {
            return "";
        }
        let start = self.line_starts[line as usize - 1] as usize;
        let end = self
            .line_starts
            .get(line as usize)
            .map(|&e| e as usize)
            .unwrap_or(self.text.len());
        self.text[start..end]
            .trim_end_matches('\n')
            .trim_end_matches('\r')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basics() {
        let f = SourceFile::new("t.sic", "abc\ndef\n\nghi");
        assert_eq!(f.line_col(0), LineCol { line: 1, col: 1 });
        assert_eq!(f.line_col(2), LineCol { line: 1, col: 3 });
        assert_eq!(f.line_col(3), LineCol { line: 1, col: 4 }); // the newline itself
        assert_eq!(f.line_col(4), LineCol { line: 2, col: 1 });
        assert_eq!(f.line_col(8), LineCol { line: 3, col: 1 }); // empty line
        assert_eq!(f.line_col(9), LineCol { line: 4, col: 1 });
    }

    #[test]
    fn line_col_counts_chars_not_bytes() {
        let f = SourceFile::new("t.sic", "let s = \"あい\";");
        let idx = f.text().find('い').unwrap() as u32;
        // One past `let s = "あ`, which is 10 characters.
        assert_eq!(f.line_col(idx), LineCol { line: 1, col: 11 });
    }

    #[test]
    fn line_text_strips_newline() {
        let f = SourceFile::new("t.sic", "a\r\nb\nc");
        assert_eq!(f.line_text(1), "a");
        assert_eq!(f.line_text(2), "b");
        assert_eq!(f.line_text(3), "c");
        assert_eq!(f.line_text(4), "");
    }

    #[test]
    fn span_to_merges() {
        let a = Span::new(3, 5);
        let b = Span::new(10, 12);
        assert_eq!(a.to(b), Span::new(3, 12));
        assert_eq!(b.to(a), Span::new(3, 12));
    }

    #[test]
    fn offset_past_end_is_clamped() {
        let f = SourceFile::new("t.sic", "ab");
        assert_eq!(f.line_col(99), LineCol { line: 1, col: 3 });
    }
}
