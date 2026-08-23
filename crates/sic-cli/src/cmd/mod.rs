pub mod compile;
pub mod disasm;
pub mod drive;
pub mod export;
pub mod hir;
pub mod journal;
pub mod parse;
pub mod plan;
pub mod resume;
pub mod run;
pub mod runs;
pub mod store;
pub mod upgrade;
pub mod verify;

use std::process::ExitCode;

use crate::module::Loaded;
use sic_bytecode::Program;
use sic_core::{Diagnostic, SourceFile, SourceMap};

/// Exit code 2: the command line, or the file named on it, is wrong.
pub const EXIT_USAGE: u8 = 2;
/// Exit code 1: the program has errors, or running it failed.
pub const EXIT_FAILURE: u8 = 1;
/// Exit code 3: the run stopped to wait for something and was checkpointed.
/// It is not a failure, and a caller has to be able to tell the difference.
pub const EXIT_SUSPENDED: u8 = 3;

/// Reads a source file.
///
/// Input that is not UTF-8, or that starts with a BOM, is rejected. Checking it
/// here, in one place, is what lets every later layer assume its input is valid
/// UTF-8.
pub fn read_source(path: &str) -> Result<SourceFile, String> {
    let bytes = read_bytes(path)?;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(format!(
            "`{path}` starts with a BOM; save it as UTF-8 without one"
        ));
    }
    let text = String::from_utf8(bytes).map_err(|e| {
        let at = e.utf8_error().valid_up_to();
        format!("`{path}` is not valid UTF-8 (at byte {at})")
    })?;
    Ok(SourceFile::new(path, text))
}

pub fn read_bytes(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read `{path}`: {e}"))
}

/// Prints diagnostics to stderr and returns exit code 1 if any were errors.
pub fn report(sources: &SourceMap, diags: &[Diagnostic]) -> ExitCode {
    for d in diags {
        eprint!("{}", d.render(sources));
        eprintln!();
    }
    let errors = diags.iter().filter(|d| d.is_error()).count();
    if errors > 0 {
        let plural = if errors == 1 { "error" } else { "errors" };
        eprintln!("aborting due to {errors} {plural}");
        ExitCode::from(EXIT_FAILURE)
    } else {
        ExitCode::SUCCESS
    }
}

/// Runs the whole front end: parse, type check, lower, and emit bytecode.
///
/// Errors are already reported when this returns `Err`, which carries the exit
/// code to use.
pub fn compile_source(path: &str) -> Result<Program, ExitCode> {
    let loaded = load_program(path)?;
    let mut diags = loaded.diags;
    let sources = loaded.sources;

    let (typed, type_diags) = sic_types::check(&loaded.module);
    diags.extend(type_diags);
    if diags.iter().any(|d| d.is_error()) {
        return Err(report(&sources, &diags));
    }
    // Warnings still deserve to be seen.
    if !diags.is_empty() {
        report(&sources, &diags);
    }

    let hir = sic_ir::lower(&loaded.module, &typed);
    let program = sic_compile::compile(&hir, &sources).map_err(|errors| {
        // Reaching this means the compiler produced something it cannot encode,
        // such as a function needing more registers than the format allows.
        for e in errors {
            eprintln!("error: {e}");
        }
        ExitCode::from(EXIT_FAILURE)
    })?;
    Ok(program)
}

/// Reads a file and everything it imports.
pub fn load_program(path: &str) -> Result<Loaded, ExitCode> {
    crate::module::load(path, &read_source).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(EXIT_USAGE)
    })
}

/// Where bytecode about to be run came from.
///
/// It decides what a verification failure means, and nothing else. Bytecode
/// this process compiled failing to verify is a bug in the compiler; a file
/// failing to verify is a file that must not run, and might have been written
/// by anybody.
pub enum From<'a> {
    Compiler(&'a str),
    File(&'a str),
}

/// The one door into the VM.
///
/// Every path that builds or restores a VM goes through this, including the one
/// that just compiled the program itself. "The VM only ever runs verified
/// bytecode" is worth something exactly as long as it has no exceptions, and it
/// had three - `resume`, `attach` and `replay` each reached the VM through
/// `decode` alone, which establishes that a `Program` can exist and says
/// nothing about jump targets, register initialization, operand types, or
/// whether a `CALL_CAP` names a capability the manifest declares.
///
/// Being one function is the point. The invariant was not lost by anybody
/// deciding to skip the check; it was lost by three call sites being written
/// without it, which is what happens when the check is something a caller has
/// to remember.
pub fn verified(program: &Program, from: From<'_>) -> Result<(), ExitCode> {
    let report = sic_verify::verify(program);
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if report.ok() {
        return Ok(());
    }
    match from {
        From::Compiler(path) => {
            eprintln!("internal error: the bytecode compiled from `{path}` did not verify")
        }
        From::File(path) => eprintln!("error: `{path}` does not verify, so it will not be run"),
    }
    for error in &report.errors {
        eprintln!("  {error}");
    }
    Err(ExitCode::from(EXIT_FAILURE))
}

/// Loads and decodes a bytecode file.
pub fn load_bytecode(path: &str) -> Result<Program, ExitCode> {
    let bytes = read_bytes(path).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(EXIT_USAGE)
    })?;
    sic_bytecode::decode(&bytes).map_err(|e| {
        eprintln!("error: `{path}` is not usable bytecode: {e}");
        ExitCode::from(EXIT_FAILURE)
    })
}
