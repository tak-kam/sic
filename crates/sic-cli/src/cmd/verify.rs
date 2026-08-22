//! `sic verify <FILE.sicb>`: check that bytecode is safe to run.

use std::process::ExitCode;

use sic_bytecode::Program;

use super::{EXIT_FAILURE, load_bytecode};

pub fn run(path: &str) -> ExitCode {
    let program = match load_bytecode(path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    report(&program)
}

/// Prints the verifier's findings and the capabilities the module declares.
pub fn report(program: &Program) -> ExitCode {
    let result = sic_verify::verify(program);
    for w in &result.warnings {
        eprintln!("warning: {w}");
    }
    for e in &result.errors {
        eprintln!("error: {e}");
    }
    if !result.ok() {
        let n = result.errors.len();
        let plural = if n == 1 { "error" } else { "errors" };
        eprintln!("verification failed with {n} {plural}");
        return ExitCode::from(EXIT_FAILURE);
    }

    println!("ok: {} function(s) verified", program.funcs.len());
    println!("required capabilities:");
    if program.caps.is_empty() {
        println!("  (none)");
    }
    for cap in &program.caps {
        println!("  {} ({:?})", cap.name, cap.kind);
    }
    ExitCode::SUCCESS
}
