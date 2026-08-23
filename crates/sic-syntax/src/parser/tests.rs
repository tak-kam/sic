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
    let f = only_fn(&m);
    match &f.body.stmts[0] {
        crate::ast::Stmt::Return { value: Some(e), .. } => expr_str(e),
        other => panic!("not a return statement: {other:?}"),
    }
}

/// The single function a test module is expected to hold.
fn only_fn(m: &crate::ast::Module) -> &crate::ast::FnDecl {
    match &m.items[0] {
        crate::ast::Item::Fn(f) => f,
        other => panic!("expected a function, found {other:?}"),
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

// ---- types, structs and lists ----

#[test]
fn type_declarations() {
    let out = ok("type Point {\n  x: Int,\n  y: Int,\n}\nfn main() { }");
    assert!(out.contains("(type Point (x Int) (y Int))"), "{out}");
}

#[test]
fn struct_literals() {
    let out = ok("type Point { x: Int, y: Int }\nfn main() { let p = Point { x: 1, y: 2 }; }");
    assert!(out.contains("(struct Point (x 1) (y 2))"), "{out}");
}

#[test]
fn a_struct_literal_is_not_allowed_in_an_if_condition() {
    // `if Point { .. }` would be ambiguous with the body, so the `{` starts the
    // body and the condition is just a name.
    let (m, diags) = parse("type Point { x: Int }\nfn f() { if Point { x: 1 } }");
    assert!(!diags.is_empty(), "{m:?}");

    // Parentheses make it unambiguous again.
    ok("type Point { ok: Bool }\nfn f() { if (Point { ok: true }).ok { return; } }");
}

#[test]
fn list_literals_and_indexing() {
    assert_eq!(expr("[1, 2, 3]"), "(list 1 2 3)");
    assert_eq!(expr("[]"), "(list)");
    assert_eq!(expr("xs[0]"), "(index xs 0)");
    // Indexing binds as tightly as a call.
    assert_eq!(expr("xs[0] + 1"), "(+ (index xs 0) 1)");
    assert_eq!(expr("f(xs)[1]"), "(index (call f xs) 1)");
    assert_eq!(expr("a.b[2]"), "(index (. a b) 2)");
}

#[test]
fn a_struct_literal_is_legal_inside_brackets_and_parentheses() {
    // Inside a delimiter there is no ambiguity to avoid.
    ok("type P { x: Int }\nfn f(p: P) { }\nfn main() { f(P { x: 1 }); }");
    ok("type P { x: Int }\nfn main() { let xs = [P { x: 1 }]; }");
}

// ---- tasks and policies ----

#[test]
fn spawn_and_await() {
    assert_eq!(expr("await t"), "(await t)");
    let out = ok("fn f() -> Int { return 1; }\nfn main() { let t = spawn f(); return await t; }");
    assert!(out.contains("(let t (spawn f))"), "{out}");
    assert!(out.contains("(return (await t))"), "{out}");
}

#[test]
fn spawn_takes_arguments_like_a_call() {
    let out = ok("fn f(a: Int) -> Int { return a; }\nfn main() { let t = spawn f(1, 2); }");
    assert!(out.contains("(spawn f 1 2)"), "{out}");
}

#[test]
fn await_binds_like_a_prefix_operator() {
    // Tighter than any binary operator, looser than a call.
    assert_eq!(expr("await a + await b"), "(+ (await a) (await b))");
    assert_eq!(expr("await f(1)"), "(await (call f 1))");
}

#[test]
fn a_policy_follows_a_call() {
    assert_eq!(expr("f(1) retry 3"), "((call f 1) retry 3)");
    assert_eq!(expr("f() timeout 500"), "((call f) timeout 500)");
    // Either order, and both.
    assert_eq!(
        expr("f() retry 2 timeout 5"),
        "((call f) retry 2 timeout 5)"
    );
    assert_eq!(
        expr("f() timeout 5 retry 2"),
        "((call f) retry 2 timeout 5)"
    );
}

#[test]
fn a_policy_does_not_disturb_the_surrounding_expression() {
    assert_eq!(expr("f() retry 2 + 1"), "(+ ((call f) retry 2) 1)");
    assert_eq!(expr("g(f() retry 2)"), "(call g ((call f) retry 2))");
}

#[test]
fn a_policy_is_rejected_when_it_is_malformed() {
    assert!(codes("fn main() { let x = f() retry; }").contains(&"E0207"));
    assert!(codes("fn main() { let x = f() retry 0; }").contains(&"E0207"));
    assert!(codes("fn main() { let x = f() retry 1 retry 2; }").contains(&"E0206"));
}

#[test]
fn spawn_needs_a_call() {
    assert!(codes("fn main() { let t = spawn 1; }").contains(&"E0201"));
    assert!(codes("fn main() { let t = spawn f; }").contains(&"E0205"));
}

// ---- capability grants ----

#[test]
fn allow_block() {
    let src = "allow {\n    fs.read \"./input.txt\";\n    process.exec \"/usr/bin/true\";\n}\nfn main() { }\n";
    assert_eq!(
        ok(src),
        "\
(module
  (allow
    (fs.read \"./input.txt\")
    (process.exec \"/usr/bin/true\"))
  (fn main
    (block)))
"
    );
}

#[test]
fn a_grant_may_omit_its_constraint() {
    let out = ok("allow { fs.read; }\nfn main() { }");
    assert!(out.contains("(fs.read)"), "{out}");
}

#[test]
fn an_empty_allow_block_is_legal() {
    let out = ok("allow { }\nfn main() { }");
    assert!(out.contains("(allow)"), "{out}");
}

#[test]
fn a_broken_grant_does_not_swallow_the_next_one() {
    let (m, diags) = parse("allow {\n  fs.read \"a\"\n  fs.write \"b\";\n}\n");
    assert!(!diags.is_empty());
    let crate::ast::Item::Allow(a) = &m.items[0] else {
        panic!("expected an allow block");
    };
    assert_eq!(a.grants.len(), 2);
}

#[test]
fn process_is_an_identifier_not_a_reserved_word() {
    // `process` is the namespace of `process.exec`, so it has to lex as a name.
    ok("fn main() { let process = 1; return process; }");
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
    assert_eq!(only_fn(&m).body.stmts.len(), 3);
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
    assert!(codes("fn f() { let import = 1; }").contains(&"E0210"));
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
