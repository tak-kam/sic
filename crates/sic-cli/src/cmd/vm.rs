//! `sic vm`: the interpreter, as a process of its own.
//!
//! Started by a run rather than by a person, like `sic mcp` and `sic hook`. It
//! is handed a socket, reads a program over it, runs it, and asks the other end
//! for every effect. It opens no file and starts no process: what it may do is
//! what the parent will answer.
//!
//! `docs/design/processes.md` §5 says why the child is this side and not the
//! broker. The short of it: the point of the split is to give the side that
//! runs the bytecode less than the side that performs effects, and a parent
//! cannot be given less than its child.

use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::rc::Rc;

use sic_journal::{Event, Journal, RunId, Sink};
use sic_vm::{Status, Value, Vm};

use crate::wire::{Ended, FromVm, ToVm, recv, send};

use super::{EXIT_FAILURE, EXIT_USAGE};

/// Both directions of one socket.
///
/// The journal's sink writes to it and the run loop reads from it, so they
/// share one handle. Single-threaded and no lock: the VM is not running while
/// the loop is waiting, and the sink only writes while the VM is.
#[derive(Debug, Clone)]
struct Wire(Rc<RefCell<UnixStream>>);

impl Wire {
    fn tell(&self, message: &FromVm) -> Result<(), String> {
        send(&mut *self.0.borrow_mut(), &message.to_bytes())
            .map_err(|e| format!("cannot reach the run: {e}"))
    }

    fn hear(&self) -> Result<ToVm, String> {
        let bytes = recv(&mut *self.0.borrow_mut())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the run went away".to_string())?;
        ToVm::from_bytes(&bytes).map_err(|e| e.to_string())
    }
}

/// A sink that puts every event on the wire.
///
/// The parent owns the journal file, the run store and the terminal; this side
/// owns none of them, which is the point. An event that cannot be sent is not a
/// reason to take the run down, and not a reason to be quiet either - the same
/// reading `FileSink` gives a journal it cannot write.
impl Sink for Wire {
    fn emit(&mut self, event: &Event) {
        if let Err(e) = self.tell(&FromVm::Event(event.clone())) {
            eprintln!("warning: {e}");
        }
    }
}

/// The two ways a run begins here.
enum Begin {
    Fresh { entry: u32, fuel: u64, run: u128 },
    Resumed { checkpoint: Vec<u8> },
}

pub fn run(socket: &str) -> ExitCode {
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("error: cannot reach the run at `{socket}`: {e}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let wire = Wire(Rc::new(RefCell::new(stream)));

    match drive(&wire) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn drive(wire: &Wire) -> Result<ExitCode, String> {
    // Two ways to begin and one way to go on. A run that is starting is told
    // where to start; a run that already exists arrives as the state it was
    // written out as, and the answer it was waiting for comes after.
    //
    // The program is kept as bytes as well as decoded: a checkpoint is tied to
    // the digest of the bytecode it came from, and the bytes that digest is of
    // are the ones that arrived. Re-encoding what was decoded would be a second
    // opinion about the same program, and `sic resume` compares against the
    // parent's.
    let (sent, start) = match wire.hear()? {
        ToVm::Start {
            program,
            entry,
            fuel,
            run,
        } => (program, Begin::Fresh { entry, fuel, run }),
        ToVm::Restore {
            program,
            checkpoint,
        } => (program, Begin::Resumed { checkpoint }),
        _ => return Err("the run said something other than what to run".to_string()),
    };
    let program = sic_bytecode::decode(&sent)
        .map_err(|e| format!("the run sent bytecode this cannot read: {e}"))?;
    // The one door into the VM, on this side of the wire too. A program that
    // arrived over a socket has exactly as much claim to be verified as one
    // that arrived off a disk - see `cmd::verified`.
    let report = sic_verify::verify(&program);
    if !report.ok() {
        let why: Vec<String> = report.errors.iter().map(|e| e.to_string()).collect();
        return Err(format!(
            "the run sent bytecode that does not verify: {}",
            why.join("; ")
        ));
    }

    let sink = Box::new(wire.clone());
    let (mut vm, mut status) = match start {
        Begin::Fresh { entry, fuel, run } => {
            let journal = Journal::new(RunId(run), sink);
            let mut vm = Vm::with_journal(&program, fuel, journal);
            let status = vm.run(entry, &[]);
            (vm, status)
        }
        Begin::Resumed { checkpoint } => {
            // The digest ties the state to the bytecode it came from, and the
            // bytecode is what arrived - the same reason a checkpoint written
            // over this wire is digested against what was sent.
            let digest = sic_core::Digest::of(&sent);
            let (mut vm, _question) = Vm::restore(&program, &checkpoint, digest, sink)
                .map_err(|e| format!("cannot pick the run up again: {e}"))?;
            // The answer the parent shaped for the call this stopped at. It is
            // sent after the state because the parent needs the state to know
            // what shape to give it.
            let ToVm::Resume(value) = wire.hear()? else {
                return Err("the run sent something other than an answer".to_string());
            };
            let status = vm.resume(value);
            (vm, status)
        }
    };

    let ended = loop {
        match status {
            Status::Suspended(request) => {
                wire.tell(&FromVm::Request(request))?;
                match wire.hear()? {
                    ToVm::Answer(Ok(sic_core::CapOutcome::Value(value))) => {
                        status = vm.resume(value);
                    }
                    ToVm::Answer(Ok(sic_core::CapOutcome::Deferred { question })) => {
                        // The state is here and the filesystem is not, so this
                        // side produces the bytes and the parent writes them.
                        let digest = sic_core::Digest::of(&sent);
                        let Some(bytes) = vm.checkpoint(digest, &question) else {
                            return Err("the run is waiting and has no state to save".to_string());
                        };
                        wire.tell(&FromVm::Checkpoint(bytes))?;
                        break Ended::Suspended(question);
                    }
                    ToVm::Answer(Err(error)) => status = vm.resume_failed(&error),
                    _ => return Err("the run answered a call with something else".to_string()),
                }
            }
            // The value lives in this arena, so this side is the one that can
            // say what it was.
            Status::Finished(Value::Unit) => break Ended::Finished(String::new()),
            Status::Finished(value) => break Ended::Finished(vm.display(&value)),
            Status::Failed(info) => {
                let mut text = info.kind.message().to_string();
                if let Some(value) = &info.value {
                    text.push_str(&format!(": {}", vm.display(value)));
                }
                if let Some(detail) = &info.detail {
                    text.push_str(&format!(": {detail}"));
                }
                match program.debug.position(info.pc) {
                    Some((line, col)) => {
                        let file = program.debug.file(info.pc).unwrap_or("?");
                        text.push_str(&format!("\n --> {file}:{line}:{col}"));
                    }
                    None => {
                        text.push_str(&format!(
                            "\n --> in `{}` at instruction {}",
                            info.func, info.pc
                        ));
                    }
                }
                break Ended::Failed(text);
            }
        }
    };

    let failed = matches!(ended, Ended::Failed(_) | Ended::Suspended(_));
    wire.tell(&FromVm::Ended(ended))?;
    Ok(match failed {
        true => ExitCode::from(EXIT_FAILURE),
        false => ExitCode::SUCCESS,
    })
}
