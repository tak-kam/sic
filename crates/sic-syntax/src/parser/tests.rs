use super::parse;
use crate::print::{dump, expr_str};

/// Asserts that the source parses without diagnostics and returns its dump.
fn ok(src: &str) -> String {
    let (m, diags) = parse(src);
    assert!(diags.is_empty(), "unexpected diagnostics: {:#?}", diags);
    dump(&m)
}

/// Parses a single expression by wrapping it in a function.
fn expr(src: &str) -> String {
    let wrapped = format!("fn f() {{ return {src}; }}");
    let (m, diags) = parse(&wrapped);
    assert!(diags.is_empty(), "unexpected diagnostics: {:#?}", diags);
    let crate::ast::Item::Fn(f) = &m.items[0];
    match &f.body.stmts[0] {
        crate::ast::Stmt::Return { value: Some(e), .. } => expr_str(e),
        other => panic!("not a return statement: {other:?}"),
    }
}

fn codes(src: &str) -> Vec<&'static str> {
    let (_, diags) = parse(src);
    diags.iter().filter_map(|d| d.code).collect()
}

#[test]
fn milestone_program() {
    let src = "fn main() {\n    let x = 10;\n    let y = x + 20;\n    return y;\n}\n";
    assert_eq!(
        ok(src),
        "\
(module
  (fn main
    (block
      (let x 10)
      (let y (+ x 20))
      (return y))))
"
    );
}

#[test]
fn precedence_and_associativity() {
    assert_eq!(expr("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(expr("1 * 2 + 3"), "(+ (* 1 2) 3)");
    assert_eq!(expr("1 - 2 - 3"), "(- (- 1 2) 3)");
    assert_eq!(expr("1 < 2 == true"), "(== (< 1 2) true)");
    assert_eq!(expr("a || b && c"), "(|| a (&& b c))");
    assert_eq!(expr("a && b || c"), "(|| (&& a b) c)");
    assert_eq!(expr("1 + 2 == 3 && x"), "(&& (== (+ 1 2) 3) x)");
    assert_eq!(expr("(1 + 2) * 3"), "(* (+ 1 2) 3)");
}

#[test]
fn unary_binds_tighter_than_binary() {
    assert_eq!(expr("-1 + 2"), "(+ (- 1) 2)");
    assert_eq!(expr("-x * y"), "(* (- x) y)");
    assert_eq!(expr("!a && b"), "(&& (! a) b)");
    assert_eq!(expr("- - 1"), "(- (- 1))");
}

#[test]
fn postfix_binds_tightest() {
    assert_eq!(expr("-f(1)"), "(- (call f 1))");
    assert_eq!(expr("a.b.c"), "(. (. a b) c)");
    assert_eq!(expr("f(1)(2)"), "(call (call f 1) 2)");
    assert_eq!(expr("a.b(1) + 2"), "(+ (call (. a b) 1) 2)");
    assert_eq!(expr("f()"), "(call f)");
    assert_eq!(expr("f(1, 2, 3)"), "(call f 1 2 3)");
}

#[test]
fn if_else_chain() {
    let src = "fn f(a: Int) -> Int {
        if a < 0 { return 0; } else if a == 0 { return 1; } else { return a; }
    }";
    assert_eq!(
        ok(src),
        "\
(module
  (fn f -> Int
    (params (a Int))
    (block
      (if (< a 0)
        (block
          (return 0))
        (else
          (if (== a 0)
            (block
              (return 1))
            (else
              (block
                (return a)))))))))
"
    );
}

#[test]
fn types_with_arguments() {
    let src = "fn f(x: List<Int>) -> Map<String, List<Int>> { return x; }";
    let out = ok(src);
    assert!(out.contains("(params (x List<Int>))"), "{out}");
    assert!(out.contains("-> Map<String, List<Int>>"), "{out}");
}

#[test]
fn literals() {
    assert_eq!(expr("true"), "true");
    assert_eq!(expr("null"), "null");
    assert_eq!(expr("1.5"), "1.5");
    assert_eq!(expr("2.0"), "2.0");
    assert_eq!(expr(r#""hi""#), "\"hi\"");
}

#[test]
fn let_with_type_annotation() {
    let out = ok("fn f() { let x: Int = 1; }");
    assert!(out.contains("(let x: Int 1)"), "{out}");
}

#[test]
fn return_without_value() {
    let out = ok("fn f() { return; }");
    assert!(out.contains("(return)"), "{out}");
}

// ---- errors and recovery ----

#[test]
fn missing_expression_is_reported_once() {
    assert_eq!(codes("fn f() { let y = x + ; }"), vec!["E0204"]);
}

#[test]
fn recovers_and_reports_multiple_errors() {
    // A broken statement does not stop the ones that follow from being parsed.
    let src = "fn f() {\n let a = ;\n let b = 1\n let c = 2;\n}\n";
    let (m, diags) = parse(src);
    assert!(diags.len() >= 2, "{diags:#?}");
    let crate::ast::Item::Fn(f) = &m.items[0];
    assert_eq!(f.body.stmts.len(), 3);
}

#[test]
fn unclosed_brace_does_not_hang() {
    let (_, diags) = parse("fn f() { let x = 1;");
    assert!(diags.iter().any(|d| d.code == Some("E0200")), "{diags:#?}");
}

#[test]
fn top_level_junk_recovers_to_next_fn() {
    let src = "let x = 1;\nfn f() { return 1; }\n";
    let (m, diags) = parse(src);
    assert_eq!(diags.iter().filter(|d| d.code == Some("E0202")).count(), 1);
    assert_eq!(m.items.len(), 1);
}

#[test]
fn reserved_word_as_identifier() {
    assert!(codes("fn f() { let agent = 1; }").contains(&"E0210"));
    assert!(codes("fn f() { return parallel; }").contains(&"E0210"));
}

#[test]
fn bare_block_statement_is_rejected() {
    assert!(codes("fn f() { { let x = 1; } }").contains(&"E0203"));
}

#[test]
fn empty_input_is_an_empty_module() {
    let (m, diags) = parse("");
    assert!(diags.is_empty());
    assert!(m.items.is_empty());
}
