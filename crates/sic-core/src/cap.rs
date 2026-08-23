//! The types that cross the boundary between the VM and the capability broker.
//!
//! They live in `sic-core` because both sides need them and neither may depend
//! on the other: the VM must not be able to reach an implementation of an
//! effect, and the broker must not be able to reach into the VM's state.
//!
//! This is the future IPC boundary, so nothing here refers to VM memory. A
//! `CapValue` owns its data and can be written out and read back.

/// What kind of effect a capability has. This is what a plan or an audit log
/// summarizes, so it is coarse on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CapKind {
    Read = 0,
    Write = 1,
    Exec = 2,
    Invoke = 3,
}

impl CapKind {
    pub fn from_u8(v: u8) -> Option<CapKind> {
        Some(match v {
            0 => CapKind::Read,
            1 => CapKind::Write,
            2 => CapKind::Exec,
            3 => CapKind::Invoke,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            CapKind::Read => "read",
            CapKind::Write => "write",
            CapKind::Exec => "exec",
            CapKind::Invoke => "invoke",
        }
    }
}

/// A value passed to or returned from a capability.
#[derive(Debug, Clone, PartialEq)]
pub enum CapValue {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    /// An argument vector, and nothing more general than one.
    ///
    /// Strings rather than values: nesting would buy a depth limit, a recursive
    /// encoder and a decoder that has to refuse a hostile depth, and nothing
    /// that exists needs any of it. See `docs/design/arguments.md`.
    List(Vec<String>),
}

impl CapValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            CapValue::Unit => "Unit",
            CapValue::Bool(_) => "Bool",
            CapValue::I64(_) => "Int",
            CapValue::F64(_) => "Float",
            CapValue::Str(_) => "String",
            CapValue::List(_) => "List<String>",
        }
    }

    /// The strings behind a `List`, for a broker that expects one.
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            CapValue::List(items) => Some(items),
            _ => None,
        }
    }

    /// The string behind a `Str`, for a broker that expects one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            CapValue::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// One entry of a module's manifest, as the broker sees it.
///
/// The broker is given the manifest, not the bytecode: it needs to know what
/// was granted, and nothing about how the module is compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapGrant {
    pub name: String,
    pub kind: CapKind,
    /// What the grant is limited to. Its meaning belongs to the capability.
    pub constraint: String,
    /// The digest the file has to have, or empty for a grant that does not pin
    /// what runs.
    pub pin: String,
    /// What the argument vector has to start with. Empty means the call may
    /// pass no arguments at all, which is what every grant meant before
    /// arguments existed.
    pub args: Vec<String>,
}

/// What the VM asks the broker to do.
#[derive(Debug, Clone, PartialEq)]
pub struct CapRequest {
    /// Index into the module's capability manifest. The broker checks the
    /// request against that entry rather than trusting the name.
    pub index: u32,
    pub name: String,
    pub args: Vec<CapValue>,
    /// The task waiting on this call. With several tasks in flight, an answer
    /// has to say which one it answers.
    pub task: u32,
    /// Which attempt this is, counting from 1. Retrying is the VM's decision,
    /// so the broker is told rather than asked.
    pub attempt: u32,
    /// How long the broker may take, in milliseconds; 0 means no deadline.
    ///
    /// The deadline is enforced here because the broker is the only side with
    /// a clock, and the VM must stay unable to read one.
    pub timeout_ms: u32,
    /// Which conversation this call belongs to, or 0 for one that starts fresh.
    ///
    /// The number identifies the caller; the task identifies which of its
    /// conversations. Both are needed, because two agents that each keep one
    /// must not end up in the same one, and the same agent in two tasks must
    /// not either.
    pub conversation: u32,
}

/// What came back from a capability call.
///
/// `Deferred` is what makes durable execution necessary rather than optional:
/// some effects cannot answer within the call. A human has to approve
/// something, a job has to finish, a model has to be asked. The run stops, its
/// state is written out, and it continues when the answer arrives - possibly in
/// another process, on another day.
#[derive(Debug, Clone, PartialEq)]
pub enum CapOutcome {
    /// The effect happened and produced this value.
    Value(CapValue),
    /// The effect will not answer now. The run must be suspended.
    Deferred {
        /// What is being waited for, in a form a person can read. This is shown
        /// to whoever has to supply the answer, and never written to telemetry.
        question: String,
    },
}

/// Why a capability call did not happen, or did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapError {
    pub message: String,
}

impl CapError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
