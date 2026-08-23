//! The JSON parser reads untrusted text and must not do anything else with it.
//!
//! It is on the path from a model's answer to a value, so it has no business
//! touching a file, a clock, or a process.

use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::time",
    "std::io",
    "SystemTime",
    "Instant",
    "unsafe",
];

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

#[test]
fn the_parser_only_parses() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty());

    let mut findings = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("source should be readable");
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for pattern in FORBIDDEN {
                if line.contains(pattern) {
                    findings.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        findings.is_empty(),
        "sic-json must only parse:\n{}",
        findings.join("\n")
    );
}
