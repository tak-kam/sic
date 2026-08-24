//! `sic resume <CHECKPOINT> <FILE.sic> --value <VALUE>`: continue a run that
//! stopped to wait.
//!
//! The source is compiled again and its digest has to match the one in the
//! checkpoint. That is what stops a run from being continued inside a program
//! that has changed since it was suspended.

use std::process::ExitCode;

use sic_broker::Broker;
use sic_core::Digest;
use sic_journal::NullSink;
use sic_vm::Vm;

use super::drive::{Needs, answer_for, drive, manifest};
use super::journal::FileSink;
use super::run::finish;
use super::{EXIT_FAILURE, EXIT_USAGE, compile_source, read_bytes};

pub struct ResumeOptions<'a> {
    pub value: Option<&'a str>,
    pub journal: Option<&'a str>,
    /// Where to write the state if the run has to wait again.
    pub checkpoint: Option<&'a str>,
    /// What is to answer `llm.invoke` from here on.
    pub llm: Option<&'a str>,
}

/// A checkpoint that no longer belongs to the program it is being resumed
/// against.
///
/// There is no `--force`, and `docs/design/durable-execution.md` says why: the
/// checkpoint holds registers and frames whose meaning is the function layout
/// of the bytecode it came from, and nothing here can tell a harmless edit from
/// a different program. What was missing was not a door - it was a sentence
/// saying which way is out, since a runtime that quietly accumulates
/// unresumable state is not being safe, it is being unhelpful about being safe.
fn orphaned(checkpoint_path: &str, source_path: &str) -> ExitCode {
    eprintln!(
        "error: `{checkpoint_path}` was taken from different bytecode than `{source_path}` \
         compiles to now, so it cannot be resumed"
    );
    eprintln!("       the digest is over bytecode, so a comment or a rename is not what did this");
    eprintln!("       if the run was recorded, it kept its own bytecode and is still yours:");
    eprintln!("           sic runs --waiting");
    eprintln!("           sic attach <RUN-ID> --value <VALUE>");
    eprintln!("       otherwise this run cannot be continued, and starting again is `sic run`");
    ExitCode::from(EXIT_FAILURE)
}

pub fn run(checkpoint_path: &str, source_path: &str, options: ResumeOptions<'_>) -> ExitCode {
    let bytes = match read_bytes(checkpoint_path) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let program = match compile_source(source_path) {
        Ok(v) => v,
        Err(code) => return code,
    };
    // A checkpoint is only meaningful against the bytecode it was taken from.
    let digest = Digest::of(&sic_bytecode::encode(&program));

    let sink: Box<dyn sic_journal::Sink> = match options.journal {
        Some(path) => match FileSink::append(path) {
            Ok(sink) => Box::new(sink),
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::from(EXIT_FAILURE);
            }
        },
        None => Box::new(NullSink),
    };

    // A checkpoint is state, not a program: what runs after it is resumed is
    // this bytecode, and it has to have been checked like any other.
    if let Err(code) = super::verified(&program, super::From::Compiler(source_path)) {
        return code;
    }

    // Checked here as well as inside `restore`, which enforces it, so that the
    // one failure a person is most likely to meet can say what to do instead of
    // only what went wrong. The broker re-checks what the compiler checked for
    // the same reason.
    if let Ok(saved) = sic_vm::Checkpoint::decode(&bytes) {
        if saved.program_digest != digest {
            return orphaned(checkpoint_path, source_path);
        }
    }

    let (mut vm, question) = match Vm::restore(&program, &bytes, digest, sink) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: cannot resume from `{checkpoint_path}`: {e}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let value = match answer_for(&program, &vm, options.value) {
        Ok(value) => value,
        Err(Needs::Reported(code)) => return code,
        Err(Needs::Answer(tag)) => {
            // Without the answer there is nothing to continue with, so say what
            // is being asked and what shape the answer has to take.
            eprintln!("waiting: {question}");
            eprintln!(
                "error: `resume` needs the answer: --value <{}>",
                tag.short_name()
            );
            return ExitCode::from(EXIT_USAGE);
        }
    };

    // A conversation lives in its run's session, and a loose checkpoint does
    // not say which run it came from. Starting a fresh one and continuing as if
    // it were the old one would change what the run means without saying so.
    if options.llm.is_some() && program.policies.iter().any(|p| p.conversation != 0) {
        eprintln!(
            "error: this program keeps a conversation, and a checkpoint does not say which run \
             it belongs to"
        );
        eprintln!(
            "       continue a recorded run instead: sic attach <RUN-ID> --value V --llm <SPEC>"
        );
        return ExitCode::from(EXIT_USAGE);
    }
    let session = sic_broker::tmux::Session {
        run: super::journal::new_run_id().to_string(),
        continuing: false,
        state: None,
    };
    let grants = manifest(&program);
    let mut broker = match super::run::open_driver(options.llm, session, &grants, None) {
        Ok(Some(driver)) => Broker::with_driver(grants, driver),
        Ok(None) => Broker::new(grants),
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let status = vm.resume(value);
    let outcome = drive(&mut vm, &mut broker, status);
    broker.finish(matches!(outcome, super::drive::Outcome::Suspended { .. }));
    finish(&mut vm, &program, outcome, options.checkpoint, None)
}
