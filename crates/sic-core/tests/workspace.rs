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
///
/// `std::{` is here because a grouped import contains none of the module paths
/// above: `use std::{fs, process};` is the ordinary way to write that line, not
/// a contrived bypass. Spelling the imports out one per line is the price of
/// having this check mean something.
///
/// The macros reach outside while a crate is built rather than while it runs,
/// which is no less outside: a file read at compile time is still a file the
/// manifest never named.
const EXTERNAL: &[&str] = &[
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::time",
    "std::io",
    "std::{",
    "SystemTime",
    "Instant",
    "include_str!",
    "include_bytes!",
    "env!",
    "option_env!",
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
                    if !line.contains(pattern) {
                        continue;
                    }
                    // `env!("CARGO_PKG_VERSION")` is Cargo substituting the
                    // manifest into the build. The crate is not reading the
                    // environment a run happens in, which is what the rule is
                    // about.
                    if pattern.ends_with("env!") && line.contains("env!(\"CARGO_") {
                        continue;
                    }
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
    assert!(
        findings.is_empty(),
        "only {} may reach outside:\n{}",
        MAY_REACH_OUTSIDE.join(" and "),
        findings.join("\n")
    );
}

#[test]
fn sic_core_depends_on_nothing_else_in_the_workspace() {
    // The third of the three boundaries. `sic-core` is what every other crate
    // depends on, so a dependency it takes is one the whole workspace takes,
    // whether or not any of them asked for it - and it is the crate a supply
    // chain attack would most want. The manifest is right today; a check is
    // what keeps it right, which is the difference between a rule and a
    // description of the current state.
    let manifest = workspace().join("crates/sic-core/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).expect("sic-core should have a manifest");

    // No section at all would satisfy the rule too, so its absence is not a
    // failure.
    let declared: Vec<&str> = match text.split("[dependencies]").nth(1) {
        Some(section) => section
            .lines()
            .map(str::trim)
            // Entries run until the next section header.
            .take_while(|line| !line.starts_with('['))
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
        None => Vec::new(),
    };
    assert!(
        declared.is_empty(),
        "sic-core must depend on nothing: every other crate depends on it, so a \
         dependency here is one the whole workspace has. {} declares:\n{}",
        manifest.display(),
        declared.join("\n")
    );

    // A dev-dependency or a build-dependency on a workspace crate would make
    // the bottom of the graph point back up into it, so the whole manifest is
    // checked rather than one section of it.
    let upward: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("path = \"../sic-"))
        .collect();
    assert!(
        upward.is_empty(),
        "sic-core must not depend on another workspace crate in any section, \
         including dev-dependencies: {}",
        upward.join(", ")
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

    // And once each. A code names one error, so two rows for one code is two
    // answers to the question a person greps this file with - which the two
    // checks above are both satisfied by, since a duplicate is neither missing
    // nor stale.
    let mut seen = listed.clone();
    seen.sort();
    let mut twice: Vec<String> = Vec::new();
    for pair in seen.windows(2) {
        if pair[0] == pair[1] && !twice.contains(&pair[0]) {
            twice.push(pair[0].clone());
        }
    }
    assert!(
        twice.is_empty(),
        "these codes are listed more than once in docs/diagnostics.md: {twice:?}"
    );
}
