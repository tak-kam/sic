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
  sic run <FILE.sic>              compile, verify and run a source file
  sic compile <FILE.sic> [-o OUT] write bytecode to OUT (default: FILE.sicb)
  sic verify <FILE.sicb>          check that bytecode is safe to run
  sic disasm <FILE.sicb>          print bytecode as instructions
  sic parse <FILE.sic>            print the AST
  sic hir <FILE.sic>              print the high-level IR
  sic help                        show this help
  sic version                     show the version

Exit codes:
  0  success
  1  the program has errors, or a run failed
  2  the command line is wrong
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];

    match command.as_str() {
        "run" => with_one_file(rest, "run", cmd::run::run),
        "parse" => with_one_file(rest, "parse", cmd::parse::run),
        "hir" => with_one_file(rest, "hir", cmd::hir::run),
        "verify" => with_one_file(rest, "verify", cmd::verify::run),
        "disasm" => with_one_file(rest, "disasm", cmd::disasm::run),
        "compile" => match parse_compile_args(rest) {
            Ok((input, output)) => cmd::compile::run(&input, output.as_deref()),
            Err(msg) => usage_error(msg),
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

fn with_one_file(args: &[String], name: &str, f: fn(&str) -> ExitCode) -> ExitCode {
    match args {
        [path] => f(path),
        [] => usage_error(format!("`{name}` needs a file")),
        _ => usage_error(format!("`{name}` takes exactly one file")),
    }
}

/// `compile <input> [-o <output>]`.
fn parse_compile_args(args: &[String]) -> Result<(String, Option<String>), String> {
    let mut input = None;
    let mut output = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                let value = args.get(i).ok_or("`-o` needs a path")?;
                if output.replace(value.clone()).is_some() {
                    return Err("`-o` was given twice".into());
                }
            }
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            other => {
                if input.replace(other.to_string()).is_some() {
                    return Err("`compile` takes exactly one input file".into());
                }
            }
        }
        i += 1;
    }
    Ok((input.ok_or("`compile` needs a file")?, output))
}

fn usage_error(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {msg}\n");
    eprint!("{USAGE}");
    ExitCode::from(2)
}
