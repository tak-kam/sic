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

use super::EXIT_FAILURE;

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
        let listener = UnixListener::bind(&path).map_err(|e| {
            format!(
                "cannot listen at `{}`: {e}; `--no-isolate` runs the interpreter here",
                path.display()
            )
        })?;
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

/// Why the conversation ended without an ending.
enum Stopped {
    /// The child closed the socket. Its exit status is what is left to read.
    Silently,
    /// Something on this side went wrong, and it already knows what.
    Saying(String),
}

/// What became of an interpreter that stopped without saying how the run ended.
///
/// Three different things, and they used to read the same. Which one it was is
/// the difference between "the machine took the run" and "sic has a bug", and a
/// person reading one line should not have to guess which.
///
/// There is no fourth case for a child that is taking too long, and no timeout
/// waiting for one. A sic program cannot run forever: fuel is spent on every
/// instruction, v0.1 has no loops, and recursion stops at `MAX_FRAMES`. So a
/// child that has not answered is either waiting on this side - which is a
/// build that takes an hour, and killing it would be the wrong answer - or it
/// has a bug, and a timeout would be a guess about how long sic's own bugs
/// take. See `docs/design/processes.md` §5b.
fn how_it_went(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    if let Some(signal) = status.signal() {
        // The case this whole arrangement is for: a run that grew too large is
        // killed here rather than taking the parent with it.
        return format!(
            "the interpreter was killed by signal {signal}; \
             the journal has everything it managed to say"
        );
    }
    match status.code() {
        // It ran, it stopped, and it said why on its own stderr.
        Some(code) if code != 0 => {
            format!("the interpreter exited {code} without saying how the run ended")
        }
        // Nothing produces this. A child that finished cleanly sent an ending
        // first, so reaching here is a bug in sic rather than anything about
        // the program.
        _ => "the interpreter finished without saying how the run ended, which is a bug in sic"
            .to_string(),
    }
}

impl Drop for Interpreter {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How an isolated run ended, and the state it left if it stopped to wait.
///
/// The checkpoint is bytes rather than a file: the child holds the state and
/// the parent holds the filesystem, which is the split that makes the child
/// able to be given less.
pub struct Ran {
    pub ended: Ended,
    pub checkpoint: Option<Vec<u8>>,
}

/// How a run begins in the child.
///
/// The same two the child knows about, from this side. A fresh run is told
/// where to start; one that already exists is handed the state it was written
/// out as, and then the answer it was waiting for.
pub enum Begin<'a> {
    Fresh {
        entry: u32,
        run: sic_journal::RunId,
        fuel: u64,
    },
    Resumed {
        checkpoint: &'a [u8],
        answer: sic_core::CapValue,
    },
}

/// Runs `program` in a child, answering every capability call from `broker`.
///
/// The loop is `drive_recording`'s, with a wire where the `Vm` was: the child
/// says what it did and what it needs, and this side performs and answers.
pub fn drive(
    program: &sic_bytecode::Program,
    begin: Begin<'_>,
    broker: &mut Broker,
    sink: &mut dyn Sink,
    record_into: Option<&std::path::Path>,
) -> Result<Ran, String> {
    // Only used to name the socket, so a fresh run's id and a resumed run's
    // process id are equally good: what matters is that two runs on one
    // machine do not collide.
    let named = match &begin {
        Begin::Fresh { run, .. } => run.to_string(),
        Begin::Resumed { .. } => format!("resumed-{}", std::process::id()),
    };
    let socket = Socket::open(&named)?;
    // Both of these fail before any bytecode has run, and both name the way
    // out: the interpreter in a process of its own is the default here, so a
    // machine that cannot start one has to be able to say so on the command
    // line rather than be stuck. There is no silent fallback - see
    // `docs/design/processes.md` §7.
    let me = std::env::current_exe().map_err(|e| {
        format!("cannot tell where this binary is: {e}; `--no-isolate` runs the interpreter here")
    })?;
    let child = Command::new(&me)
        .args(["vm", "--socket"])
        .arg(&socket.path)
        .spawn()
        .map_err(|e| {
            format!("cannot start the interpreter: {e}; `--no-isolate` runs it here instead")
        })?;
    let mut child = Interpreter(child);

    let (mut stream, _) = socket
        .listener
        .accept()
        .map_err(|e| format!("the interpreter did not connect: {e}"))?;

    let encoded = sic_bytecode::encode(program);
    let told = match begin {
        Begin::Fresh { entry, run, fuel } => ToVm::Start {
            program: encoded,
            entry,
            fuel,
            run: run.0,
        },
        Begin::Resumed { checkpoint, .. } => ToVm::Restore {
            program: encoded,
            checkpoint: checkpoint.to_vec(),
        },
    };
    let resuming = matches!(told, ToVm::Restore { .. });
    send(&mut stream, &told.to_bytes()).map_err(|e| format!("cannot send the program: {e}"))?;
    if resuming {
        // After the state, because the parent needed the state to know what
        // shape to give the answer.
        let Begin::Resumed { answer, .. } = begin else {
            unreachable!("`resuming` is only true for a resumed run");
        };
        send(&mut stream, &ToVm::Resume(answer).to_bytes())
            .map_err(|e| format!("cannot send the answer: {e}"))?;
    }

    let ran = converse(&mut stream, broker, sink, record_into);
    // Waited for either way, and before anything is reported: a child that
    // stopped without an ending is one whose exit status is the only account
    // left of what happened to it.
    let status = child.0.wait();
    match (ran, status) {
        (Ok(ran), _) => Ok(ran),
        (Err(Stopped::Silently), Ok(status)) => Err(how_it_went(status)),
        (Err(Stopped::Silently), Err(e)) => Err(format!(
            "the interpreter stopped and cannot be waited for: {e}"
        )),
        (Err(Stopped::Saying(message)), _) => Err(message),
    }
}

