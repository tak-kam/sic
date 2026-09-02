use super::{MAX_DEPTH, parse};
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
fn a_type_may_describe_part_of_a_document() {
    let out = ok("type Line {\n  reason: String,\n  ..\n}\nfn main() { }");
    assert!(out.contains("(type Line (reason String) ..)"), "{out}");
    // It sits where a field would, so it is separated like one: the comma
    // after `reason` is the same comma two fields would need.
    let out = ok("type Line { reason: String, .. }\nfn main() { }");
    assert!(out.contains("(type Line (reason String) ..)"), "{out}");
    // A type may be nothing but the marker, and it then describes any object.
    let out = ok("type Anything { .. }\nfn main() { }");
    assert!(out.contains("(type Anything ..)"), "{out}");
}

#[test]
fn the_marker_has_to_be_the_last_thing_in_the_body() {
    // It reads as "and the rest", and there is nowhere else for the rest to be.
    assert_eq!(
        codes("type Line { .., reason: String }\nfn main() { }"),
        vec!["E0219"]
    );
}

#[test]
fn a_field_may_say_it_is_sometimes_not_there() {
    let out = ok("type Artifact {\n  reason: String,\n  executable: String?,\n}\nfn main() { }");
    assert!(
        out.contains("(type Artifact (reason String) (executable String?))"),
        "{out}"
    );
    // And it composes with `..`, which is the shape a protocol reader has:
    // some fields ignored, one sometimes missing.
    let out = ok("type Artifact { reason: String, executable: String?, .. }\nfn main() { }");
    assert!(
        out.contains("(type Artifact (reason String) (executable String?) ..)"),
        "{out}"
    );
}

#[test]
fn a_question_mark_belongs_to_a_field_and_nowhere_else() {
    // A field may be optional; a value is never, so there is nowhere else in
    // the grammar for the marker to go - not a `let`, not a parameter, not a
    // return type, and not the element type of a list.
    assert_eq!(
        codes("fn main() { let x: String? = \"a\"; }"),
        vec!["E0221"]
    );
    assert_eq!(codes("fn f(x: Int?) { }\nfn main() { }"), vec!["E0221"]);
    assert_eq!(codes("fn f() -> Int? { }\nfn main() { }"), vec!["E0221"]);
    assert_eq!(
        codes("type T { xs: List<String?> }\nfn main() { }"),
        vec!["E0221"]
    );
}

#[test]
fn asking_whether_a_field_is_there_is_postfix() {
    // It binds like `.` and `[i]`, so `a.b?` is a question about `a.b` rather
    // than about `a`.
    assert_eq!(expr("a.b?"), "(has (. a b))");
    assert_eq!(expr("!a.b?"), "(! (has (. a b)))");
}

#[test]
fn a_field_access_is_still_two_tokens() {
    // `..` is one token now, and `a.b` had better not have become one.
    assert_eq!(expr("p.x"), "(. p x)");
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
fn a_grant_may_pin_what_runs() {
    let out = ok("allow { process.exec \"/usr/bin/true\" sha256 \"abc\"; }\nfn main() { }");
    assert!(out.contains("sha256 \"abc\""), "{out}");
    assert!(codes("allow { process.exec \"/x\" sha256; }\nfn main() { }").contains(&"E0211"));
}

/// `answers` is a fifth arm of the order-free clause loop, so it composes with
/// the other four in any order - and the format is a bare identifier, so a word
/// that is not one of the two is a diagnostic here rather than a string nothing
/// refuses until a broker does.
#[test]
fn a_grant_may_say_what_shape_its_program_answers_in() {
    let out = ok("allow { process.run \"/usr/bin/cargo\" answers jsonl; }\nfn main() { }");
    assert!(out.contains("answers jsonl"), "{out}");
    let out = ok(
        "allow { process.run \"/usr/bin/cargo\" answers json in \"/srv\" repeatable; }\n\
         fn main() { }",
    );
    assert!(out.contains("answers json"), "{out}");
    assert!(out.contains("in \"/srv\""), "{out}");
    assert!(out.contains("repeatable"), "{out}");
    // The same clauses the other way round, because order-free means it.
    let out = ok(
        "allow { process.run \"/usr/bin/cargo\" repeatable in \"/srv\" answers json; }\n\
         fn main() { }",
    );
    assert!(out.contains("answers json"), "{out}");

    for bad in [
        "allow { process.run \"/x\" answers jsonl1; }\nfn main() { }",
        "allow { process.run \"/x\" answers \"json\"; }\nfn main() { }",
        "allow { process.run \"/x\" answers; }\nfn main() { }",
    ] {
        assert!(codes(bad).contains(&"E0220"), "{bad}");
    }
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
    assert!(codes("fn f() { let capability = 1; }").contains(&"E0210"));
    assert!(codes("fn f() { return parallel; }").contains(&"E0210"));
}

#[test]
fn for_over_a_list() {
    let src = "fn f() {\n    for x in xs {\n        log info x;\n    }\n}\n";
    assert_eq!(
        ok(src),
        "\
(module
  (fn f
    (block
      (for x xs
        (block
          (log info x))))))
"
    );
}

