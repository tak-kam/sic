//! `sic run <FILE.sic>`: compile, verify, then run.
//!
//! Verification is not optional here. Running bytecode this process just
//! produced may look redundant, but treating "the VM only ever runs verified
//! bytecode" as an invariant with no exceptions is what makes it worth
//! anything.

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
// Only the two-process path refuses anything at the command line.
#[cfg(unix)]
use super::EXIT_USAGE;

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
    /// Run the interpreter in a process of its own - see
    /// `docs/design/processes.md`. Unix only, and a flag rather than the
    /// default while the second shape is new.
    ///
    /// Accepted everywhere and acted on where there is a socket: a person who
    /// types it on Windows is told by `sic run` that it did nothing, which is
    /// better than a command line that fails to parse for a reason about this
    /// machine.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub isolate: bool,
}

pub fn run(path: &str, options: RunOptions<'_>) -> ExitCode {
    let program = match compile_source(path) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if let Err(code) = super::verified(&program, super::From::Compiler(path)) {
        return code;
    }

    let Some(entry) = program.func_by_name("main") else {
        eprintln!("error: `{path}` has no `main` function");
        return ExitCode::from(EXIT_FAILURE);
    };
    if !program.funcs[entry as usize].params.is_empty() {
        eprintln!("error: `main` must take no arguments");
        return ExitCode::from(EXIT_FAILURE);
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
                    return ExitCode::from(EXIT_FAILURE);
                }
                eprintln!("run {run_id}  recorded in {}", dir.display());
                Some(dir)
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::from(EXIT_FAILURE);
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
                return ExitCode::from(EXIT_FAILURE);
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
                return ExitCode::from(EXIT_FAILURE);
            }
        };

    #[cfg(not(unix))]
    if options.isolate {
        eprintln!(
            "warning: `--isolate` needs a unix socket, and this build has none; running here"
        );
    }
    #[cfg(unix)]
    if options.isolate {
        return isolated(
            &program,
            entry,
            run_id,
            sink.as_mut(),
            &mut broker,
            recording.as_deref(),
        );
    }

    let journal = Journal::new(run_id, sink);
    let mut vm = Vm::with_journal(&program, DEFAULT_FUEL, journal);
    let status = vm.run(entry, &[]);
    let outcome = drive_recording(&mut vm, &mut broker, status, recording.as_deref());
    // A run that stopped to be continued keeps whatever conversation it was
    // holding; one that is over keeps nothing.
    broker.finish(matches!(outcome, Outcome::Suspended { .. }));

    // A recorded run that has to wait keeps its checkpoint too, so `sic resume`
    // can find it beside everything else.
    let checkpoint = match (&recording, options.checkpoint) {
        (Some(dir), None) => Some(dir.join(store::CHECKPOINT).to_string_lossy().into_owned()),
        (_, given) => given.map(str::to_string),
    };
    // A recorded run is identified by its id, so that is what the hint uses:
    // nothing about a path has to be remembered.
    let hint = recording
        .as_ref()
        .map(|_| format!("sic attach {} --value <VALUE>", &run_id.to_string()[..8]));
    finish(
        &mut vm,
        &program,
        outcome,
        checkpoint.as_deref(),
        hint.as_deref(),
    )
}

/// The same run, with the interpreter in a process of its own.
///
/// A separate function rather than a branch inside the loop, because it is a
/// different loop: there is no `Vm` on this side to ask anything of, and the
/// events arrive rather than being emitted. `docs/design/processes.md` §4 says
/// which commands take this shape and which do not.
#[cfg(unix)]
fn isolated(
    program: &Program,
    entry: u32,
    run_id: sic_journal::RunId,
    sink: &mut dyn sic_journal::Sink,
    broker: &mut sic_broker::Broker,
    recording: Option<&std::path::Path>,
) -> ExitCode {
    // A checkpoint does not cross the wire yet, so a program that could stop
    // and wait is refused before it starts rather than after it has done half
    // its work.
    if let Some(cap) = super::isolate::could_suspend(program) {
        eprintln!(
            "error: `--isolate` cannot save a run that stops to wait, and this program grants `{cap}`"
        );
        eprintln!("       run it without `--isolate`, which writes a checkpoint");
        return ExitCode::from(EXIT_USAGE);
    }
    match super::isolate::drive(
        program,
        entry,
        run_id,
        DEFAULT_FUEL,
        broker,
        sink,
        recording,
    ) {
        Ok(ended) => super::isolate::finish(ended),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Reports how a run ended, writing a checkpoint if it stopped to wait.
pub fn finish(
    vm: &mut Vm,
    program: &Program,
    outcome: Outcome,
    checkpoint_path: Option<&str>,
    resume_hint: Option<&str>,
) -> ExitCode {
    match outcome {
        Outcome::Finished(Value::Unit) => ExitCode::SUCCESS,
        Outcome::Finished(value) => {
            println!("{}", vm.display(&value));
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
            write_checkpoint(vm, program, path, &question, resume_hint)
        }
    }
}

fn write_checkpoint(
    vm: &mut Vm,
    program: &Program,
    path: &str,
    question: &str,
    resume_hint: Option<&str>,
) -> ExitCode {
    // The digest ties the checkpoint to this exact bytecode, so it cannot be
    // resumed against a program that has changed underneath it.
    let digest = Digest::of(&sic_bytecode::encode(program));
    let Some(bytes) = vm.checkpoint(digest, question) else {
        eprintln!("internal error: the run is waiting but has no state to save");
        return ExitCode::from(EXIT_FAILURE);
    };
    if let Err(e) = std::fs::write(path, &bytes) {
        eprintln!("error: cannot write `{path}`: {e}");
        return ExitCode::from(EXIT_FAILURE);
    }
    eprintln!("waiting: {question}");
    eprintln!("saved {} bytes to {path}", bytes.len());
    match resume_hint {
        Some(hint) => eprintln!("answer with:  {hint}"),
        None => eprintln!("resume with: sic resume {path} <FILE.sic> --value <VALUE>"),
    }
    ExitCode::from(EXIT_SUSPENDED)
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
