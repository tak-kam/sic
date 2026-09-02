//! `sic run <FILE.sic>`: compile, verify, then run.
//!
//! Verification is not optional here. Running bytecode this process just
//! produced may look redundant, but treating "the VM only ever runs verified
//! bytecode" as an invariant with no exceptions is what makes it worth
//! anything.

use crate::out::sayln;

use std::process::ExitCode;

use sic_broker::Broker;
use sic_bytecode::Program;
use sic_core::Digest;
use sic_journal::Journal;
use sic_vm::{DEFAULT_FUEL, FailInfo, Value, Vm};

use super::drive::{Outcome, drive_recording, manifest};
use super::journal::{FileSink, new_run_id};
use super::store;
use super::{EXIT_FAILURE, EXIT_SUSPENDED, compile_source};

pub struct RunOptions<'a> {
    pub journal: Option<&'a str>,
    /// Where to write the run's state if it has to stop and wait.
    pub checkpoint: Option<&'a str>,
    /// Keep the whole run - bytecode, journal, and what the broker answered -
    /// so it can be explained and replayed later.
    pub record: bool,
    /// What is to answer `llm.invoke`, as `<multiplexer>:<agent>`. Without one
    /// a model call defers, which is what it has always done.
    pub llm: Option<&'a str>,
    /// Whether the interpreter runs in a process of its own - see
    /// `docs/design/processes.md`. On unix it does unless `--no-isolate`
    /// says otherwise; there is no socket anywhere else.
    pub isolation: super::Isolation,
    /// Ask the terminal when the run stops for an answer, instead of leaving
    /// it waiting - see `docs/design/interactive.md`.
    pub interactive: bool,
}

/// Runs the program, and keeps asking while somebody is there to be asked.
///
/// The loop is `attach`'s, because `attach` is already the operation "answer
/// one question and continue" and nothing says it can only be reached from a
/// second command line. By the time it is called the run has suspended, which
/// is what wrote the checkpoint - so the question on the screen is one the run
/// has already survived. `docs/design/interactive.md` §2.
pub fn run(path: &str, options: RunOptions<'_>) -> ExitCode {
    let interactive = options.interactive;
    let llm = options.llm;
    let isolation = options.isolation;
    let (code, waiting) = start(path, options);
    match waiting {
        Some(run) if interactive => {
            super::runs::attach(&run[..8], None, None, llm, isolation, true)
        }
        _ => code,
    }
}

