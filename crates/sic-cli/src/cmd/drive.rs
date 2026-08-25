//! Running a program to completion, or to the point where it has to stop.
//!
//! This loop is the whole of the VM's access to the outside world: it takes the
//! capability requests the VM suspends on, asks the broker, and hands the
//! answers back. When an answer is not available now, the run stops here and a
//! checkpoint takes over.

use std::process::ExitCode;

use sic_broker::Broker;
use sic_bytecode::{Program, TypeDesc};
use sic_core::{CapGrant, CapOutcome, CapValue};
use sic_vm::{FailInfo, Status, Value, Vm};

/// How a run ended, from the driver's point of view.
#[derive(Debug)]
pub enum Outcome {
    Finished(Value),
    Failed(FailInfo),
    /// The run is waiting for something that is not in this process.
    Suspended {
        question: String,
    },
}

pub fn drive(vm: &mut Vm, broker: &mut Broker, status: Status) -> Outcome {
    drive_recording(vm, broker, status, None)
}

/// The same loop, recording each answer where a run is being kept.
pub fn drive_recording(
    vm: &mut Vm,
    broker: &mut Broker,
    mut status: Status,
    record_into: Option<&std::path::Path>,
) -> Outcome {
    loop {
        match status {
            Status::Suspended(request) => match broker.call(&request) {
                Ok(CapOutcome::Value(value)) => {
                    // What the agent reached for while answering. Recorded
                    // before the answer, because that is the order it happened
                    // in.
                    vm.record_tool_uses(&broker.take_tool_uses());
                    if let Some(dir) = record_into {
                        let answer = super::store::Answer::from_broker(&value);
                        if let Err(msg) = super::store::record_answer(dir, &answer) {
                            eprintln!("warning: {msg}");
                        }
                    }
                    status = vm.resume(value)
                }
                Ok(CapOutcome::Deferred { question }) => {
                    return Outcome::Suspended { question };
                }
                Err(error) => status = vm.resume_failed(&error),
            },
            Status::Finished(value) => return Outcome::Finished(value),
            Status::Failed(info) => return Outcome::Failed(info),
        }
    }
}

/// The manifest the broker enforces, taken from the bytecode.
pub fn manifest(program: &Program) -> Vec<CapGrant> {
    program
        .caps
        .iter()
        .map(|c| CapGrant {
            name: c.name.clone(),
            kind: c.kind,
            constraint: c.constraints.clone(),
            pin: c.pin.clone(),
            args: c.args.clone(),
            delegable: c.delegable,
            dir: c.dir.clone(),
            env: c.env.clone(),
        })
        .collect()
}

/// Reads a value supplied on the command line, in the type the capability
/// returns.
pub fn parse_answer(text: &str, tag: &TypeDesc) -> Result<CapValue, String> {
    Ok(match tag {
        TypeDesc::Bool => match text {
            "true" => CapValue::Bool(true),
            "false" => CapValue::Bool(false),
            other => return Err(format!("`{other}` is not `true` or `false`")),
        },
        TypeDesc::Int => CapValue::I64(
            text.parse()
                .map_err(|_| format!("`{text}` is not an integer"))?,
        ),
        TypeDesc::Float => CapValue::F64(
            text.parse()
                .map_err(|_| format!("`{text}` is not a number"))?,
        ),
        TypeDesc::Str => CapValue::Str(text.to_string()),
        TypeDesc::Unit => CapValue::Unit,
        // A capability answers with one value the broker can produce; the
        // verifier would have refused a manifest asking for anything else.
        TypeDesc::Task(_) | TypeDesc::List(_) | TypeDesc::Object { .. } => {
            return Err(format!(
                "a capability cannot answer with a {}",
                tag.short_name()
            ));
        }
    })
}

/// The type a capability returns, for reading an answer.
pub fn capability_return_type<'a>(program: &'a Program, cap: &str) -> Option<&'a TypeDesc> {
    let decl = program.caps.iter().find(|c| c.name == cap)?;
    program.types.get(decl.ret_type as usize)
}

/// What a waiting run still needs before it can go on.
///
/// The two commands that pick a waiting run up are answered by different
/// things - `resume` by whoever holds the checkpoint file, `attach` by whatever
/// is driving a recorded run - so each of them says "nobody gave me an answer"
/// its own way, on its own stream, with its own exit code. What they must not
/// disagree about is what the run is waiting for and what shape the answer has
/// to take, and that is what this leaves to one place.
pub enum Needs<'a> {
    /// No answer was supplied. The type is what to ask for; the question came
    /// back from `Vm::restore`, so the caller already has it.
    Answer(&'a TypeDesc),
    /// Already reported, with the code to exit on. A checkpoint that is not
    /// waiting, a capability the program does not declare, and text that is not
    /// the type that capability returns are the same failure whichever command
    /// reached them, so neither command is asked to word them again.
    Reported(ExitCode),
}

/// The answer a waiting VM is to be resumed with.
///
/// `resume` and `attach` each restore a checkpoint, find what it stopped on,
/// look up the type that capability returns, and read the answer in that type.
/// Written out twice, the sequence drifted - which is what happens when three
/// lookups and four error paths are something every caller has to get right on
/// its own.
/// Turns `--value` into the answer the waiting call is shaped for.
///
/// The capability's name rather than the `Vm` it came from: when the run is in
/// another process there is no `Vm` on this side, and the checkpoint says what
/// is being waited on. One shape for both, rather than the same parsing twice.
pub fn answer_for<'a>(
    program: &'a Program,
    cap: &str,
    value: Option<&str>,
) -> Result<CapValue, Needs<'a>> {
    let Some(tag) = capability_return_type(program, cap) else {
        eprintln!("error: `{cap}` is not a capability this program declares");
        return Err(Needs::Reported(ExitCode::from(super::EXIT_FAILURE)));
    };
    let Some(text) = value else {
        return Err(Needs::Answer(tag));
    };
    parse_answer(text, tag).map_err(|msg| {
        eprintln!("error: {msg}, and `{cap}` returns {}", tag.short_name());
        Needs::Reported(ExitCode::from(super::EXIT_USAGE))
    })
}
