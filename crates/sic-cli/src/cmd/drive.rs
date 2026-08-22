//! Running a program to completion, or to the point where it has to stop.
//!
//! This loop is the whole of the VM's access to the outside world: it takes the
//! capability requests the VM suspends on, asks the broker, and hands the
//! answers back. When an answer is not available now, the run stops here and a
//! checkpoint takes over.

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

pub fn drive(vm: &mut Vm, broker: &mut Broker, mut status: Status) -> Outcome {
    loop {
        match status {
            Status::Suspended(request) => match broker.call(&request) {
                Ok(CapOutcome::Value(value)) => status = vm.resume(value),
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
        })
        .collect()
}

/// Reads a value supplied on the command line, in the type the capability
/// returns.
pub fn parse_answer(text: &str, tag: TypeDesc) -> Result<CapValue, String> {
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
        // A capability cannot produce a task; the verifier would have refused
        // the manifest.
        TypeDesc::Task(_) => {
            return Err("a capability cannot answer with a task".to_string());
        }
    })
}

/// The type a capability returns, for reading an answer.
pub fn capability_return_type(program: &Program, cap: &str) -> Option<TypeDesc> {
    let decl = program.caps.iter().find(|c| c.name == cap)?;
    program.types.get(decl.ret_type as usize).copied()
}
