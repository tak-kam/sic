use super::*;
use crate::ty::Types;

fn check_src(src: &str) -> (Typed, Vec<Diagnostic>) {
    let (module, parse_diags) = sic_syntax::parse(src);
    assert!(parse_diags.is_empty(), "parse errors: {parse_diags:#?}");
    check(&module)
}

fn codes(src: &str) -> Vec<&'static str> {
    let (_, diags) = check_src(src);
    diags.iter().filter_map(|d| d.code).collect()
}

fn ok(src: &str) -> Typed {
    let (typed, diags) = check_src(src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    typed
}

#[test]
fn milestone_infers_int() {
    let typed = ok("fn main() {\n let x = 10;\n let y = x + 20;\n return y;\n}\n");
    let main = &typed.fns[0];
    assert_eq!(main.ret, Types::INT);
    assert_eq!(main.local_types, vec![Types::INT, Types::INT]);
    assert_eq!(typed.entry, Some(FuncId(0)));
}

#[test]
fn annotated_signature() {
    let typed = ok("fn add(a: Int, b: Int) -> Int { return a + b; }");
    assert_eq!(typed.fns[0].params, vec![Types::INT, Types::INT]);
    assert_eq!(typed.fns[0].ret, Types::INT);
}

#[test]
fn calls_are_checked() {
    ok("fn add(a: Int, b: Int) -> Int { return a + b; }\nfn main() { return add(1, 2); }");
    assert!(
        codes("fn f(a: Int) -> Int { return a; }\nfn main() { return f(true); }")
            .contains(&"E0301")
    );
    assert!(
        codes("fn f(a: Int) -> Int { return a; }\nfn main() { return f(1, 2); }")
            .contains(&"E0302")
    );
    assert!(codes("fn main() { return nope(1); }").contains(&"E0300"));
    assert!(codes("fn main() { let x = 1; return x(1); }").contains(&"E0305"));
}

#[test]
fn a_function_checked_earlier_can_be_called_without_annotation() {
    ok("fn helper() { return 1; }\nfn main() { return helper(); }");
}

#[test]
fn forward_reference_needs_an_annotation() {
    assert!(codes("fn main() { return helper(); }\nfn helper() { return 1; }").contains(&"E0306"));
    // Recursion is a forward reference to the function being checked.
    assert!(codes("fn f(n: Int) { return f(n); }").contains(&"E0306"));
    // With an annotation it is fine.
    ok("fn f(n: Int) -> Int { if n == 0 { return 0; } return f(n - 1); }");
}

#[test]
fn operators_accept_only_what_the_vm_can_execute() {
    assert!(codes("fn main() { return 1 + true; }").contains(&"E0303"));
    assert!(codes("fn main() { return 1.5 + 2.5; }").contains(&"E0303"));
    // `+` is the one operator with two instructions behind it, and `String` is
    // the only type other than `Int` it takes.
    ok("fn main() -> String { return \"a\" + \"b\"; }");
    assert!(codes("fn main() { return \"a\" - \"b\"; }").contains(&"E0303"));
    assert!(codes("fn main() { return \"a\" + 1; }").contains(&"E0303"));
    assert!(codes("fn main() { return true < false; }").contains(&"E0303"));
    assert!(codes("fn main() { return -true; }").contains(&"E0303"));
    assert!(codes("fn main() { return !1; }").contains(&"E0303"));
    ok("fn main() { return true == false; }");
    ok("fn main() { return 1 < 2 && 3 >= 4; }");
}

#[test]
fn a_float_and_a_string_are_values_in_their_own_right() {
    let typed = ok("fn main() -> Float { let x = 1.5; return x; }");
    assert_eq!(typed.fns[0].ret, Types::FLOAT);
    let typed = ok("fn main() -> String { let s = \"hi\"; return s; }");
    assert_eq!(typed.fns[0].ret, Types::STR);
}

/// Ordering is the whole of what a `Float` does, and the two things it does
/// not do are refused for different reasons: arithmetic is deferred, equality
/// is declined. `docs/design/v0.1.md` §4 argues both.
#[test]
fn a_float_orders_and_does_nothing_else() {
    ok("fn main() -> Bool { return 0.9 > 0.7; }");
    ok("fn main() -> Bool { let c = 0.9; return c >= 0.7 && c <= 1.0; }");
    assert!(codes("fn main() { return 0.7 == 0.7; }").contains(&"E0303"));
    assert!(codes("fn main() { return 0.7 != 0.7; }").contains(&"E0303"));
    assert!(codes("fn main() { return 1.5 * 2.0; }").contains(&"E0303"));
    // No implicit conversion arrived with the operators.
    assert!(codes("fn main() { return 0.5 < 1; }").contains(&"E0303"));
}

#[test]
fn if_condition_must_be_bool() {
    assert!(codes("fn main() { if 1 { return 1; } return 0; }").contains(&"E0301"));
    ok("fn main() { if true { return 1; } return 0; }");
}

#[test]
fn return_type_must_match_annotation() {
    assert!(codes("fn f() -> Int { return true; }").contains(&"E0301"));
    // Without an annotation the first return fixes the type.
    assert!(codes("fn f() { if true { return 1; } return true; }").contains(&"E0301"));
}

#[test]
fn missing_return_is_reported() {
    assert!(codes("fn f() -> Int { let x = 1; }").contains(&"E0307"));
    // Returning on both branches of an if/else counts.
    ok("fn f(a: Int) -> Int { if a > 0 { return 1; } else { return 2; } }");
    // Only one branch is not enough.
    assert!(codes("fn f(a: Int) -> Int { if a > 0 { return 1; } }").contains(&"E0307"));
    // A function that returns Unit needs no return at all.
    ok("fn f() { let x = 1; }");
}

#[test]
fn shadowing_is_allowed_and_uses_a_new_slot() {
    let typed = ok("fn main() { let x = 1; if true { let x = 2; } return x; }");
    assert_eq!(typed.fns[0].local_types.len(), 2);
}

#[test]
fn unknown_names_and_types() {
    assert!(codes("fn main() { return x; }").contains(&"E0300"));
    assert!(codes("fn f(a: Nope) { }").contains(&"E0310"));
    // `List<T>` is a type now; `List` on its own still is not.
    ok("fn f(a: List<Int>) { }");
    assert!(codes("fn f(a: List) { }").contains(&"E0310"));
}

#[test]
fn duplicate_function() {
    assert!(codes("fn f() { }\nfn f() { }").contains(&"E0304"));
}

#[test]
fn unsupported_v01_features() {
    assert!(codes("fn main() { let x = null; }").contains(&"E0312"));
    // Field access exists now, so this is "Int has no fields" rather than
    // "field access is not supported".
    assert!(codes("fn main() { let a = 1; return a.b; }").contains(&"E0341"));
    assert!(codes("fn f() { }\nfn main() { let x = f(); }").contains(&"E0311"));
}

// ---- record types and lists ----

const POINT: &str = "type Point { x: Int, y: Int }\n";

#[test]
fn a_record_can_be_built_and_read() {
    let typed = ok(&format!(
        "{POINT}fn main() -> Int {{ let p = Point {{ x: 1, y: 2 }}; return p.x + p.y; }}"
    ));
    assert_eq!(typed.fns[0].ret, Types::INT);
    let p = typed.fns[0].local_types[0];
    assert_eq!(typed.types.name(p), "Point");
}

#[test]
fn every_field_is_required_and_named_once() {
    assert!(codes(&format!("{POINT}fn main() {{ let p = Point {{ x: 1 }}; }}")).contains(&"E0350"));
    assert!(
        codes(&format!(
            "{POINT}fn main() {{ let p = Point {{ x: 1, y: 2, z: 3 }}; }}"
        ))
        .contains(&"E0348")
    );
    assert!(
        codes(&format!(
            "{POINT}fn main() {{ let p = Point {{ x: 1, x: 2, y: 3 }}; }}"
        ))
        .contains(&"E0349")
    );
    assert!(
        codes(&format!(
            "{POINT}fn main() {{ let p = Point {{ x: true, y: 2 }}; }}"
        ))
        .contains(&"E0301")
    );
}

#[test]
fn a_field_that_does_not_exist_is_reported() {
    assert!(
        codes(&format!(
            "{POINT}fn main() -> Int {{ let p = Point {{ x: 1, y: 2 }}; return p.z; }}"
        ))
        .contains(&"E0341")
    );
    assert!(codes("fn main() -> Int { let n = 1; return n.x; }").contains(&"E0341"));
}

#[test]
fn types_may_refer_to_each_other_in_any_order() {
    ok("type Outer { inner: Inner }\ntype Inner { n: Int }\n\
        fn main() -> Int { let o = Outer { inner: Inner { n: 1 } }; return o.inner.n; }");
}

#[test]
fn a_type_containing_itself_is_rejected() {
    assert!(codes("type Loop { next: Loop }\nfn main() { }").contains(&"E0340"));
    assert!(codes("type A { b: B }\ntype B { a: A }\nfn main() { }").contains(&"E0340"));
    // A list is a handle, so a cycle through one is finite.
    ok("type Tree { children: List<Tree> }\nfn main() { }");
    // So is an optional field, for a different reason: every value of the type
    // terminates, because the chain has to stop at a field that was not there.
    ok("type Span { expansion: Expansion? }\ntype Expansion { span: Span }\nfn main() { }");
    ok("type Loop { next: Loop? }\nfn main() { }");
}

/// The marker is on the field, and the only thing it changes about the field's
/// type is that the document need not carry it: `a.executable` is a `String`,
/// because nothing in this language holds a value that is sometimes there.
#[test]
fn an_optional_field_reads_as_its_own_type() {
    let typed = ok("type A { x: Int?, y: Int }\n\
        fn main() -> Int { let a = A { y: 1 }; return a.x + a.y; }");
    assert_eq!(typed.fns[0].ret, Types::INT);
}

/// A literal may leave an optional field out, which is the program declining
/// to give a value rather than being handed one it did not choose. A required
/// field is unchanged.
#[test]
fn a_literal_may_leave_an_optional_field_out() {
    ok("type A { x: Int?, y: Int }\nfn main() { let a = A { y: 1 }; }");
    assert!(
        codes("type A { x: Int?, y: Int }\nfn main() { let a = A { x: 1 }; }").contains(&"E0350")
    );
}

/// There is nothing to ask about a field that is always there, and `?` on
/// anything that is not a field access is not a question this language has.
#[test]
fn only_an_optional_field_can_be_asked_about() {
    ok("type A { x: Int? }\nfn main() -> Bool { let a = A { }; return a.x?; }");
    assert!(
        codes("type A { x: Int }\nfn main() -> Bool { let a = A { x: 1 }; return a.x?; }")
            .contains(&"E0343")
    );
    assert!(codes("fn main() -> Bool { let n = 1; return n?; }").contains(&"E0343"));
}

/// A `Unit` field already holds `null` and nothing else, so an optional one
/// would have no way to tell the two apart. Refusing it is what makes "absent
/// and `null` are the same thing" true of the value as well as the document.
#[test]
fn an_optional_unit_field_is_rejected() {
    assert!(codes("type A { x: Unit? }\nfn main() { }").contains(&"E0355"));
    // A required `Unit` field is untouched: it is how a program says a field
    // is always `null`, and it worked before this.
    ok("type A { x: Unit }\nfn main() { }");
}

#[test]
fn duplicate_and_builtin_type_names_are_rejected() {
    assert!(codes("type P { x: Int }\ntype P { y: Int }\nfn main() { }").contains(&"E0344"));
    assert!(codes("type Int { x: Int }\nfn main() { }").contains(&"E0345"));
    assert!(codes("type List { x: Int }\nfn main() { }").contains(&"E0345"));
    assert!(codes("type P { x: Int, x: Bool }\nfn main() { }").contains(&"E0346"));
}

#[test]
fn lists_are_homogeneous() {
    let typed = ok("fn main() -> Int { let xs = [1, 2, 3]; return xs[0]; }");
    assert_eq!(typed.types.name(typed.fns[0].local_types[0]), "List<Int>");
    assert!(codes("fn main() { let xs = [1, true]; }").contains(&"E0301"));
}

#[test]
fn an_empty_list_needs_an_annotation() {
    ok("fn main() { let xs: List<Int> = []; }");
    assert!(codes("fn main() { let xs = []; }").contains(&"E0342"));
}

#[test]
fn indexing_needs_a_list_and_an_integer() {
    assert!(codes("fn main() -> Int { let n = 1; return n[0]; }").contains(&"E0351"));
    assert!(codes("fn main() -> Int { let xs = [1]; return xs[true]; }").contains(&"E0301"));
}

#[test]
fn len_works_on_lists_and_strings() {
    ok("fn main() -> Int { let xs = [1, 2]; return len(xs); }");
    ok("fn main() -> Int { return len(\"abc\"); }");
    assert!(codes("fn main() -> Int { return len(1); }").contains(&"E0352"));
    assert!(codes("fn main() -> Int { let xs = [1]; return len(xs, xs); }").contains(&"E0302"));
    // A module that defines its own `len` gets that one.
    ok("fn len(n: Int) -> Int { return n; }\nfn main() -> Int { return len(2); }");
}

#[test]
fn nested_types_type_check_through_lists() {
    ok("type Evidence { source: String }\n\
        type Diagnosis { cause: String, evidence: List<Evidence> }\n\
        fn main() -> String {\n\
            let d = Diagnosis { cause: \"disk\", evidence: [Evidence { source: \"syslog\" }] };\n\
            return d.evidence[0].source;\n\
        }");
}

// ---- tasks ----

#[test]
fn spawn_produces_a_task_and_await_unwraps_it() {
    let typed = ok("fn work(a: Int) -> Int { return a; }\n\
        fn main() -> Int { let t = spawn work(1); return await t; }");
    assert_eq!(typed.fns[1].ret, Types::INT);
    // The local holding the task has the task type.
    let task_local = typed.fns[1].local_types[0];
    assert_eq!(typed.types.task_output(task_local), Some(Types::INT));
    assert_eq!(typed.types.name(task_local), "Task<Int>");
}

#[test]
fn a_task_type_can_be_written_down() {
    ok("fn work() -> Int { return 1; }\n\
        fn main() -> Int { let t: Task<Int> = spawn work(); return await t; }");
    assert!(
        codes(
            "fn work() -> Int { return 1; }\n\
            fn main() -> Int { let t: Task<Bool> = spawn work(); return 0; }"
        )
        .contains(&"E0301")
    );
    assert!(codes("fn f(t: Task) { }").contains(&"E0310"));
}

#[test]
fn spawn_checks_its_arguments_like_a_call() {
    assert!(
        codes("fn work(a: Int) -> Int { return a; }\nfn main() { let t = spawn work(true); }")
            .contains(&"E0301")
    );
    assert!(
        codes("fn work(a: Int) -> Int { return a; }\nfn main() { let t = spawn work(); }")
            .contains(&"E0302")
    );
    assert!(codes("fn main() { let t = spawn nope(); }").contains(&"E0300"));
}

#[test]
fn only_a_task_can_be_awaited() {
    assert!(codes("fn main() -> Int { let x = 1; return await x; }").contains(&"E0333"));
}

#[test]
fn a_capability_cannot_be_spawned() {
    // Two effects in flight at once is a broker change, not a language one.
    assert!(
        codes(&format!(
            "{ALLOW_ALL}fn main() {{ let t = spawn fs.read(\"./in.txt\"); }}"
        ))
        .contains(&"E0332")
    );
}

#[test]
fn main_cannot_return_a_task() {
    let cs =
        codes("fn work() -> Int { return 1; }\nfn main() -> Task<Int> { return spawn work(); }");
    assert!(cs.contains(&"E0331"), "{cs:?}");
}

// ---- retry and timeout ----

#[test]
fn a_policy_belongs_to_a_capability_call() {
    ok(&format!(
        "{ALLOW_ALL}fn main() {{ let t = fs.read(\"./in.txt\") retry 3 timeout 500; }}"
    ));
}

/// `retry` performs the effect again, so a program may only ask for it where
/// the manifest says performing it twice is the same as performing it once.
#[test]
fn a_retry_needs_a_grant_that_says_the_effect_can_be_repeated() {
    let cs = codes(&format!(
        "{ALLOW_ALL}fn main() -> Int {{ return process.exec(\"/usr/bin/true\") retry 3; }}"
    ));
    assert!(cs.contains(&"E0374"), "{cs:?}");

    // Saying so makes it compile, and the claim is about this grant rather
    // than about the capability: `fs.read` above says it and `process.exec`
    // does not.
    ok(&format!(
        "{ALLOW_ALL}fn main() -> String {{ return fs.read(\"./in.txt\") retry 3; }}"
    ));

    // One attempt is not a retry, so it needs nothing.
    ok(&format!(
        "{ALLOW_ALL}fn main() -> Int {{ return process.exec(\"/usr/bin/true\") retry 1; }}"
    ));

    // Nor does a deadline, which repeats nothing.
    ok(&format!(
        "{ALLOW_ALL}fn main() -> Int {{ return process.exec(\"/usr/bin/true\") timeout 50; }}"
    ));
}

#[test]
fn a_policy_on_a_function_call_is_rejected() {
    let cs = codes("fn work() -> Int { return 1; }\nfn main() -> Int { return work() retry 3; }");
    assert!(cs.contains(&"E0330"), "{cs:?}");
}

// ---- capabilities ----

const ALLOW_ALL: &str = "allow {\n  fs.read \"./in.txt\" repeatable;\n  fs.write \"./out.txt\";\n  process.exec \"/usr/bin/true\";\n}\n";

#[test]
fn a_granted_capability_can_be_called() {
    let typed = ok(&format!(
        "{ALLOW_ALL}fn main() {{\n  let text = fs.read(\"./in.txt\");\n  fs.write(\"./out.txt\", text);\n  return process.exec(\"/usr/bin/true\");\n}}\n"
    ));
    assert_eq!(typed.caps.len(), 3);
    assert_eq!(typed.caps[0].name, "fs.read");
    assert_eq!(typed.caps[0].kind, sic_core::CapKind::Read);
    assert_eq!(typed.caps[0].constraint, "./in.txt");
    // The result types come from the capability signatures.
    assert_eq!(typed.fns[0].ret, Types::INT);
}

#[test]
fn calling_an_ungranted_capability_is_a_compile_error() {
    // The manifest of a compiled module is complete by construction, so this
    // has to fail before anything runs.
    assert!(codes("fn main() { let t = fs.read(\"x\"); }").contains(&"E0320"));
}

#[test]
fn a_grant_must_name_a_real_capability() {
    assert!(codes("allow { fs.delete \"x\"; }\nfn main() { }").contains(&"E0321"));
    assert!(codes("fn main() { return fs.delete(\"x\"); }").contains(&"E0324"));
}

#[test]
fn a_grant_must_be_constrained() {
    assert!(codes("allow { fs.read; }\nfn main() { }").contains(&"E0322"));
}

#[test]
fn only_running_a_program_can_be_pinned() {
    let digest = "a".repeat(64);
    ok(&format!(
        "allow {{ process.exec \"/usr/bin/true\" sha256 \"{digest}\"; }}\n\
         fn main() -> Int {{ return process.exec(\"/usr/bin/true\"); }}"
    ));
    // Pinning what a capability reads would have to say what the contents must
    // be, which is not what a grant is for.
    assert!(
        codes(&format!(
            "allow {{ fs.read \"a\" sha256 \"{digest}\"; }}\nfn main() {{ }}"
        ))
        .contains(&"E0327")
    );
}

#[test]
fn a_pin_is_a_sha256_digest() {
    assert!(
        codes("allow { process.exec \"/x\" sha256 \"nope\"; }\nfn main() { }").contains(&"E0326")
    );
    assert!(
        codes(&format!(
            "allow {{ process.exec \"/x\" sha256 \"{}\"; }}\nfn main() {{ }}",
            "z".repeat(64)
        ))
        .contains(&"E0326")
    );
}

#[test]
fn a_capability_is_granted_once() {
    assert!(codes("allow { fs.read \"a\"; fs.read \"b\"; }\nfn main() { }").contains(&"E0323"));
}

#[test]
fn capability_arguments_are_checked() {
    assert!(codes(&format!("{ALLOW_ALL}fn main() {{ let t = fs.read(1); }}")).contains(&"E0301"));
    assert!(codes(&format!("{ALLOW_ALL}fn main() {{ let t = fs.read(); }}")).contains(&"E0302"));
}

#[test]
fn a_capability_is_not_a_value() {
    assert!(codes(&format!("{ALLOW_ALL}fn main() {{ let f = fs.read; }}")).contains(&"E0325"));
}

#[test]
fn a_local_binding_shadows_a_capability_namespace() {
    // With `fs` bound, `fs.read(..)` is a field access on a value, not a
    // capability call, and field access does not exist yet.
    let cs = codes(&format!(
        "{ALLOW_ALL}fn main() {{ let fs = 1; let t = fs.read(\"x\"); }}"
    ));
    assert!(cs.contains(&"E0308"), "{cs:?}");
}

#[test]
fn a_capability_returning_unit_cannot_be_bound() {
    assert!(
        codes(&format!(
            "{ALLOW_ALL}fn main() {{ let x = fs.write(\"a\", \"b\"); }}"
        ))
        .contains(&"E0311")
    );
}

#[test]
fn one_error_does_not_cascade() {
    // `nope` is unknown; using its result must not add more diagnostics.
    let cs = codes("fn main() { return nope() + 1 * 2; }");
    assert_eq!(cs, vec!["E0300"]);
}

// ---- agents ----

const AGENT: &str = "type Diagnosis { cause: String }\n\
allow { llm.invoke \"a-model\"; }\n\
agent diagnose { input: String, output: Diagnosis, budget: 2 }\n";

#[test]
fn an_agent_is_called_like_a_function_and_returns_its_output_type() {
    // A field of the answer is still the answer, so the return type says so.
    let typed = ok(&format!(
        "{AGENT}fn main() -> LLM<String> {{ let d = diagnose(\"logs\"); return d.cause; }}"
    ));
    assert_eq!(typed.agents.len(), 1);
    assert_eq!(typed.agents[0].name, "diagnose");
    assert_eq!(typed.agents[0].budget, Some(2));
    assert_eq!(typed.types.name(typed.agents[0].output), "Diagnosis");
}

#[test]
fn an_agent_needs_the_capability_to_talk_to_a_model() {
    // There is no path to an effect the manifest does not name.
    let cs = codes(
        "type D { cause: String }\nagent diagnose { input: String, output: D }\nfn main() { }",
    );
    assert!(cs.contains(&"E0362"), "{cs:?}");
}

#[test]
fn an_agent_takes_a_prompt() {
    assert!(codes(&format!("{AGENT}fn main() {{ let d = diagnose(1); }}")).contains(&"E0301"));
    assert!(codes(&format!("{AGENT}fn main() {{ let d = diagnose(); }}")).contains(&"E0302"));
    // A non-String input has no way to become a prompt yet.
    assert!(
        codes(
            "allow { llm.invoke \"m\"; }\ntype D { c: String }\n\
               agent a { input: Int, output: D }\nfn main() { }"
        )
        .contains(&"E0363")
    );
}

#[test]
fn an_agent_needs_both_an_input_and_an_output() {
    assert!(
        codes("allow { llm.invoke \"m\"; }\nagent a { input: String }\nfn main() { }")
            .contains(&"E0364")
    );
}

#[test]
fn an_agent_cannot_share_a_name_with_a_function() {
    let cs = codes(&format!(
        "{AGENT}fn diagnose(s: String) -> String {{ return s; }}\nfn main() {{ }}"
    ));
    assert!(cs.contains(&"E0361"), "{cs:?}");
}

/// Asking again is a second model call, so it needs the claim a second call of
/// anything needs. The same code as `retry N` after a capability call, because
/// it is the same question about the same grant.
#[test]
fn an_agent_may_only_retry_where_the_grant_says_the_effect_repeats() {
    let base = "type D { cause: String }\n";
    let cs = codes(&format!(
        "{base}allow {{ llm.invoke \"m\"; }}\n\
         agent a {{ input: String, output: D, retry: 3 }}\nfn main() {{ }}"
    ));
    assert!(cs.contains(&"E0374"), "{cs:?}");

    let typed = ok(&format!(
        "{base}allow {{ llm.invoke \"m\" repeatable; }}\n\
         agent a {{ input: String, output: D, retry: 3 }}\nfn main() {{ }}"
    ));
    assert_eq!(typed.agents[0].retry, Some(3));

    // One attempt repeats nothing, so it needs nothing.
    ok(&format!(
        "{base}allow {{ llm.invoke \"m\"; }}\n\
         agent a {{ input: String, output: D, retry: 1 }}\nfn main() {{ }}"
    ));
}

/// `retry` after an agent call is still refused, and the note now names the
/// field that replaces it - which is the whole of what E0330 is for here.
#[test]
fn a_retry_after_an_agent_call_points_at_the_declaration() {
    let cs = codes(&format!(
        "{AGENT}fn main() {{ let d = diagnose(\"logs\") retry 3; }}"
    ));
    assert!(cs.contains(&"E0330"), "{cs:?}");
}

// ---- trust and provenance ----

const TRUST: &str = "type Plan { action: String }\n\
allow { llm.invoke \"m\"; human.approve \"deploying\"; process.exec \"/usr/bin/true\"; }\n\
agent make_plan { input: String, output: Plan }\n";

#[test]
fn an_agents_answer_carries_where_it_came_from() {
    let typed = ok(&format!("{TRUST}fn main() {{ let p = make_plan(\"x\"); }}"));
    assert_eq!(typed.types.name(typed.fns[0].local_types[0]), "LLM<Plan>");
}

#[test]
fn approve_is_the_only_way_to_produce_a_human_approved_value() {
    let typed = ok(&format!(
        "{TRUST}fn main() {{ let p = make_plan(\"x\"); let a = approve(\"ok?\", p); }}"
    ));
    assert_eq!(
        typed.types.name(typed.fns[0].local_types[1]),
        "HumanApproved<Plan>"
    );
    // Asking a person is an effect like any other.
    assert!(
        codes(
            "type P { a: String }\nallow { llm.invoke \"m\"; }\n\
               agent f { input: String, output: P }\n\
               fn main() { let p = f(\"x\"); let a = approve(\"ok?\", p); }"
        )
        .contains(&"E0370")
    );
}

#[test]
fn the_specification_example_is_a_compile_error() {
    // `deploy(LLM<Plan>)` must not compile; `deploy(HumanApproved<Plan>)` must.
    ok(&format!(
        "{TRUST}fn deploy(p: HumanApproved<Plan>) -> Int {{ return 1; }}\n\
         fn main() -> Int {{\n\
             let p = make_plan(\"x\");\n\
             return deploy(approve(\"ok?\", p));\n\
         }}"
    ));
    let cs = codes(&format!(
        "{TRUST}fn deploy(p: HumanApproved<Plan>) -> Int {{ return 1; }}\n\
         fn main() -> Int {{ let p = make_plan(\"x\"); return deploy(p); }}"
    ));
    assert!(cs.contains(&"E0301"), "{cs:?}");
}

#[test]
fn provenance_follows_a_field() {
    // A field of a model's answer is still the model's answer.
    let typed = ok(&format!(
        "{TRUST}fn main() {{ let p = make_plan(\"x\"); let a = p.action; }}"
    ));
    assert_eq!(typed.types.name(typed.fns[0].local_types[1]), "LLM<String>");
}

/// A `for` binding is the element `xs[i]` would have reached, so it carries
/// where the list came from. `docs/design/trust.md` §2a is what both agree
/// with, and they agree by sharing the rule rather than by both remembering it.
#[test]
fn provenance_follows_a_for_binding() {
    let src = "type Plan { steps: List<String> }\n\
               allow { llm.invoke \"m\"; }\n\
               agent make_plan { input: String, output: Plan }\n\
               fn main() { let p = make_plan(\"x\"); for s in p.steps { log info s; } }";
    let typed = ok(src);
    // Local 0 is `p`, local 1 is the binding: `p.steps` is an expression and
    // never gets a slot of its own.
    assert_eq!(typed.types.name(typed.fns[0].local_types[1]), "LLM<String>");

    // And the label is not something the loop takes off on the way in.
    let cs = codes(
        "type Plan { steps: List<String> }\n\
         allow { llm.invoke \"m\"; }\n\
         agent make_plan { input: String, output: Plan }\n\
         fn join(a: String) -> String { return a; }\n\
         fn main() { let p = make_plan(\"x\"); for s in p.steps { join(s); } }",
    );
    assert!(cs.contains(&"E0301"), "{cs:?}");
}

/// A `return` inside a loop is not a return on every path. The list can be
/// empty, and then the body never ran - which is the one thing a loop can do
/// that an `if` with both arms cannot.
#[test]
fn a_return_inside_a_loop_is_not_a_return_on_every_path() {
    let cs = codes("fn f(xs: List<Int>) -> Int { for x in xs { return x; } }");
    assert!(cs.contains(&"E0307"), "{cs:?}");
    ok("fn f(xs: List<Int>) -> Int { for x in xs { return x; } return 0; }");
}

/// Only a list. A `String` has a length and a `for` over one would have to
/// invent what an element of it is.
#[test]
fn only_a_list_can_be_walked() {
    let cs = codes("fn main() { for c in \"abc\" { log info c; } }");
    assert!(cs.contains(&"E0354"), "{cs:?}");
}

#[test]
fn a_trusted_value_is_not_its_inner_type() {
    // Arithmetic is exactly where provenance gets lost: it answers a value of
    // the operand's own kind, which the label is not on. A comparison answers a
    // `Bool` about the operand, which cannot be one - see `trust.md` §2a.
    let typed = ok(&format!(
        "{TRUST}fn main() -> Bool {{ let p = make_plan(\"x\"); return p.action == \"go\"; }}"
    ));
    assert_eq!(typed.types.name(typed.fns[0].ret), "Bool");
    assert!(
        codes(&format!(
            "{TRUST}fn main() {{ let p = make_plan(\"x\"); let n = !p; }}"
        ))
        .contains(&"E0371")
    );
    // Joining two strings is the exception, and it is one because its answer
    // is still labelled - writing `String` there is E0301. See
    // `docs/design/trust.md` §2a.
    let typed = ok(&format!(
        "{TRUST}fn main() -> LLM<String> {{ let p = make_plan(\"x\"); return p.action + \"!\"; }}"
    ));
    assert_eq!(typed.types.name(typed.fns[0].ret), "LLM<String>");
    assert!(
        codes(&format!(
            "{TRUST}fn main() -> String {{ let p = make_plan(\"x\"); return p.action + \"!\"; }}"
        ))
        .contains(&"E0301")
    );
}

#[test]
fn a_models_answer_cannot_reach_a_capability_that_changes_something() {
    let cs = codes(&format!(
        "{TRUST}fn main() -> Int {{\n\
             let p = make_plan(\"x\");\n\
             return process.exec(p.action);\n\
         }}"
    ));
    assert!(cs.contains(&"E0372"), "{cs:?}");
}

#[test]
fn asking_a_model_about_a_models_answer_is_ordinary() {
    // The rule is about the capability's kind, not about the value.
    ok(&format!(
        "{TRUST}fn main() {{\n\
             let p = make_plan(\"x\");\n\
             let again = make_plan(p.action);\n\
         }}"
    ));
}

#[test]
fn a_trust_type_needs_its_argument() {
    assert!(codes("fn f(p: LLM) { }").contains(&"E0310"));
}