/// The run itself, and the id of a recorded run that stopped to wait.
fn start(path: &str, options: RunOptions<'_>) -> (ExitCode, Option<String>) {
    let program = match compile_source(path) {
        Ok(v) => v,
        Err(code) => return (code, None),
    };

    if let Err(code) = super::verified(&program, super::From::Compiler(path)) {
        return (code, None);
    }

    let Some(entry) = program.func_by_name("main") else {
        eprintln!("error: `{path}` has no `main` function");
        return (ExitCode::from(EXIT_FAILURE), None);
    };
    if !program.funcs[entry as usize].params.is_empty() {
        eprintln!("error: `main` must take no arguments");
        return (ExitCode::from(EXIT_FAILURE), None);
    }

    let run_id = new_run_id();

    // A recorded run keeps the bytecode beside its journal: replaying needs the
    // exact program, and "the file on disk now" is not it.
    let recording = if options.record {
        match store::create(run_id) {
            Ok(dir) => {
                let bytes = sic_bytecode::encode(&program);
                if let Err(e) = std::fs::write(dir.join(store::PROGRAM), &bytes) {
                    eprintln!("error: cannot write the recorded program: {e}");
                    return (ExitCode::from(EXIT_FAILURE), None);
                }
                eprintln!("run {run_id}  recorded in {}", dir.display());
                Some(dir)
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                return (ExitCode::from(EXIT_FAILURE), None);
            }
        }
    } else {
        None
    };

    let journal_path = match (&recording, options.journal) {
        (Some(dir), None) => Some(dir.join(store::JOURNAL).to_string_lossy().into_owned()),
        (_, given) => given.map(str::to_string),
    };
    // Always wrapped, including around the sink that writes nothing: `log` is
    // what a program says while it works, and a person watching should not
    // have to have asked for a journal to see it.
    let sink: Box<dyn sic_journal::Sink> = match journal_path.as_deref() {
        Some(path) => match FileSink::create(path) {
            Ok(sink) => {
                if recording.is_none() {
                    eprintln!("run {run_id} -> {path}");
                }
                Box::new(sink)
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                return (ExitCode::from(EXIT_FAILURE), None);
            }
        },
        None => Box::new(sic_journal::NullSink),
    };
    // The sink is the CLI's, on either shape. In one process a `Journal` wraps
    // it and the VM emits through it; in two the child emits and the events
    // arrive here, so the sink is handed over on its own.
    // `mut` only on the side that hands it over rather than wrapping it.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut sink: Box<dyn sic_journal::Sink> =
        Box::new(super::journal::LogSink::around(sink, recording.as_deref()));

    // Opened before the run starts, so a run that is going to fail for want of
    // a tool fails before it has done anything.
    let session = super::driver::Session {
        run: run_id.to_string(),
        continuing: false,
        state: recording.as_ref().map(|dir| dir.join(store::CONVERSATIONS)),
    };
    let manifest = manifest(&program);
    let mut broker =
        match super::driver::open(options.llm, session, &manifest, recording.as_deref()) {
            Ok(Some(driver)) => Broker::with_driver(manifest, driver),
            Ok(None) => Broker::new(manifest),
            Err(message) => {
                eprintln!("error: {message}");
                return (ExitCode::from(EXIT_FAILURE), None);
            }
        };

    // Where a run that stops to wait puts its state. Computed before the run
    // rather than after it, because both shapes need the same answer and only
    // one of them still has a `Vm` to ask afterwards.
    let checkpoint = match (&recording, options.checkpoint) {
        (Some(dir), None) => Some(dir.join(store::CHECKPOINT).to_string_lossy().into_owned()),
        (_, given) => given.map(str::to_string),
    };
    // A recorded run is identified by its id, so that is what the hint uses:
    // nothing about a path has to be remembered.
    let hint = recording
        .as_ref()
        .map(|_| format!("sic attach {} --value <VALUE>", &run_id.to_string()[..8]));
    let stopping = if options.interactive {
        Stopping::Asking
    } else {
        Stopping::Waiting(hint.as_deref())
    };

    #[cfg(not(unix))]
    super::no_socket_here(options.isolation);
    #[cfg(unix)]
    if options.isolation.separate() {
        let (code, waiting) = isolated(
            &program,
            entry,
            run_id,
            sink.as_mut(),
            &mut broker,
            Keeping {
                recording: recording.as_deref(),
                checkpoint: checkpoint.as_deref(),
                stopping,
            },
        );
        return (code, kept(&recording, run_id, waiting));
    }

    let journal = Journal::new(run_id, sink);
    let mut vm = Vm::with_journal(&program, DEFAULT_FUEL, journal);
    let status = vm.run(entry, &[]);
    let outcome = drive_recording(&mut vm, &mut broker, status, recording.as_deref());
    // A run that stopped to be continued keeps whatever conversation it was
    // holding; one that is over keeps nothing.
    let waiting = matches!(outcome, Outcome::Suspended { .. });
    broker.finish(waiting);

    // A recorded run that has to wait keeps its checkpoint too, so `sic resume`
    // can find it beside everything else.
    let code = finish(&mut vm, &program, outcome, checkpoint.as_deref(), stopping);
    (code, kept(&recording, run_id, waiting))
}

/// The id of a run that stopped to wait and is being kept, which is the only
/// case `attach` can pick up again.
///
/// A run that waits without `--record` has a checkpoint and no store, and
/// `docs/design/interactive.md` §4 says why that is not the interactive path:
/// there would be nowhere to record the reason for the answer it is about to
/// ask for.
fn kept(
    recording: &Option<std::path::PathBuf>,
    run: sic_journal::RunId,
    waiting: bool,
) -> Option<String> {
    (waiting && recording.is_some()).then(|| run.to_string())
}

/// The same run, with the interpreter in a process of its own.
///
/// A separate function rather than a branch inside the loop, because it is a
/// different loop: there is no `Vm` on this side to ask anything of, and the
/// events arrive rather than being emitted. `docs/design/processes.md` §4 says
/// which commands take this shape and which do not.
/// What a run keeps, and how it is picked up again if it stops to wait.
///
/// Three facts that are one idea: whether the whole run is being recorded,
/// where its state goes if it waits, and what to tell a person to type. They
/// travel together because they are decided together, above.
#[cfg(unix)]
pub struct Keeping<'a> {
    pub recording: Option<&'a std::path::Path>,
    pub checkpoint: Option<&'a str>,
    pub stopping: Stopping<'a>,
}