/// `for x in Point { .. }` would be the body and a struct literal reading the
/// same `{`, which is the ambiguity an `if` condition already has. The header
/// is read the same way, so parentheses are what makes a literal legal again.
#[test]
fn a_struct_literal_in_a_for_header_needs_parentheses() {
    ok("fn f() { for x in (Point { a: 1 }).xs { log info x; } }");
    let src = "fn f() { for x in Point { a: 1 } { log info x; } }";
    assert!(!codes(src).is_empty(), "the bare literal must not parse");
}

/// `for` and `in` mean something now. The other three still do not.
#[test]
fn while_loop_and_mut_are_still_reserved() {
    assert!(codes("fn f() { let while = 1; }").contains(&"E0210"));
    assert!(codes("fn f() { let loop = 1; }").contains(&"E0210"));
    assert!(codes("fn f() { let mut = 1; }").contains(&"E0210"));
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

#[test]
fn imports_and_requirements() {
    let src = "\
import \"./lib/deploy.sic\";

requires {
    process.exec;
    fs.read;
}

fn main() -> Int {
    return 0;
}
";
    assert_eq!(
        ok(src),
        "\
(module
  (import \"./lib/deploy.sic\")
  (requires process.exec fs.read)
  (fn main -> Int
    (block
      (return 0))))
"
    );
}

#[test]
fn an_import_needs_a_path() {
    assert!(codes("import lib;").contains(&"E0212"));
}

#[test]
fn a_requirement_is_a_capability_name() {
    assert!(codes("requires { process; }").contains(&"E0200"));
}

/// `memory: task` is how an agent says it keeps a conversation. `task` is the
/// only scope: one lasting a whole run is what a program that never spawns
/// already gets, and one lasting a call is what not writing this means.
#[test]
fn an_agent_may_keep_a_conversation() {
    let dump = ok("agent r { input: String, output: P, budget: 2, memory: task }\nfn main() { }");
    assert!(dump.contains("memory"), "{dump}");

    assert!(codes("agent r { memory: run }\nfn main() { }").contains(&"E0215"));
    assert!(codes("agent r { memory: 3 }\nfn main() { }").contains(&"E0215"));
    // An unknown setting still says what the settings are.
    assert!(codes("agent r { recall: task }\nfn main() { }").contains(&"E0209"));
}

/// One source file per shape the parser recurses on, each nested `n` deep.
///
/// The deep input is built here rather than kept as a fixture: a file of four
/// thousand parentheses is not something a reader can check, and the number
/// that matters is `MAX_DEPTH`, which a fixture would go stale against.
fn nested(n: usize) -> Vec<(&'static str, String)> {
    vec![
        (
            "parentheses",
            format!("fn f() {{ return {}1{}; }}", "(".repeat(n), ")".repeat(n)),
        ),
        (
            "unary `!`",
            format!("fn f() {{ return {}true; }}", "!".repeat(n)),
        ),
        (
            "unary `-`",
            format!("fn f() {{ return {}1; }}", "-".repeat(n)),
        ),
        (
            "`await`",
            format!("fn f() {{ let x = {}y; }}", "await ".repeat(n)),
        ),
        (
            "list literals",
            format!("fn f() {{ let x = {}{}; }}", "[".repeat(n), "]".repeat(n)),
        ),
        (
            "call arguments",
            format!("fn f() {{ let x = {}{}; }}", "g(".repeat(n), ")".repeat(n)),
        ),
        (
            "index expressions",
            format!("fn f() {{ let x = a{}{}; }}", "[a".repeat(n), "]".repeat(n)),
        ),
        (
            "struct literals",
            format!(
                "fn f() {{ let x = {}1{}; }}",
                "P { v: ".repeat(n),
                " }".repeat(n)
            ),
        ),
        (
            "type arguments",
            format!(
                "fn f() -> {}Int{} {{ return x; }}",
                "List<".repeat(n),
                ">".repeat(n)
            ),
        ),
        (
            "`if` blocks",
            format!("fn f() {{ {}{} }}", "if true { ".repeat(n), "}".repeat(n)),
        ),
        (
            "`else if` chains",
            format!("fn f() {{ if true {{}} {}}}", "else if true {} ".repeat(n)),
        ),
    ]
}

/// Source is untrusted in the same way a model's answer is, so a file that
/// nests too deeply has to be a diagnostic rather than a stack overflow. Every
/// shape is checked, because a limit that counted only parentheses would leave
/// the same hole open under a different shape of input.
#[test]
fn nesting_deeper_than_the_limit_is_reported_and_not_fatal() {
    for (shape, src) in nested(MAX_DEPTH as usize + 1) {
        // Exactly one code: the levels on the way out each have an unclosed
        // delimiter to complain about, and a hundred of those would bury the
        // line that says what is actually wrong.
        assert_eq!(codes(&src), vec!["E0214"], "{shape}");
    }
}

/// The other side of the boundary. The limit counts blocks, expressions and
/// types against one budget, so a nested `if` spends two levels and a
/// parenthesis one; what is promised is that nesting well inside the limit is
/// accepted, not the exact level each shape turns over at.
#[test]
fn nesting_within_the_limit_still_parses() {
    for (shape, src) in nested(MAX_DEPTH as usize / 4) {
        let (_, diags) = parse(&src);
        assert!(diags.is_empty(), "{shape}: {diags:#?}");
    }
}

/// Hitting the limit ends the parse rather than derailing it: the parser stops
/// where it is instead of looping on input it cannot make progress through.
#[test]
fn a_file_that_nests_too_deeply_does_not_hang() {
    let deep = "(".repeat(MAX_DEPTH as usize * 4);
    let src = format!("fn f() {{ return {deep}1; }}\nfn g() {{ return 2; }}");
    assert_eq!(codes(&src), vec!["E0214"]);
}

/// Node ids have to be unique across a whole program, not within one file.
///
/// The checker keys its tables by `NodeId` and a program is one module merged
/// from all its files, so two files that each numbered from zero put two
/// entries under one key and the second won. `Parser::id` is the only place an
/// id is minted and it only ever counts up, so a supply carried from file to
/// file is enough: what has to be true is that the second file starts where the
/// first stopped.
#[test]
fn two_files_do_not_share_node_ids() {
    use super::{NodeIds, parse_at};

    let src = "fn f(x: Int) -> Int { return x + 1; }";
    let mut ids = NodeIds::new();

    let (first, diags) = parse_at(src, 0, &mut ids);
    assert!(diags.is_empty(), "{diags:#?}");
    let boundary = ids.peek();
    assert!(boundary > 0, "the first file used some ids");

    let (second, diags) = parse_at(src, 0, &mut ids);
    assert!(diags.is_empty(), "{diags:#?}");
    assert!(ids.peek() > boundary, "the second file used some too");

    // Same source, so anything that matches between them would have collided.
    assert_ne!(fn_of(&first).id, fn_of(&second).id);
    assert!(fn_of(&second).id.0 >= boundary);
    assert!(return_id(fn_of(&second)).0 >= boundary);

    // And a parse on its own still starts from zero, because a single file is
    // the whole program.
    let (alone, _) = super::parse(src);
    assert_eq!(fn_of(&alone).id, fn_of(&first).id);
}

fn fn_of(m: &crate::ast::Module) -> &crate::ast::FnDecl {
    match &m.items[0] {
        crate::ast::Item::Fn(f) => f,
        other => panic!("not a function: {other:?}"),
    }
}

/// The id of the expression a function returns - one the checker keys `res_of`
/// and `type_of` by, which is where a collision did its damage.
fn return_id(f: &crate::ast::FnDecl) -> sic_core::NodeId {
    match &f.body.stmts[0] {
        crate::ast::Stmt::Return { value: Some(e), .. } => e.id,
        other => panic!("not a return: {other:?}"),
    }
}

/// The three bounds an agent with tools needs, and each is a number: model
/// calls, tool uses, and milliseconds. `deadline` shares its unit with
/// `timeout`, because two units for duration in one language is a bug nobody
/// sees.
#[test]
fn an_agent_may_bound_its_tools_and_its_time() {
    let dump = ok(concat!(
        "agent r { input: String, output: P, budget: 2, tools: 30, deadline: 600000 }\n",
        "fn main() { }",
    ));
    assert!(dump.contains("(budget 2)"), "{dump}");
    assert!(dump.contains("(tools 30)"), "{dump}");
    assert!(dump.contains("(deadline 600000)"), "{dump}");

    for bad in [
        "agent r { tools: -1 }\nfn main() { }",
        "agent r { budget: 0 }\nfn main() { }",
        "agent r { deadline: 0 }\nfn main() { }",
        "agent r { deadline: task }\nfn main() { }",
    ] {
        assert!(codes(bad).contains(&"E0208"), "{bad}");
    }
}

/// `tools: 0` is the one count that may be zero, because zero is not a
/// degenerate allowance here - it is the strongest claim an agent declaration
/// can make: this one answers a question and does not act.
///
/// An agent that may make no model calls is not an agent, and an answer that
/// must arrive in no milliseconds cannot arrive, so `budget` and `deadline`
/// keep their floor. That asymmetry is the whole of #86.
#[test]
fn an_agent_may_declare_that_it_uses_no_tools() {
    let dump = ok(concat!(
        "agent r { input: String, output: P, tools: 0 }\n",
        "fn main() { }",
    ));
    assert!(dump.contains("(tools 0)"), "{dump}");
}

/// `retry` reads the same in an agent body as it does after a call.
///
/// It is a keyword in both places rather than a keyword in one and a field name
/// in the other, so the parser reads the token where it stands. A `retry` that
/// had to be spelled differently inside a declaration would be a second word
/// for one idea.
#[test]
fn an_agent_may_declare_a_retry() {
    let dump = ok(concat!(
        "agent r { input: String, output: P, retry: 3 }\n",
        "fn main() { }",
    ));
    assert!(dump.contains("(retry 3)"), "{dump}");

    for bad in [
        "agent r { retry: 0 }\nfn main() { }",
        "agent r { retry: -1 }\nfn main() { }",
        "agent r { retry: task }\nfn main() { }",
    ] {
        assert!(codes(bad).contains(&"E0208"), "{bad}");
    }
}

/// A chain is flat to read and deep as a tree.
///
/// `1 + 1 + 1 + ...` and `a.f.f.f...` are read by a loop, so the parser never
/// goes deeper - but each operator wraps what came before, so the tree gets one
/// level per term. Every pass that walks the AST afterwards recurses on that,
/// and three thousand terms in a seven-kilobyte file used to take the process
/// down in `print::dump`, in name resolution and in type checking.
#[test]
fn a_flat_chain_is_still_a_deep_tree() {
    let long = MAX_DEPTH as usize + 1;
    let chains = [
        (
            "a sum",
            format!("fn f() {{ return {}; }}", vec!["1"; long].join(" + ")),
        ),
        (
            "field access",
            format!("fn f() {{ let a = 1; return a{}; }}", ".f".repeat(long)),
        ),
        (
            "indexing",
            format!("fn f() {{ let a = 1; return a{}; }}", "[0]".repeat(long)),
        ),
        (
            "calls",
            format!("fn f() {{ let a = 1; return a{}; }}", "()".repeat(long)),
        ),
    ];
    for (what, src) in chains {
        assert_eq!(codes(&src), vec!["E0214"], "{what}");
    }
}

/// Nesting and chaining are additive, because they are the same path.
///
/// A hundred nested parentheses each holding a hundred-term sum is one path two
/// hundred nodes long, which is why there is one budget and not two: bounding
/// them separately would bound their product, and a stack does not care about
/// products.
#[test]
fn nesting_and_chaining_share_one_budget() {
    // A little under half of each, since the block and the `return` are on the
    // same path and take a level too.
    let half = MAX_DEPTH as usize / 2 - 4;
    let together = format!(
        "fn f() {{ return {}{}{}; }}",
        "(".repeat(half),
        vec!["1"; half].join(" + "),
        ")".repeat(half)
    );
    assert!(parse(&together).1.is_empty(), "{together}");

    // Neither half is over the budget on its own; together they are.
    let over = format!(
        "fn f() {{ return {}{}{}; }}",
        "(".repeat(half),
        vec!["1"; half + 16].join(" + "),
        ")".repeat(half)
    );
    assert_eq!(codes(&over), vec!["E0214"]);
}
