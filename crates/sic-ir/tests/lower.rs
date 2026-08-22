use sic_ir::lower::compile_to_hir;

fn hir(src: &str) -> String {
    match compile_to_hir(src) {
        Ok(h) => sic_ir::dump(&h),
        Err(diags) => panic!("unexpected diagnostics: {diags:#?}"),
    }
}

#[test]
fn milestone_lowers_to_one_block() {
    let src = "fn main() {\n    let x = 10;\n    let y = x + 20;\n    return y;\n}\n";
    assert_eq!(
        hir(src),
        "\
consts:
  k0 = 10
  k1 = 20

fn main/0:
  bb0:
    %2 = const k0
    %0 = move %2
    %3 = const k1
    %4 = add %0 %3
    %1 = move %4
    return %1
"
    );
}

#[test]
fn reading_a_variable_costs_no_instruction() {
    // `return x;` reuses the local rather than copying it.
    let out = hir("fn main() { let x = 1; return x; }");
    assert!(out.contains("return %0"), "{out}");
}

#[test]
fn if_else_becomes_blocks() {
    let out = hir("fn f(a: Int) -> Int { if a > 0 { return 1; } else { return 2; } }");
    assert!(out.contains("branch %"), "{out}");
    // Both arms return, so the join block is unreachable but still present.
    assert_eq!(out.matches("return").count(), 3, "{out}");
}

#[test]
fn short_circuit_and_branches() {
    let out = hir("fn f(a: Bool, b: Bool) -> Bool { return a && b; }");
    // The result is written before the branch so it is defined on both paths.
    assert!(out.contains("%2 = move %0"), "{out}");
    assert!(out.contains("branch %0"), "{out}");
    assert!(out.contains("%2 = move %1"), "{out}");
}

#[test]
fn constants_are_deduplicated() {
    let out = hir("fn f() -> Int { return 7 + 7; }");
    assert_eq!(out.matches("k0 = 7").count(), 1, "{out}");
    assert!(!out.contains("k1"), "{out}");
}

#[test]
fn calls_lower_to_a_call_instruction() {
    let out =
        hir("fn add(a: Int, b: Int) -> Int { return a + b; }\nfn main() { return add(1, 2); }");
    assert!(out.contains("call f0(%0, %1)"), "{out}");
}

#[test]
fn a_function_without_return_ends_in_return_unit() {
    let out = hir("fn f() { let x = 1; }");
    assert!(out.trim_end().ends_with("return"), "{out}");
}
