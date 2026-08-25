//! The `sic` command.
//!
//! Argument parsing is hand-written. That follows the zero-dependency rule, but
//! the real benefit is that everything the CLI accepts can be read in this one
//! file.

mod cmd;
mod module;
mod path;
#[cfg(unix)]
mod wire;

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
sic - a language for AI agents and workflows

Usage:
  sic run <FILE.sic> [--journal PATH] [--checkpoint PATH] [--record] [--llm SPEC] [--isolate]
                                  compile, verify and run a source file,
                                  optionally recording its execution journal,
                                  saving its state if it has to wait, or
                                  keeping the whole run with --record;
                                  --llm <multiplexer>:<agent>, as in tmux:claude,
                                  answers llm.invoke by driving that agent in a
                                  pane instead of stopping to ask a person;
                                  --isolate puts the interpreter in a process of
                                  its own, which opens no file and starts no
                                  program (unix only; `resume` and `attach` take
                                  it too, and a checkpoint does not care which
                                  shape wrote it)
  sic runs [--waiting]            list recorded runs, or only those waiting
  sic attach <RUN-ID> [--value V] [--because WHY] [--llm SPEC] [--isolate]
                                  see what a waiting run needs, or answer it -
                                  `--because` records why, next to the answer,
                                  and `--llm` picks up the run's own agent panes
  sic explain <RUN-ID>            summarize a recorded run
  sic inspect-run <RUN-ID>        print every event of a recorded run
  sic replay <RUN-ID>             re-run it against its recorded answers
  sic recheck <RUN-ID> <FILE.sic>
                                  run FILE against those answers instead, to
                                  see whether an edit still asks what the
                                  recording answered
  sic resume <CHECKPOINT> <FILE.sic> --value <VALUE> [--journal PATH] [--checkpoint PATH] [--llm SPEC] [--isolate]
                                  continue a run that stopped to wait
  sic compile <FILE.sic> [-o OUT] write bytecode to OUT (default: FILE.sicb)
  sic export <JOURNAL> [--traces PATH] [--metrics PATH]
                                  convert an execution journal to OpenTelemetry
  sic plan <FILE.sic|FILE.sicb>   show what a program may do, without running it
  sic verify <FILE.sicb>          check that bytecode is safe to run
  sic disasm <FILE.sicb>          print bytecode as instructions
  sic parse <FILE.sic>            print the AST
  sic hir <FILE.sic>              print the high-level IR
  sic upgrade [--check]           fetch the latest release, check it against the
                                  digests that release publishes, and replace
                                  this binary with it
  sic upgrade --to FILE --sha256 HEX [--check]
                                  the same from a file already on disk, checked
                                  against a digest you bring; --check says what
                                  would happen and changes nothing
  sic mcp                         serve the capabilities a run granted, to the
                                  agent answering for it; started by a run, not
                                  by a person
  sic hook                        decide whether the agent may use one of its
                                  own tools; likewise
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
            Ok(asked) => cmd::run::run(
                &asked.file,
                cmd::run::RunOptions {
                    journal: asked.flags[0].as_deref(),
                    checkpoint: asked.flags[1].as_deref(),
                    record: asked.record,
                    llm: asked.flags[2].as_deref(),
                    isolate: asked.isolate,
                },
            ),
            Err(msg) => usage_error(msg),
        },
        "runs" => match rest {
            [] => cmd::runs::list(),
            [flag] if flag == "--waiting" => cmd::runs::list_waiting(),
            _ => usage_error("`runs` takes at most `--waiting`"),
        },
        "attach" => match parse_flags(
            &without_isolate(rest).0,
            &["--value", "--because", "--llm"],
            1,
        ) {
            Ok((files, flags)) => cmd::runs::attach(
                &files[0],
                flags[0].as_deref(),
                flags[1].as_deref(),
                flags[2].as_deref(),
                without_isolate(rest).1,
            ),
            Err(msg) => usage_error(msg),
        },
        "explain" => with_one_file(rest, "explain", cmd::runs::explain),
        "inspect-run" => with_one_file(rest, "inspect-run", cmd::runs::inspect),
        // A replay answers from what was recorded. A driver that could reach a
        // live agent would make it something else.
        "replay" if rest.iter().any(|a| a == "--llm") => usage_error(
            "`replay` re-runs a recorded run against its recorded answers, so it takes no driver",
        ),
        "replay" => with_one_file(rest, "replay", cmd::runs::replay),
        "recheck" if rest.iter().any(|a| a == "--llm") => usage_error(
            "`recheck` answers a program's calls from what was recorded, so it takes no driver: \
             a check that reached a live agent would be answering a different question",
        ),
        "recheck" => match rest {
            [run, source] => cmd::runs::recheck(run, source),
            _ => usage_error("`recheck` takes a run id and a source file"),
        },
        "resume" => match parse_flags(
            &without_isolate(rest).0,
            &["--value", "--journal", "--checkpoint", "--because", "--llm"],
            2,
        ) {
            // A reason needs somewhere to live, and a checkpoint is a run's
            // state rather than its record. Saying so beats accepting the flag
            // and dropping what it carried.
            Ok((_, flags)) if flags[3].is_some() => usage_error(
                "`resume` cannot record a reason: a checkpoint holds a run's state, not its \
                 record. `sic attach <RUN-ID> --value V --because \"...\"` writes one",
            ),
            Ok((files, flags)) => cmd::resume::run(
                &files[0],
                &files[1],
                cmd::resume::ResumeOptions {
                    value: flags[0].as_deref(),
                    journal: flags[1].as_deref(),
                    checkpoint: flags[2].as_deref(),
                    llm: flags[4].as_deref(),
                    isolate: without_isolate(rest).1,
                },
            ),
            Err(msg) => usage_error(msg),
        },
        // Started by a run, not by a person: the agent answering a model call
        // runs this to reach the capabilities the program granted.
        //
        // Both of these talk to a unix socket the run is listening on. Where
        // there is none they are refused with the reason rather than reported
        // as unknown, because whoever typed one read it in a document.
        #[cfg(unix)]
        "mcp" => match rest.is_empty() {
            true => cmd::mcp::run(),
            false => usage_error("`mcp` takes no arguments"),
        },
        // Also started by a run: the agent asks this before every tool call.
        #[cfg(unix)]
        "hook" => match rest.is_empty() {
            true => cmd::hook::run(),
            false => usage_error("`hook` takes no arguments"),
        },
        // The interpreter as a process of its own, talking over the socket the
        // run listens on: `docs/design/processes.md`.
        #[cfg(unix)]
        "vm" => match rest {
            [flag, path] if flag == "--socket" => cmd::vm::run(path),
            _ => usage_error("`vm` takes `--socket PATH`"),
        },
        #[cfg(not(unix))]
        "mcp" | "hook" | "vm" => {
            usage_error("this serves the unix socket a run listens on, and this build has none")
        }
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
        "upgrade" => match parse_upgrade(rest) {
            Ok((to, sha256, check)) => cmd::upgrade::run(cmd::upgrade::UpgradeOptions {
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

/// `run <input> [--journal PATH] [--checkpoint PATH] [--llm SPEC] [--record]`.
///
/// `--record` takes no value, so it cannot go through `parse_flags`.
/// What `sic run` was asked for.
///
/// A struct rather than a tuple of four, because the fourth was the one that
/// made a reader count positions.
struct Run {
    file: String,
    flags: Vec<Option<String>>,
    record: bool,
    isolate: bool,
}

/// Lifts `--isolate` out, and says whether it was there.
///
/// Three commands take it - `run`, `resume`, `attach` - and a flag with no
/// value does not fit `parse_flags`, which pairs each with one.
fn without_isolate(args: &[String]) -> (Vec<String>, bool) {
    let mut isolate = false;
    let rest = args
        .iter()
        .filter(|a| {
            let found = a.as_str() == "--isolate";
            isolate |= found;
            !found
        })
        .cloned()
        .collect();
    (rest, isolate)
}

fn parse_run(args: &[String]) -> Result<Run, String> {
    let (args, isolate) = without_isolate(args);
    let mut record = false;
    let rest: Vec<String> = args
        .iter()
        .filter(|a| {
            let found = a.as_str() == "--record";
            record |= found;
            !found
        })
        .cloned()
        .collect();
    let (files, flags) = parse_flags(&rest, &["--journal", "--checkpoint", "--llm"], 1)?;
    Ok(Run {
        file: files[0].clone(),
        flags,
        record,
        isolate,
    })
}

/// `upgrade [--to PATH] [--sha256 HEX] [--check]`.
///
/// `--check` takes no value, so it cannot go through `parse_flags` either.
fn parse_upgrade(args: &[String]) -> Result<(Option<String>, Option<String>, bool), String> {
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