fn converse(
    stream: &mut UnixStream,
    broker: &mut Broker,
    sink: &mut dyn Sink,
    record_into: Option<&std::path::Path>,
) -> Result<Ran, Stopped> {
    let mut checkpoint = None;
    loop {
        let Some(bytes) = recv(stream).map_err(|e| Stopped::Saying(e.to_string()))? else {
            // The child closed without saying how the run ended. Its exit
            // status is the only account left, and the caller reads it.
            return Err(Stopped::Silently);
        };
        match FromVm::from_bytes(&bytes).map_err(|e| Stopped::Saying(e.to_string()))? {
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
                    .map_err(|e| Stopped::Saying(format!("cannot answer the interpreter: {e}")))?;
            }
            // It arrives before the ending it belongs to, so it is held until
            // the child says what that ending was.
            FromVm::Checkpoint(bytes) => checkpoint = Some(bytes),
            FromVm::Ended(ended) => return Ok(Ran { ended, checkpoint }),
        }
    }
}

/// Reports how an isolated run ended, in the parent's voice.
///
/// The same words `run::finish` uses, because a person should not be able to
/// tell which shape ran their program from what it printed.
pub fn finish(ran: Ran, checkpoint_path: Option<&str>, how: super::run::Stopping<'_>) -> ExitCode {
    match ran.ended {
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
            let Some(path) = checkpoint_path else {
                eprintln!("error: the run is waiting for `{question}` and has nowhere to be saved");
                eprintln!("       pass --checkpoint PATH to write its state out");
                return ExitCode::from(EXIT_FAILURE);
            };
            let Some(bytes) = ran.checkpoint else {
                eprintln!("internal error: the run is waiting but sent no state to save");
                return ExitCode::from(EXIT_FAILURE);
            };
            super::run::saved(&bytes, path, &question, how)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    /// The three ways an interpreter can stop without saying how the run
    /// ended, told apart. They used to read the same, and which one it was is
    /// the difference between the machine taking the run and sic having a bug.
    #[test]
    fn how_an_interpreter_stopped_is_three_different_things() {
        // A wait status: the signal is the low seven bits, the exit code is
        // the byte above them.
        let killed = how_it_went(std::process::ExitStatus::from_raw(9));
        assert!(killed.contains("killed by signal 9"), "{killed}");
        assert!(killed.contains("the journal has everything"), "{killed}");

        let failed = how_it_went(std::process::ExitStatus::from_raw(2 << 8));
        assert!(failed.contains("exited 2"), "{failed}");
        assert!(!failed.contains("signal"), "{failed}");

        // Nothing produces this one, and saying so is the point: a reader who
        // meets it should know it is not about their program.
        let quiet = how_it_went(std::process::ExitStatus::from_raw(0));
        assert!(quiet.contains("a bug in sic"), "{quiet}");
    }
}
