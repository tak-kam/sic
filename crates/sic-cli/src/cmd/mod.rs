pub mod compile;
pub mod disasm;
pub mod drive;
pub mod hir;
pub mod journal;
pub mod parse;
pub mod resume;
pub mod run;
pub mod verify;

use std::process::ExitCode;

use sic_bytecode::Program;
use sic_core::{Diagnostic, SourceFile};

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
pub fn report(file: &SourceFile, diags: &[Diagnostic]) -> ExitCode {
    for d in diags {
        eprint!("{}", d.render(file));
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
pub fn compile_source(path: &str) -> Result<(SourceFile, Program), ExitCode> {
    let file = read_source(path).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(EXIT_USAGE)
    })?;

    let (module, mut diags) = sic_syntax::parse(file.text());
    let (typed, type_diags) = sic_types::check(&module);
    diags.extend(type_diags);
    if diags.iter().any(|d| d.is_error()) {
        return Err(report(&file, &diags));
    }
    // Warnings still deserve to be seen.
    if !diags.is_empty() {
        report(&file, &diags);
    }

    let hir = sic_ir::lower(&module, &typed);
    let program = sic_compile::compile(&hir, &file).map_err(|errors| {
        // Reaching this means the compiler produced something it cannot encode,
        // such as a function needing more registers than the format allows.
        for e in errors {
            eprintln!("error: {e}");
        }
        ExitCode::from(EXIT_FAILURE)
    })?;
    Ok((file, program))
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
