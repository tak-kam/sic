//! `sic run <FILE.sic>`: compile, verify, then run.
//!
//! Verification is not optional here. Running bytecode this process just
//! produced may look redundant, but treating "the VM only ever runs verified
//! bytecode" as an invariant with no exceptions is what makes it worth
//! anything.

use std::process::ExitCode;

use sic_broker::{AgentDriver, Broker};
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
    let journal = match journal_path.as_deref() {
        Some(path) => match FileSink::create(path) {
            Ok(sink) => {
                if recording.is_none() {
                    eprintln!("run {run_id} -> {path}");
                }
                Journal::new(run_id, Box::new(sink))
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::from(EXIT_FAILURE);
            }
        },
        None => Journal::new(run_id, Box::new(sic_journal::NullSink)),
    };

    // Opened before the run starts, so a run that is going to fail for want of
    // a tool fails before it has done anything.
    let session = sic_broker::tmux::Session {
        run: run_id.to_string(),
        continuing: false,
        state: recording.as_ref().map(|dir| dir.join(store::CONVERSATIONS)),
    };
    let mut broker = match open_driver(options.llm, session, recording.as_deref()) {
        Ok(Some(driver)) => Broker::with_driver(manifest(&program), driver),
        Ok(None) => Broker::new(manifest(&program)),
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

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

/// Opens the driver a run was told to use, and records what it turned out to
/// be.
///
/// Nothing is chosen here. A driver that started answering model calls because
/// it happened to be installed would make what a run did depend on what was
/// lying around, which is the same argument as refusing to search `PATH`.
pub fn open_driver(
    spec: Option<&str>,
    session: sic_broker::tmux::Session,
    recording: Option<&std::path::Path>,
) -> Result<Option<Box<dyn sic_broker::AgentDriver>>, String> {
    let Some(spec) = spec else {
        return Ok(None);
    };
    let driver = sic_broker::TmuxDriver::open(spec, session).map_err(|e| e.message)?;
    let info = driver.info().clone();
    if let Some(dir) = recording {
        store::record_driver(dir, &info)?;
    }
    eprintln!(
        "llm.invoke answered by {} - {}, {}",
        info.driver, info.agent, info.multiplexer
    );
    Ok(Some(Box::new(driver)))
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
