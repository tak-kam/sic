//! The VM's isolation is an invariant, so it is checked rather than trusted.
//!
//! `sic-vm` must not be able to reach the outside world: no files, no network,
//! no processes, no environment, no clock. Everything external will arrive as a
//! capability, brokered by a component the VM does not depend on. A grep is a
//! blunt instrument, but it catches the way this property is actually lost -
//! someone adds a convenient `std::fs::read` and nobody notices.

use std::path::{Path, PathBuf};

/// Modules that would let the VM observe or change something outside itself.
const FORBIDDEN: &[&str] = &[
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::time",
    "std::io",
    // A grouped import contains none of the paths above: `use std::{fs,
    // process};` is how a person writes that line without thinking about it.
    "std::{",
    "SystemTime",
    "Instant",
    // Reaching outside while the crate is built is no less outside.
    "include_str!",
    "include_bytes!",
    "env!",
    "option_env!",
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
fn the_vm_cannot_reach_the_outside_world() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty(), "no source files found under {src:?}");

    let mut findings = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("source should be readable");
        for (number, line) in text.lines().enumerate() {
            // The test module builds programs to run; it is not part of the VM.
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
        "sic-vm must not perform external effects:\n{}",
        findings.join("\n")
    );
}

#[test]
fn the_vm_has_no_dependencies_outside_the_workspace() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = std::fs::read_to_string(manifest).expect("the manifest should be readable");
    let deps = text
        .split("[dependencies]")
        .nth(1)
        .expect("the manifest should declare a dependencies section");
    for line in deps.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
            if line.starts_with('[') {
                break;
            }
            continue;
        }
        assert!(
            line.contains("path = \"../sic-"),
            "sic-vm may only depend on workspace crates, found: {line}"
        );
    }
}

#[test]
fn the_vm_does_not_depend_on_the_capability_broker() {
    // The VM and the broker are the two halves of the future process split. If
    // the VM ever depends on the broker, that boundary is gone.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = std::fs::read_to_string(manifest).expect("the manifest should be readable");
    assert!(!text.contains("sic-broker"));
}
