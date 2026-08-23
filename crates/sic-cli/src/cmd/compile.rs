//! `sic compile <FILE.sic> [-o OUT]`: write bytecode to a file.

use std::process::ExitCode;

use super::{EXIT_FAILURE, compile_source};

pub fn run(path: &str, output: Option<&str>) -> ExitCode {
    let program = match compile_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let out_path = match output {
        Some(p) => p.to_string(),
        None => default_output_path(path),
    };
    let bytes = sic_bytecode::encode(&program);
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        eprintln!("error: cannot write `{out_path}`: {e}");
        return ExitCode::from(EXIT_FAILURE);
    }
    println!("wrote {out_path} ({} bytes)", bytes.len());
    ExitCode::SUCCESS
}

/// `main.sic` becomes `main.sicb`.
fn default_output_path(input: &str) -> String {
    match input.strip_suffix(".sic") {
        Some(stem) => format!("{stem}.sicb"),
        None => format!("{input}.sicb"),
    }
}
