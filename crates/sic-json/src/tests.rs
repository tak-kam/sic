use super::*;

fn ok(text: &str) -> Json {
    parse(text).unwrap_or_else(|e| panic!("{text} should parse: {e}"))
}

fn err(text: &str) -> String {
    parse(text)
        .map(|v| panic!("{text} should not parse, got {v:?}"))
        .unwrap_err()
        .message
}

#[test]
fn scalars() {
    assert_eq!(ok("null"), Json::Null);
    assert_eq!(ok("true"), Json::Bool(true));
    assert_eq!(ok("false"), Json::Bool(false));
    assert_eq!(ok("\"hi\""), Json::Str("hi".into()));
}

#[test]
fn integers_and_floats_are_distinguished() {
    // sic has two number types, so which one a value is has to be decided here
    // rather than guessed later.
    assert_eq!(ok("0"), Json::Int(0));
    assert_eq!(ok("-42"), Json::Int(-42));
    assert_eq!(ok("1.5"), Json::Float(1.5));
    assert_eq!(ok("1.0"), Json::Float(1.0));
    assert_eq!(ok("1e3"), Json::Float(1000.0));
    assert_eq!(ok("-1.5e-3"), Json::Float(-0.0015));
}

#[test]
fn numbers_follow_the_grammar() {
    assert!(err("01").contains("trailing"));
    assert!(err("1.").contains("digit after"));
    assert!(err(".5").contains("unexpected"));
    assert!(err("1e").contains("exponent"));
    assert!(err("+1").contains("unexpected"));
    assert!(err("NaN").contains("unexpected"));
    assert!(err("99999999999999999999").contains("outside the range"));
}

#[test]
fn arrays_and_objects() {
    assert_eq!(ok("[]"), Json::Array(Vec::new()));
    assert_eq!(ok("[1, 2]"), Json::Array(vec![Json::Int(1), Json::Int(2)]));
    assert_eq!(ok("{}"), Json::Object(Vec::new()));
    assert_eq!(
        ok("{\"a\": 1}"),
        Json::Object(vec![("a".into(), Json::Int(1))])
    );
    assert_eq!(ok(" \t\r\n [ 1 ] ").kind(), "an array");
}

#[test]
fn what_the_grammar_does_not_allow() {
    // A model that produces these has produced invalid JSON, and saying so is
    // more useful than guessing what was meant.
    assert!(err("[1, 2,]").contains("unexpected"));
    assert!(err("{\"a\": 1,}").contains("expected `\""));
    assert!(err("{'a': 1}").contains("expected `\""));
    assert!(err("// a comment\n1").contains("unexpected"));
    assert!(err("{\"a\": 1} {\"b\": 2}").contains("trailing"));
    assert!(err("").contains("empty"));
}

#[test]
fn duplicate_keys_are_an_error() {
    // Last-wins would make the meaning depend on the order.
    assert!(err("{\"a\": 1, \"a\": 2}").contains("duplicate key `a`"));
}

#[test]
fn strings_and_escapes() {
    assert_eq!(ok(r#""a\nb""#), Json::Str("a\nb".into()));
    assert_eq!(ok(r#""あ""#), Json::Str("あ".into()));
    assert_eq!(
        ok(r#""\/\\\"\b\f\r\t""#),
        Json::Str("/\\\"\u{8}\u{c}\r\t".into())
    );
    // Surrogate pairs make an astral character.
    assert_eq!(ok(r#""😀""#), Json::Str("😀".into()));
    assert!(err(r#""\uD83D""#).contains("low surrogate"));
    assert!(err(r#""\uDE00""#).contains("without a high one"));
    assert!(err(r#""\q""#).contains("unknown escape"));
    assert!(err("\"a\nb\"").contains("control character"));
    assert!(err(r#""unclosed"#).contains("not closed"));
    // Non-ASCII text needs no escaping.
    assert_eq!(ok("\"日本語\""), Json::Str("日本語".into()));
}

#[test]
fn nesting_is_bounded() {
    // The input is untrusted, so a deep document must not exhaust the stack.
    let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
    assert!(err(&deep).contains("nested deeper"));

    let fine = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
    assert!(parse(&fine).is_ok());
}

#[test]
fn the_document_is_bounded() {
    let huge = format!("\"{}\"", "x".repeat(MAX_LEN));
    assert!(err(&huge).contains("over the"));
}

#[test]
fn a_nested_document_round_trips_its_shape() {
    let value = ok(r#"{"cause": "disk full", "evidence": [{"source": "syslog", "weight": 3}]}"#);
    assert_eq!(value.member("cause"), Some(&Json::Str("disk full".into())));
    let Some(Json::Array(evidence)) = value.member("evidence") else {
        panic!("expected an array");
    };
    assert_eq!(evidence[0].member("weight"), Some(&Json::Int(3)));
}

#[test]
fn a_message_names_the_character_that_is_there() {
    // What a model answers with is untrusted text in whatever language it
    // chose, so this is the one input where non-ASCII is expected. Casting the
    // byte would report `ã`, the Latin-1 reading of the first byte of `そ`,
    // which is a character the document does not contain.
    assert!(err("そうですね").contains("unexpected `そ`"));
    assert!(err(r#""\そ""#).contains("unknown escape `\\そ`"));
    let e = parse(r#""\そ""#).unwrap_err();
    assert_eq!(&r#""\そ""#[e.offset..e.offset + 3], "そ");
}

#[test]
fn bytes_that_are_not_utf8_are_reported_as_that() {
    // A JSON document has to be UTF-8 (RFC 8259 §8.1). `parse` takes a `&str`,
    // so the only place bytes that are not can turn up is this decode.
    assert_eq!(char_at("そ".as_bytes(), 0), Some('そ'));
    assert_eq!(char_at("aそ".as_bytes(), 1), Some('そ'));
    // A character cut short, and a byte that is valid UTF-8 nowhere.
    assert_eq!(char_at(b"\xE3\x81", 0), None);
    assert_eq!(char_at(b"\xFF", 0), None);
    assert_eq!(char_at(b"", 0), None);
    assert!(not_utf8(0xE3).contains("not UTF-8"));
    assert!(not_utf8(0xE3).contains("0xE3"));
}

#[test]
fn an_escape_cut_by_a_character_is_refused_not_a_panic() {
    // The four hex digits are read as bytes, because slicing the text four
    // bytes on can land inside `そ`.
    assert!(err(r#""\u00そ""#).contains("four hex digits"));
    assert!(err(r#""\uそ""#).contains("four hex digits"));
    // `from_str_radix` would have taken the sign; the grammar has no such digit.
    assert!(err(r#""\u+041""#).contains("four hex digits"));
    assert_eq!(ok(r#""\u0041""#), Json::Str("A".into()));
}

#[test]
fn errors_say_where() {
    let e = parse("{\"a\": }").unwrap_err();
    assert_eq!(&"{\"a\": }"[e.offset..e.offset + 1], "}");
}
