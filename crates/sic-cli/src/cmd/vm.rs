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
    let ToVm::Start {
        program,
        entry,
        fuel,
        run,
    } = wire.hear()?
    else {
        return Err("the run said something other than what to run".to_string());
    };

    let program = sic_bytecode::decode(&program)
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

    let journal = Journal::new(RunId(run), Box::new(wire.clone()));
    let mut vm = Vm::with_journal(&program, fuel, journal);
    let mut status = vm.run(entry, &[]);

    let ended = loop {
        match status {
            Status::Suspended(request) => {
                wire.tell(&FromVm::Request(request))?;
                match wire.hear()? {
                    ToVm::Answer(Ok(sic_core::CapOutcome::Value(value))) => {
                        status = vm.resume(value);
                    }
                    ToVm::Answer(Ok(sic_core::CapOutcome::Deferred { question })) => {
                        // Unit 4 of `docs/design/processes.md` is where the
                        // checkpoint crosses. Until it does, the parent refuses
                        // a `--isolate` run that would have to wait, so this is
                        // a thing the run has already declined to ask for.
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
