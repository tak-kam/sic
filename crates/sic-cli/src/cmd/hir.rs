//! `sic hir <FILE>`: print the high-level IR.
//!
//! This is the layer where workflow semantics still exist, so it is worth being
//! able to look at directly.

use std::process::ExitCode;

use super::{EXIT_USAGE, read_source, report};

pub fn run(path: &str) -> ExitCode {
    let file = match read_source(path) {
        Ok(f) => f,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let (module, mut diags) = sic_syntax::parse(file.text());
    let (typed, type_diags) = sic_types::check(&module);
    diags.extend(type_diags);
    if diags.iter().any(|d| d.is_error()) {
        return report(&file, &diags);
    }
    report(&file, &diags);

    print!("{}", sic_ir::dump(&sic_ir::lower(&module, &typed)));
    ExitCode::SUCCESS
}
