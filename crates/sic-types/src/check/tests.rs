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
    assert!(codes("fn f(a: List<Int>) { }").contains(&"E0310"));
}

#[test]
fn duplicate_function() {
    assert!(codes("fn f() { }\nfn f() { }").contains(&"E0304"));
}

#[test]
fn unsupported_v01_features() {
    assert!(codes("fn main() { let x = null; }").contains(&"E0312"));
    assert!(codes("fn main() { let a = 1; return a.b; }").contains(&"E0308"));
    assert!(codes("fn f() { }\nfn main() { let x = f(); }").contains(&"E0311"));
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
