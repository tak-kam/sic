//! The journal is part of what makes a run reproducible, so it must not observe
//! anything the run does not.
//!
//! In particular it must not read a clock: a timestamp is an observation a sink
//! adds from outside, and `seq` is what orders events. If the journal read the
//! time itself, two runs of the same program would stop producing the same
//! stream, and replay would depend on when it happened.

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
fn the_journal_reads_no_clock_and_opens_no_file() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty(), "no source files found under {src:?}");

    let mut findings = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("source should be readable");
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for pattern in FORBIDDEN {
                if line.contains(pattern) {
                    findings.push(format!("{}:{}: {}", file.display(), number + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        findings.is_empty(),
        "sic-journal must not observe anything outside a run:\n{}",
        findings.join("\n")
    );
}
