//! End-to-end tests that run the built binary.

use std::process::Command;

fn sic(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_sic"))
        .args(args)
        .output()
        .expect("failed to run sic");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Writes a source file to a temporary path (no tempfile crate).
fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("sic-test-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn parses_the_milestone_example() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/milestone.sic");
    let (stdout, stderr, code) = sic(&["parse", path]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        stdout,
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
fn reports_errors_with_source_location_and_exit_code_1() {
    let path = write_temp("bad.sic", "fn main() {\n    let y = x + ;\n}\n");
    let (_, stderr, code) = sic(&["parse", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0204"), "{stderr}");
    assert!(stderr.contains(":2:17"), "{stderr}");
    assert!(stderr.contains("aborting due to 1 error"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn rejects_bom() {
    let path = write_temp("bom.sic", "\u{FEFF}fn main() {}\n");
    let (_, stderr, code) = sic(&["parse", path.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("BOM"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn missing_file_is_a_usage_error() {
    let (_, stderr, code) = sic(&["parse", "/nonexistent/whatever.sic"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("cannot read"), "{stderr}");
}

#[test]
fn no_args_prints_usage() {
    let (_, stderr, code) = sic(&[]);
    assert_eq!(code, 2);
    assert!(stderr.contains("Usage:"), "{stderr}");
}

// ---- the whole pipeline: source -> bytecode -> verifier -> VM ----

fn example(name: &str) -> String {
    format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn runs_the_milestone_example() {
    let (stdout, stderr, code) = sic(&["run", &example("milestone.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "30\n");
}

#[test]
fn runs_recursion_and_short_circuit_examples() {
    let (stdout, stderr, code) = sic(&["run", &example("factorial.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "3628800\n");

    let (stdout, stderr, code) = sic(&["run", &example("branching.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "1\n");
}

#[test]
fn a_type_error_stops_the_run() {
    let path = write_temp("type-error.sic", "fn main() {\n    return 1 + true;\n}\n");
    let (stdout, stderr, code) = sic(&["run", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("E0303"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn a_runtime_failure_names_the_source_position() {
    let path = write_temp(
        "div0.sic",
        "fn main() {\n    let n = 0;\n    return 10 / n;\n}\n",
    );
    let (_, stderr, code) = sic(&["run", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("division by zero"), "{stderr}");
    // The debug section maps the failing instruction back to the source.
    assert!(stderr.contains(":3:12"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn running_a_module_without_main_is_an_error() {
    let path = write_temp("nomain.sic", "fn helper() -> Int { return 1; }\n");
    let (_, stderr, code) = sic(&["run", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("no `main`"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn compile_then_verify_then_disassemble() {
    let src = write_temp("pipeline.sic", "fn main() {\n    return 6 * 7;\n}\n");
    let out = src.with_extension("sicb");
    let out_str = out.to_str().unwrap().to_string();

    let (stdout, stderr, code) = sic(&["compile", src.to_str().unwrap(), "-o", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.starts_with("wrote "), "{stdout}");

    let (stdout, stderr, code) = sic(&["verify", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("1 function(s) verified"), "{stdout}");
    assert!(stdout.contains("required capabilities:"), "{stdout}");

    let (stdout, _, code) = sic(&["disasm", &out_str]);
    assert_eq!(code, 0);
    assert!(stdout.contains("MUL_I64"), "{stdout}");
    assert!(stdout.contains("RETURN"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn verify_rejects_a_corrupted_file() {
    let src = write_temp("corrupt.sic", "fn main() {\n    return 1;\n}\n");
    let out = src.with_extension("sicb");
    let out_str = out.to_str().unwrap().to_string();
    let (_, _, code) = sic(&["compile", src.to_str().unwrap(), "-o", &out_str]);
    assert_eq!(code, 0);

    let mut bytes = std::fs::read(&out).unwrap();
    bytes[0] = b'X'; // break the magic
    std::fs::write(&out, &bytes).unwrap();

    let (_, stderr, code) = sic(&["verify", &out_str]);
    assert_eq!(code, 1);
    assert!(stderr.contains("magic"), "{stderr}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn hir_prints_the_intermediate_representation() {
    let (stdout, stderr, code) = sic(&["hir", &example("milestone.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("fn main/0:"), "{stdout}");
    assert!(stdout.contains("bb0:"), "{stdout}");
}

#[test]
fn version_and_help() {
    let (stdout, _, code) = sic(&["version"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("sic 0.1.0"), "{stdout}");

    let (stdout, _, code) = sic(&["help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("sic run"), "{stdout}");
    assert!(stdout.contains("sic parse"), "{stdout}");
}