#[cfg(unix)]
fn isolated(
    program: &Program,
    entry: u32,
    run_id: sic_journal::RunId,
    sink: &mut dyn sic_journal::Sink,
    broker: &mut sic_broker::Broker,
    keeping: Keeping<'_>,
) -> (ExitCode, bool) {
    match super::isolate::drive(
        program,
        super::isolate::Begin::Fresh {
            entry,
            run: run_id,
            fuel: DEFAULT_FUEL,
        },
        broker,
        sink,
        keeping.recording,
    ) {
        Ok(ran) => {
            // A run that is still waiting keeps whatever the driver is
            // holding; one that is over keeps nothing. Same as the shape
            // above, and for the same reason.
            let waiting = matches!(ran.ended, crate::wire::Ended::Suspended(_));
            broker.finish(waiting);
            (
                super::isolate::finish(ran, keeping.checkpoint, keeping.stopping),
                waiting,
            )
        }
        Err(message) => {
            eprintln!("error: {message}");
            (ExitCode::from(EXIT_FAILURE), false)
        }
    }
}

/// How a run that stopped to wait is reported.
///
/// Both shapes wrote this out for themselves until interactive mode gave them
/// a third thing to agree about, which is what makes it one type rather than a
/// flag each.
#[derive(Clone, Copy)]
pub enum Stopping<'a> {
    /// Nobody is here. Say what the run is waiting for, and what to type.
    Waiting(Option<&'a str>),
    /// The terminal is about to be asked. The question is the prompt's job,
    /// and a command line nobody is going to type reads as something they were
    /// supposed to do. `docs/design/interactive.md` §7.
    Asking,
}

/// Writes a run's state out, and says what it is waiting for.
pub fn saved(bytes: &[u8], path: &str, question: &str, how: Stopping<'_>) -> ExitCode {
    if let Err(e) = std::fs::write(path, bytes) {
        eprintln!("error: cannot write `{path}`: {e}");
        return ExitCode::from(EXIT_FAILURE);
    }
    // Said either way: that the run is on disk before anybody is asked
    // anything is the whole of why asking is free.
    eprintln!("saved {} bytes to {path}", bytes.len());
    if let Stopping::Waiting(hint) = how {
        eprintln!("waiting: {question}");
        match hint {
            Some(hint) => eprintln!("answer with:  {hint}"),
            None => eprintln!("resume with: sic resume {path} <FILE.sic> --value <VALUE>"),
        }
    }
    ExitCode::from(EXIT_SUSPENDED)
}

/// Reports how a run ended, writing a checkpoint if it stopped to wait.
pub fn finish(
    vm: &mut Vm,
    program: &Program,
    outcome: Outcome,
    checkpoint_path: Option<&str>,
    how: Stopping<'_>,
) -> ExitCode {
    match outcome {
        Outcome::Finished(Value::Unit) => ExitCode::SUCCESS,
        Outcome::Finished(value) => {
            sayln!("{}", vm.display(&value));
            ExitCode::SUCCESS
        }
        Outcome::Failed(info) => {
            report_failure(vm, program, &info);
            ExitCode::from(EXIT_FAILURE)
        }
        Outcome::Suspended { question } => {
            let Some(path) = checkpoint_path else {
                // Without somewhere to put the state, the only alternative
                // would be to lose the run.
                eprintln!("error: the run is waiting for `{question}` and has nowhere to be saved");
                eprintln!("       pass --checkpoint PATH to write its state out");
                return ExitCode::from(EXIT_FAILURE);
            };
            write_checkpoint(vm, program, path, &question, how)
        }
    }
}

fn write_checkpoint(
    vm: &mut Vm,
    program: &Program,
    path: &str,
    question: &str,
    how: Stopping<'_>,
) -> ExitCode {
    // The digest ties the checkpoint to this exact bytecode, so it cannot be
    // resumed against a program that has changed underneath it.
    let digest = Digest::of(&sic_bytecode::encode(program));
    let Some(bytes) = vm.checkpoint(digest, question) else {
        eprintln!("internal error: the run is waiting but has no state to save");
        return ExitCode::from(EXIT_FAILURE);
    };
    saved(&bytes, path, question, how)
}

/// Reports a runtime failure, naming the source line through the debug section.
///
/// The file comes from the debug section rather than from whatever was named on
/// the command line, so a failure inside an imported file says so.
pub fn report_failure(vm: &Vm, program: &Program, info: &FailInfo) {
    eprint!("error: {}", info.kind.message());
    if let Some(value) = &info.value {
        eprint!(": {}", vm.display(value));
    }
    if let Some(detail) = &info.detail {
        eprint!(": {detail}");
    }
    eprintln!();

    match program.debug.position(info.pc) {
        Some((line, col)) => {
            let name = program.debug.file(info.pc).unwrap_or("?");
            eprintln!(" --> {name}:{line}:{col}")
        }
        None => eprintln!(" --> in `{}` at instruction {}", info.func, info.pc),
    }
}
