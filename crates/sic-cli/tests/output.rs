//! Nothing in this crate writes to standard output with `println!`.
//!
//! `println!` panics when the write fails, and it fails whenever whatever was
//! reading closes the pipe first. `crates/sic-cli/src/out.rs` says why that is
//! the wrong behaviour for this program and what replaces it. This test is here
//! because the fix is only a fix if it is everywhere: one `println!` that got
//! missed, or one added later by somebody who reached for the macro every Rust
//! program uses, is the panic back in one place - and it is a place nobody will
//! find until they pipe that command into `head`.
//!
//! `eprintln!` is untouched and stays. A diagnostic nobody is reading is a
//! different thing from a result nobody is reading, and a diagnostic that
//! vanished would be worse than a panic.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the source directory should exist") {
        let path = entry.expect("a readable entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `println!` and `print!`, where neither is the tail of a longer word - so
/// `eprintln!` does not match, and neither does a mention inside a name.
fn writes_to_stdout(line: &str) -> bool {
    for macro_name in ["println!", "print!"] {
        let mut rest = line;
        while let Some(at) = rest.find(macro_name) {
            let before = rest[..at].chars().next_back();
            if !before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                return true;
            }
            rest = &rest[at + macro_name.len()..];
        }
    }
    false
}

#[test]
fn nothing_prints_with_a_macro_that_panics_on_a_closed_pipe() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty());

    let mut findings = Vec::new();
    for file in &files {
        // `out.rs` is where the replacement lives, and it names both macros in
        // its own documentation.
        if file.file_name().is_some_and(|n| n == "out.rs") {
            continue;
        }
        let text = std::fs::read_to_string(file).expect("source should be readable");
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if writes_to_stdout(line) {
                findings.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        findings.is_empty(),
        "use `sayln!` and `say!` from `crate::out`, which let a reader stop \
         reading:\n{}",
        findings.join("\n")
    );
}

#[test]
fn the_test_can_tell_the_two_families_apart() {
    assert!(writes_to_stdout("    println!(\"x\");"));
    assert!(writes_to_stdout("    print!(\"x\");"));
    assert!(!writes_to_stdout("    eprintln!(\"x\");"));
    assert!(!writes_to_stdout("    eprint!(\"x\");"));
    // A name that ends in one of them is not a call to one.
    assert!(!writes_to_stdout("    self.pretty_print!(x);"));
}
