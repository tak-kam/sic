//! `sic hir <FILE>`: print the high-level IR.
//!
//! This is the layer where workflow semantics still exist, so it is worth being
//! able to look at directly.

use crate::out::say;

use std::process::ExitCode;

use super::{load_program, report};

pub fn run(path: &str) -> ExitCode {
    let loaded = match load_program(path) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let mut diags = loaded.diags;
    let (typed, type_diags) = sic_types::check(&loaded.module);
    diags.extend(type_diags);
    if diags.iter().any(|d| d.is_error()) {
        return report(&loaded.sources, &diags);
    }
    report(&loaded.sources, &diags);

    say!("{}", sic_ir::dump(&sic_ir::lower(&loaded.module, &typed)));
    ExitCode::SUCCESS
}
