//! Checks about the workspace as a whole.
//!
//! These live in `sic-core` because it is what every other crate depends on,
//! and because a rule about the workspace needs one place to be checked rather
//! than a copy per crate.

use std::path::{Path, PathBuf};

/// The workspace root, found by walking up from this crate.
fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("sic-core sits two levels below the workspace root")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every crate's `src`, paired with the crate's name.
fn crate_sources() -> Vec<(String, Vec<PathBuf>)> {
    let crates = workspace().join("crates");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates)
        .expect("crates/ should exist")
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut files = Vec::new();
        rust_files(&entry.path().join("src"), &mut files);
        if !files.is_empty() {
            out.push((name, files));
        }
    }
    out.sort();
    out
}

/// Reading external state, or changing it.
const EXTERNAL: &[&str] = &[
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::time",
    "SystemTime",
    "Instant",
];

/// The crates that are allowed to reach outside.
///
/// `sic-broker` performs the effects a capability names, and `sic-cli` is the
/// program a person runs. Everything else is a pure function of its input, and
/// that is what makes the capability boundary mean anything: an effect the
/// manifest does not name has nowhere to come from.
const MAY_REACH_OUTSIDE: &[&str] = &["sic-broker", "sic-cli"];

#[test]
fn only_the_broker_and_the_cli_touch_the_outside_world() {
    let mut findings = Vec::new();
    for (name, files) in crate_sources() {
        if MAY_REACH_OUTSIDE.contains(&name.as_str()) {
            continue;
        }
        for file in files {
            let text = std::fs::read_to_string(&file).expect("source should be readable");
            for (number, line) in text.lines().enumerate() {
                // A mention in a comment is a description, not a use.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for pattern in EXTERNAL {
                    if line.contains(pattern) {
                        findings.push(format!(
                            "{name}: {}:{}: {}",
                            file.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        findings.is_empty(),
        "only {} may reach outside:\n{}",
        MAY_REACH_OUTSIDE.join(" and "),
        findings.join("\n")
    );
}

#[test]
fn every_diagnostic_code_is_in_the_index() {
    // An index that drifts is worse than none, so it is checked rather than
    // maintained by hand.
    let mut used: Vec<String> = Vec::new();
    for (_, files) in crate_sources() {
        for file in files {
            // A code in a test is a code being tested, not one being defined.
            if file.file_name().is_some_and(|n| n == "tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("source should be readable");
            let mut rest = text.as_str();
            while let Some(at) = rest.find("\"E0") {
                let candidate = &rest[at + 1..];
                if candidate.len() >= 6 && candidate.as_bytes()[5] == b'"' {
                    let code = &candidate[..5];
                    if code[1..].chars().all(|c| c.is_ascii_digit()) {
                        used.push(code.to_string());
                    }
                }
                rest = &rest[at + 3..];
            }
        }
    }
    used.sort();
    used.dedup();
    // The two the diagnostic renderer's own tests use are not real codes.
    used.retain(|c| c != "E0001" && c != "E0100");

    let index = std::fs::read_to_string(workspace().join("docs/diagnostics.md"))
        .expect("docs/diagnostics.md should exist");

    let missing: Vec<&String> = used
        .iter()
        .filter(|code| !index.contains(code.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "these codes are reported but not in docs/diagnostics.md: {missing:?}"
    );

    // And the other way: an entry for a code nothing reports is a promise the
    // compiler does not keep.
    let mut listed = Vec::new();
    let mut rest = index.as_str();
    while let Some(at) = rest.find("| E0") {
        let code = &rest[at + 2..at + 7];
        if code[1..].chars().all(|c| c.is_ascii_digit()) {
            listed.push(code.to_string());
        }
        rest = &rest[at + 3..];
    }
    let stale: Vec<&String> = listed.iter().filter(|code| !used.contains(code)).collect();
    assert!(
        stale.is_empty(),
        "these codes are in docs/diagnostics.md but nothing reports them: {stale:?}"
    );
}
