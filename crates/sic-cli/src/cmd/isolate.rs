//! Running a program with the interpreter in a process of its own.
//!
//! The parent: it compiles, verifies, listens on a socket, starts `sic vm`,
//! and answers. It keeps the terminal, the run store, the journal sink, the
//! manifest and the broker; the child keeps the bytecode and the arena.
//!
//! `docs/design/processes.md`. What this buys today is a resource bound - a run
//! that grows its arena takes the child rather than everything - and what it
//! makes possible later is a child with fewer privileges than the parent, which
//! is the one thing a crate boundary cannot do.

use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, ExitCode};

use sic_broker::Broker;
use sic_journal::Sink;

use crate::wire::{Ended, FromVm, ToVm, recv, send};

use super::{EXIT_FAILURE, EXIT_SUSPENDED};

/// The socket a run listens on, removed when the run ends.
///
/// One outliving its run would be a door into a run that is not there - the
/// same reason `route` removes its own.
struct Socket {
    path: std::path::PathBuf,
    listener: UnixListener,
}

impl Socket {
    fn open(run: &str) -> Result<Socket, String> {
        let path = std::env::temp_dir().join(format!("sic-vm-{}-{run}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("cannot listen at `{}`: {e}", path.display()))?;
        Ok(Socket { path, listener })
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A child that is killed if this goes out of scope before it is waited for.
///
/// An interpreter left running with nobody reading its socket is the failure
/// this whole arrangement is supposed to bound, so it is not left to a signal.
struct Interpreter(Child);

impl Drop for Interpreter {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `program` in a child, answering every capability call from `broker`.
///
/// The loop is `drive_recording`'s, with a wire where the `Vm` was: the child
/// says what it did and what it needs, and this side performs and answers.
pub fn drive(
    program: &sic_bytecode::Program,
    entry: u32,
    run: sic_journal::RunId,
    fuel: u64,
    broker: &mut Broker,
    sink: &mut dyn Sink,
    record_into: Option<&std::path::Path>,
) -> Result<Ended, String> {
    let socket = Socket::open(&run.to_string())?;
    let me =
        std::env::current_exe().map_err(|e| format!("cannot tell where this binary is: {e}"))?;
    let child = Command::new(&me)
        .args(["vm", "--socket"])
        .arg(&socket.path)
        .spawn()
        .map_err(|e| format!("cannot start the interpreter: {e}"))?;
    let mut child = Interpreter(child);

    let (mut stream, _) = socket
        .listener
        .accept()
        .map_err(|e| format!("the interpreter did not connect: {e}"))?;

    send(
        &mut stream,
        &ToVm::Start {
            program: sic_bytecode::encode(program),
            entry,
            fuel,
            run: run.0,
        }
        .to_bytes(),
    )
    .map_err(|e| format!("cannot send the program: {e}"))?;

    let ended = converse(&mut stream, broker, sink, record_into)?;
    // It has said everything; wait for it rather than killing it, so that a
    // child which is about to exit cleanly is not reported as killed.
    let _ = child.0.wait();
    Ok(ended)
}

fn converse(
    stream: &mut UnixStream,
    broker: &mut Broker,
    sink: &mut dyn Sink,
    record_into: Option<&std::path::Path>,
) -> Result<Ended, String> {
    loop {
        let Some(bytes) = recv(stream).map_err(|e| e.to_string())? else {
            // The child closed without saying how the run ended. That is the
            // case this arrangement exists for: it ran out of memory, or it was
            // killed. The parent still has every event it was sent.
            return Err("the interpreter stopped without saying how the run ended; \
                 the journal has everything it managed to say"
                .to_string());
        };
        match FromVm::from_bytes(&bytes).map_err(|e| e.to_string())? {
            FromVm::Event(event) => sink.emit(&event),
            FromVm::Request(request) => {
                let answer = broker.call(&request);
                if let Ok(sic_core::CapOutcome::Value(value)) = &answer {
                    if let Some(dir) = record_into {
                        let recorded = super::store::Answer::from_broker(value);
                        if let Err(msg) = super::store::record_answer(dir, &recorded) {
                            eprintln!("warning: {msg}");
                        }
                    }
                }
                send(stream, &ToVm::Answer(answer).to_bytes())
                    .map_err(|e| format!("cannot answer the interpreter: {e}"))?;
            }
            // Unit 4 of `docs/design/processes.md`. Until it arrives, `run`
            // refuses `--isolate` for a program that could suspend, so nothing
            // reaches here.
            FromVm::Checkpoint(_) => {
                return Err("a checkpoint crossed a wire that does not carry one yet".to_string());
            }
            FromVm::Ended(ended) => return Ok(ended),
        }
    }
}

/// Reports how an isolated run ended, in the parent's voice.
pub fn finish(ended: Ended) -> ExitCode {
    match ended {
        Ended::Finished(text) if text.is_empty() => ExitCode::SUCCESS,
        Ended::Finished(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        // Rendered by the child, which is the side the value and the source
        // position live on. See `wire::Ended`.
        Ended::Failed(text) => {
            eprintln!("error: {text}");
            ExitCode::from(EXIT_FAILURE)
        }
        Ended::Suspended(question) => {
            eprintln!("error: the run is waiting for `{question}` and `--isolate` cannot save it");
            eprintln!("       run it without `--isolate` to write a checkpoint");
            ExitCode::from(EXIT_SUSPENDED)
        }
    }
}

/// Whether a program has any capability call that could stop the run.
///
/// `--isolate` cannot write a checkpoint yet, so a program that might need one
/// is refused before it starts rather than after it has done half its work.
/// The three that can defer are the ones nothing in this process can answer.
pub fn could_suspend(program: &sic_bytecode::Program) -> Option<&str> {
    program
        .caps
        .iter()
        .map(|c| c.name.as_str())
        .find(|name| matches!(*name, "llm.invoke" | "human.approve" | "human.choose"))
}
