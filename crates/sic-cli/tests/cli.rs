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

#[test]
fn version_and_help() {
    let (stdout, _, code) = sic(&["version"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("sic 0.1.0"), "{stdout}");

    let (stdout, _, code) = sic(&["help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("sic parse"), "{stdout}");
}
