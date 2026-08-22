pub mod parse;

use std::process::ExitCode;

use sic_core::SourceFile;

/// Reads a source file.
///
/// Input that is not UTF-8, or that starts with a BOM, is rejected. Checking it
/// here, in one place, is what lets every later layer assume its input is valid
/// UTF-8.
pub fn read_source(path: &str) -> Result<SourceFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
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

/// Prints diagnostics to stderr and returns exit code 1 if any were errors.
pub fn report(file: &SourceFile, diags: &[sic_core::Diagnostic]) -> ExitCode {
    for d in diags {
        eprint!("{}", d.render(file));
        eprintln!();
    }
    let errors = diags.iter().filter(|d| d.is_error()).count();
    if errors > 0 {
        let plural = if errors == 1 { "error" } else { "errors" };
        eprintln!("aborting due to {errors} {plural}");
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
