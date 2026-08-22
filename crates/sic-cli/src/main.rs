//! The `sic` command.
//!
//! Argument parsing is hand-written. That follows the zero-dependency rule, but
//! the real benefit is that everything the CLI accepts can be read in this one
//! file.

mod cmd;

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
sic - a language for AI agents and workflows

Usage:
  sic parse <FILE>     parse a source file and print its AST
  sic help             show this help
  sic version          show the version

Exit codes:
  0  success
  1  the source has errors
  2  the command line is wrong
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };

    match command.as_str() {
        "parse" => match args.get(1) {
            Some(path) if args.len() == 2 => cmd::parse::run(path),
            Some(_) => usage_error("`parse` takes exactly one file"),
            None => usage_error("`parse` needs a file"),
        },
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        "version" | "--version" | "-V" => {
            println!("sic {VERSION}");
            ExitCode::SUCCESS
        }
        other => usage_error(format!("unknown command `{other}`")),
    }
}

fn usage_error(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {msg}\n");
    eprint!("{USAGE}");
    ExitCode::from(2)
}
