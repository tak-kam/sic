//! `sic parse <FILE>`: print the AST of a source file.

use std::process::ExitCode;

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

    let (module, diags) = sic_syntax::parse(file.text());
    let status = report(&file, &diags);

    // Print whatever the parser recovered even when there were errors; a partial
    // AST is still useful.
    print!("{}", dump(&module));
    status
}
