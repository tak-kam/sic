//! `sic parse <FILE>`: print the AST of a source file.

use crate::out::say;

use std::process::ExitCode;

use sic_core::SourceMap;
use sic_syntax::print::dump;

use super::{EXIT_USAGE, read_source, report};

pub fn run(path: &str) -> ExitCode {
    let file = match read_source(path) {
        Ok(f) => f,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    // One file, deliberately: `parse` shows what the parser made of the file it
    // was given, imports included, rather than the program they add up to.
    let (module, diags) = sic_syntax::parse(file.text());
    let status = report(&SourceMap::single(file), &diags);

    // Print whatever the parser recovered even when there were errors; a partial
    // AST is still useful.
    say!("{}", dump(&module));
    status
}
