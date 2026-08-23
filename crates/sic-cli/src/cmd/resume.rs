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

use super::drive::{capability_return_type, drive, manifest, parse_answer};
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

    let (mut vm, question) = match Vm::restore(&program, &bytes, digest, sink) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: cannot resume from `{checkpoint_path}`: {e}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let Some(cap) = vm.pending_capability().map(str::to_string) else {
        eprintln!("internal error: the checkpoint is not waiting for anything");
        return ExitCode::from(EXIT_FAILURE);
    };
    let Some(tag) = capability_return_type(&program, &cap) else {
        eprintln!("error: `{cap}` is not a capability this program declares");
        return ExitCode::from(EXIT_FAILURE);
    };

    let Some(text) = options.value else {
        // Without the answer there is nothing to continue with, so say what is
        // being asked and what shape the answer has to take.
        eprintln!("waiting: {question}");
        eprintln!(
            "error: `resume` needs the answer: --value <{}>",
            tag.short_name()
        );
        return ExitCode::from(EXIT_USAGE);
    };
    let value = match parse_answer(text, tag) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("error: {msg}, and `{cap}` returns {}", tag.short_name());
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let mut broker = match super::run::open_driver(options.llm, None) {
        Ok(Some(driver)) => Broker::with_driver(manifest(&program), driver),
        Ok(None) => Broker::new(manifest(&program)),
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let status = vm.resume(value);
    let outcome = drive(&mut vm, &mut broker, status);
    finish(&mut vm, &program, outcome, options.checkpoint, None)
}
