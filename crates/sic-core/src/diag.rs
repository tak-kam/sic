//! Diagnostics and how they are rendered for a terminal.
//!
//! Every layer collects diagnostics instead of stopping at the first error, so a
//! diagnostic is plain data rather than an exceptional control flow path.

use crate::span::{SourceFile, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub const fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// A source range together with the text explaining it.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

impl Label {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// A stable diagnostic code, to be used by `sic explain <code>` later.
    pub code: Option<&'static str>,
    pub message: String,
    pub primary: Label,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>, primary: Label) -> Self {
        Self {
            severity: Severity::Error,
            code: Some(code),
            message: message.into(),
            primary,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>, primary: Label) -> Self {
        Self {
            severity: Severity::Warning,
            code: Some(code),
            ..Self::error(code, message, primary)
        }
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.secondary.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Renders a human readable form for a terminal.
    ///
    /// No colors: deciding whether the output is a TTY and adding escapes is the
    /// CLI's job. This function returns a deterministic string so it stays easy
    /// to test.
    pub fn render(&self, file: &SourceFile) -> String {
        let mut out = String::new();
        match self.code {
            Some(code) => out.push_str(&format!(
                "{}[{}]: {}\n",
                self.severity.label(),
                code,
                self.message
            )),
            None => out.push_str(&format!("{}: {}\n", self.severity.label(), self.message)),
        }

        let pos = file.line_col(self.primary.span.lo);
        // The gutter is as wide as the line number it has to hold.
        let gutter = pos.line.to_string().len();
        let pad = " ".repeat(gutter);

        out.push_str(&format!("{pad}--> {}:{}\n", file.name(), pos));
        out.push_str(&format!("{pad} |\n"));
        render_snippet(&mut out, file, &self.primary, gutter);

        for label in &self.secondary {
            let lc = file.line_col(label.span.lo);
            out.push_str(&format!("{pad} |\n"));
            if lc.line != pos.line {
                out.push_str(&format!("{pad}--> {}:{}\n", file.name(), lc));
            }
            render_snippet(&mut out, file, label, gutter);
        }

        for note in &self.notes {
            out.push_str(&format!("{pad} = note: {note}\n"));
        }
        out
    }
}

/// Writes the source line and the caret line for a single label.
fn render_snippet(out: &mut String, file: &SourceFile, label: &Label, gutter: usize) {
    let start = file.line_col(label.span.lo);
    let line = file.line_text(start.line);
    let num = format!("{:>width$}", start.line, width = gutter);
    // Tabs would misalign the carets, so collapse each to a single space.
    let display_line: String = line
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .collect();
    out.push_str(&format!("{num} | {display_line}\n"));

    let end = file.line_col(label.span.hi);
    // A span covering several lines is underlined up to the end of its first line.
    let caret_len = if end.line == start.line {
        (end.col.saturating_sub(start.col)).max(1)
    } else {
        (display_line.chars().count() as u32 + 1)
            .saturating_sub(start.col)
            .max(1)
    };
    let pad = " ".repeat(gutter);
    let indent = " ".repeat(start.col.saturating_sub(1) as usize);
    let carets = "^".repeat(caret_len as usize);
    if label.message.is_empty() {
        out.push_str(&format!("{pad} | {indent}{carets}\n"));
    } else {
        out.push_str(&format!("{pad} | {indent}{carets} {}\n", label.message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_caret_under_span() {
        let file = SourceFile::new("main.sic", "fn main() {\n    let y = x + ;\n}\n");
        let at = file.text().find(';').unwrap() as u32;
        let d = Diagnostic::error(
            "E0100",
            "expected an expression",
            Label::new(Span::new(at, at + 1), "an expression is required here"),
        )
        .with_note("the right-hand side of `+` is missing");

        let s = d.render(&file);
        let expected = "\
error[E0100]: expected an expression
 --> main.sic:2:17
  |
2 |     let y = x + ;
  |                 ^ an expression is required here
  = note: the right-hand side of `+` is missing
";
        assert_eq!(s, expected);
    }

    #[test]
    fn multi_line_span_stops_at_first_line() {
        let file = SourceFile::new("main.sic", "fn f() {\n  1\n}\n");
        let d = Diagnostic::error("E0001", "test", Label::new(Span::new(0, 12), ""));
        let s = d.render(&file);
        assert!(s.contains("1 | fn f() {\n"), "{s}");
        assert!(s.contains("^^^^^^^^"), "{s}");
    }
}
