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
fn operators_are_int_or_bool_only() {
    assert!(codes("fn main() { return 1 + true; }").contains(&"E0303"));
    assert!(codes("fn main() { return 1.5 + 2.5; }").contains(&"E0303"));
    assert!(codes("fn main() { return \"a\" + \"b\"; }").contains(&"E0303"));
    assert!(codes("fn main() { return true < false; }").contains(&"E0303"));
    assert!(codes("fn main() { return -true; }").contains(&"E0303"));
    assert!(codes("fn main() { return !1; }").contains(&"E0303"));
    ok("fn main() { return true == false; }");
    ok("fn main() { return 1 < 2 && 3 >= 4; }");
}

#[test]
fn float_and_string_are_values_but_have_no_operators() {
    let typed = ok("fn main() -> Float { let x = 1.5; return x; }");
    assert_eq!(typed.fns[0].ret, Types::FLOAT);
    let typed = ok("fn main() -> String { let s = \"hi\"; return s; }");
    assert_eq!(typed.fns[0].ret, Types::STR);
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

#[test]
fn a_policy_on_a_function_call_is_rejected() {
    let cs = codes("fn work() -> Int { return 1; }\nfn main() -> Int { return work() retry 3; }");
    assert!(cs.contains(&"E0330"), "{cs:?}");
}

// ---- capabilities ----

const ALLOW_ALL: &str = "allow {\n  fs.read \"./in.txt\";\n  fs.write \"./out.txt\";\n  process.exec \"/usr/bin/true\";\n}\n";

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

#[test]
fn a_trusted_value_is_not_its_inner_type() {
    // Arithmetic is exactly where provenance gets lost.
    assert!(
        codes(&format!(
            "{TRUST}fn main() -> String {{ let p = make_plan(\"x\"); return p.action + \"!\"; }}"
        ))
        .contains(&"E0371")
    );
    assert!(
        codes(&format!(
            "{TRUST}fn main() {{ let p = make_plan(\"x\"); let n = !p; }}"
        ))
        .contains(&"E0371")
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
