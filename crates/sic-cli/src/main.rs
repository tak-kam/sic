//! The `sic` command.
//!
//! Argument parsing is hand-written. That follows the zero-dependency rule, but
//! the real benefit is that everything the CLI accepts can be read in this one
//! file.

mod cmd;
mod module;

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
sic - a language for AI agents and workflows

Usage:
  sic run <FILE.sic> [--journal PATH] [--checkpoint PATH] [--record]
                                  compile, verify and run a source file,
                                  optionally recording its execution journal,
                                  saving its state if it has to wait, or
                                  keeping the whole run with --record
  sic runs [--waiting]            list recorded runs, or only those waiting
  sic attach <RUN-ID> [--value V] see what a waiting run needs, or answer it
  sic explain <RUN-ID>            summarize a recorded run
  sic inspect-run <RUN-ID>        print every event of a recorded run
  sic replay <RUN-ID>             re-run it against its recorded answers
  sic resume <CHECKPOINT> <FILE.sic> --value <VALUE> [--journal PATH] [--checkpoint PATH]
                                  continue a run that stopped to wait
  sic compile <FILE.sic> [-o OUT] write bytecode to OUT (default: FILE.sicb)
  sic export <JOURNAL> [--traces PATH] [--metrics PATH]
                                  convert an execution journal to OpenTelemetry
  sic plan <FILE.sic|FILE.sicb>   show what a program may do, without running it
  sic verify <FILE.sicb>          check that bytecode is safe to run
  sic disasm <FILE.sicb>          print bytecode as instructions
  sic parse <FILE.sic>            print the AST
  sic hir <FILE.sic>              print the high-level IR
  sic update [--to FILE --sha256 HEX] [--check]
                                  replace this binary with one already on disk,
                                  after checking it against a digest; --check
                                  says what would happen and changes nothing
  sic help                        show this help
  sic version                     show the version

Exit codes:
  0  success
  1  the program has errors, or a run failed
  2  the command line is wrong
  3  the run is waiting for something and was checkpointed
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];

    match command.as_str() {
        "run" => match parse_run(rest) {
            Ok((file, journal, checkpoint, record)) => cmd::run::run(
                &file,
                cmd::run::RunOptions {
                    journal: journal.as_deref(),
                    checkpoint: checkpoint.as_deref(),
                    record,
                },
            ),
            Err(msg) => usage_error(msg),
        },
        "runs" => match rest {
            [] => cmd::runs::list(),
            [flag] if flag == "--waiting" => cmd::runs::list_waiting(),
            _ => usage_error("`runs` takes at most `--waiting`"),
        },
        "attach" => match parse_flags(rest, &["--value"], 1) {
            Ok((files, flags)) => cmd::runs::attach(&files[0], flags[0].as_deref()),
            Err(msg) => usage_error(msg),
        },
        "explain" => with_one_file(rest, "explain", cmd::runs::explain),
        "inspect-run" => with_one_file(rest, "inspect-run", cmd::runs::inspect),
        "replay" => with_one_file(rest, "replay", cmd::runs::replay),
        "resume" => match parse_flags(rest, &["--value", "--journal", "--checkpoint"], 2) {
            Ok((files, flags)) => cmd::resume::run(
                &files[0],
                &files[1],
                cmd::resume::ResumeOptions {
                    value: flags[0].as_deref(),
                    journal: flags[1].as_deref(),
                    checkpoint: flags[2].as_deref(),
                },
            ),
            Err(msg) => usage_error(msg),
        },
        "parse" => with_one_file(rest, "parse", cmd::parse::run),
        "hir" => with_one_file(rest, "hir", cmd::hir::run),
        "export" => match parse_flags(rest, &["--traces", "--metrics"], 1) {
            Ok((files, flags)) => cmd::export::run(
                &files[0],
                cmd::export::ExportOptions {
                    traces: flags[0].as_deref(),
                    metrics: flags[1].as_deref(),
                },
            ),
            Err(msg) => usage_error(msg),
        },
        "plan" => with_one_file(rest, "plan", cmd::plan::run),
        "verify" => with_one_file(rest, "verify", cmd::verify::run),
        "disasm" => with_one_file(rest, "disasm", cmd::disasm::run),
        "update" => match parse_update(rest) {
            Ok((to, sha256, check)) => cmd::update::run(cmd::update::UpdateOptions {
                to: to.as_deref(),
                sha256: sha256.as_deref(),
                check,
            }),
            Err(msg) => usage_error(msg),
        },
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

/// `run <input> [--journal PATH] [--checkpoint PATH] [--record]`.
///
/// `--record` takes no value, so it cannot go through `parse_flags`.
fn parse_run(args: &[String]) -> Result<(String, Option<String>, Option<String>, bool), String> {
    let mut record = false;
    let rest: Vec<String> = args
        .iter()
        .filter(|a| {
            if a.as_str() == "--record" {
                record = true;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    let (files, flags) = parse_flags(&rest, &["--journal", "--checkpoint"], 1)?;
    Ok((files[0].clone(), flags[0].clone(), flags[1].clone(), record))
}

/// `update [--to PATH] [--sha256 HEX] [--check]`.
///
/// `--check` takes no value, so it cannot go through `parse_flags` either.
fn parse_update(args: &[String]) -> Result<(Option<String>, Option<String>, bool), String> {
    let mut check = false;
    let rest: Vec<String> = args
        .iter()
        .filter(|a| {
            if a.as_str() == "--check" {
                check = true;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    let (_, flags) = parse_flags(&rest, &["--to", "--sha256"], 0)?;
    Ok((flags[0].clone(), flags[1].clone(), check))
}

/// Splits arguments into the expected positional files and the values of the
/// named options, each of which takes one argument.
fn parse_flags(
    args: &[String],
    names: &[&str],
    expected_files: usize,
) -> Result<(Vec<String>, Vec<Option<String>>), String> {
    let mut files: Vec<String> = Vec::new();
    let mut values: Vec<Option<String>> = vec![None; names.len()];
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match names.iter().position(|n| *n == arg) {
            Some(index) => {
                i += 1;
                let value = args.get(i).ok_or(format!("`{arg}` needs a value"))?;
                if values[index].replace(value.clone()).is_some() {
                    return Err(format!("`{arg}` was given twice"));
                }
            }
            None if arg.starts_with('-') => return Err(format!("unknown option `{arg}`")),
            None => files.push(arg.to_string()),
        }
        i += 1;
    }
    if files.len() != expected_files {
        return Err(format!(
            "expected {expected_files} file argument(s), got {}",
            files.len()
        ));
    }
    Ok((files, values))
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
