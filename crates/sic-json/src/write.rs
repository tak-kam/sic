//! Writing JSON.
//!
//! Only the leaf: a string, escaped and quoted. The documents this workspace
//! writes - a journal line, an OTLP payload, a recorded answer, a JSON-RPC
//! reply - are each a fixed shape that its own writer builds, and building them
//! is not the part that was worth sharing. Escaping is: it is the one place
//! where getting it wrong produces a document that parses into something other
//! than what was meant, and it belongs to the format rather than to any of the
//! four writers that used to carry a copy of it.

/// Appends `value` to `out` as a JSON string, quotes included.
///
/// What is escaped is what RFC 8259 §7 requires and nothing else: the quote,
/// the backslash, and every code point below U+0020. Text above it is written
/// as it stands, because a JSON document is UTF-8 (§8.1) and `\u` escaping
/// something already representable would only make it longer.
///
/// The three C0 characters with a short form here - `\n`, `\r`, `\t` - are the
/// ones that appear in the text this workspace writes. `\b` and `\f` have short
/// forms too and get the numeric escape instead; both spellings are valid, and
/// a reader who meets `\u0008` in a journal line is not misled by it.
///
/// The input is a `&str`, so it is already valid UTF-8 and cannot hold a lone
/// surrogate. That is why nothing here has to reject anything.
pub fn write_quoted(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `value` as a JSON string, quotes included.
pub fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    write_quoted(&mut out, value);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Json, parse};

    #[test]
    fn quotes_and_escapes_what_json_requires() {
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("a \"quote\""), "\"a \\\"quote\\\"\"");
        assert_eq!(quoted("a \\ backslash"), "\"a \\\\ backslash\"");
        assert_eq!(quoted("a\nb\rc\td"), "\"a\\nb\\rc\\td\"");
    }

    /// Every C0 control character comes out escaped, and the two with a short
    /// form this writer does not use come out numeric rather than raw.
    #[test]
    fn every_control_character_is_escaped() {
        for raw in 0u32..0x20 {
            let c = char::from_u32(raw).unwrap();
            let written = quoted(&c.to_string());
            assert!(
                !written.chars().any(|w| (w as u32) < 0x20),
                "U+{raw:04X} was written raw: {written:?}"
            );
        }
        assert_eq!(quoted("\u{8}"), "\"\\u0008\"");
        assert_eq!(quoted("\u{c}"), "\"\\u000c\"");
        assert_eq!(quoted("\u{1}"), "\"\\u0001\"");
    }

    /// U+007F is a control character to a terminal and an ordinary one to JSON.
    #[test]
    fn text_above_the_control_range_is_written_as_it_stands() {
        assert_eq!(quoted("\u{7f}"), "\"\u{7f}\"");
        assert_eq!(quoted("日本語"), "\"日本語\"");
        assert_eq!(quoted("\u{1f600}"), "\"\u{1f600}\"");
    }

    /// The property the four copies of this existed to have, checked here once:
    /// what comes out parses, and parses back to what went in.
    #[test]
    fn round_trips_through_the_parser() {
        let awkward = [
            "",
            "plain",
            "a \"quote\", a \\, a \n and a \u{1}",
            "\u{0}\u{1f}\u{7f}",
            "日本語\u{1f600}",
            "\\\\\"\"",
        ];
        for text in awkward {
            let json = quoted(text);
            match parse(&json) {
                Ok(Json::Str(back)) => assert_eq!(back, text, "{json:?}"),
                other => panic!("{json:?} parsed as {other:?}"),
            }
        }
    }

    #[test]
    fn writing_appends_rather_than_replaces() {
        let mut out = String::from("{\"k\":");
        write_quoted(&mut out, "v");
        out.push('}');
        assert_eq!(out, "{\"k\":\"v\"}");
    }
}
