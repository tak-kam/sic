//! `sic export <JOURNAL> [--traces PATH] [--metrics PATH]`.
//!
//! The journal is the canonical record; this is a view of it. Nothing is sent
//! anywhere: sending telemetry is an external effect, and an external effect is
//! a capability. What comes out is a document, and getting it to a collector is
//! somebody else's job.

use crate::out::sayln;

use std::process::ExitCode;

use sic_otel::Resource;

use super::{EXIT_FAILURE, EXIT_USAGE, read_bytes};

pub struct ExportOptions<'a> {
    pub traces: Option<&'a str>,
    pub metrics: Option<&'a str>,
}

pub fn run(journal_path: &str, options: ExportOptions<'_>) -> ExitCode {
    let bytes = match read_bytes(journal_path) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        eprintln!("error: `{journal_path}` is not valid UTF-8");
        return ExitCode::from(EXIT_USAGE);
    };

    let result = sic_journal::read_jsonl(&text);
    // A run killed mid-write leaves a fragment. Exporting the rest and saying
    // how much was dropped beats refusing the whole journal.
    for skipped in &result.skipped {
        eprintln!("warning: skipped {skipped}");
    }
    if result.events.is_empty() {
        eprintln!("error: `{journal_path}` has no journal events");
        return ExitCode::from(EXIT_FAILURE);
    }

    let resource = Resource::default();
    let traces = sic_otel::traces(&result.events, &resource);
    let metrics = sic_otel::metrics(&result.events, &resource);

    match (options.traces, options.metrics) {
        (None, None) => sayln!("{traces}"),
        (traces_path, metrics_path) => {
            if let Some(path) = traces_path {
                if let Err(code) = write(path, &traces) {
                    return code;
                }
            }
            if let Some(path) = metrics_path {
                if let Err(code) = write(path, &metrics) {
                    return code;
                }
            }
        }
    }
    ExitCode::SUCCESS
}

fn write(path: &str, document: &str) -> Result<(), ExitCode> {
    match std::fs::write(path, document) {
        Ok(()) => {
            eprintln!("wrote {path} ({} bytes)", document.len());
            Ok(())
        }
        Err(e) => {
            eprintln!("error: cannot write `{path}`: {e}");
            Err(ExitCode::from(EXIT_FAILURE))
        }
    }
}
