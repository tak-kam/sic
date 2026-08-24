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

/// Something an agent did while answering a model call.
///
/// Two things, kept apart on purpose. A capability the agent reached through
/// the broker is a capability call and enters the journal as one. A tool of the
/// agent's own is not: the manifest does not name `Bash` or `Edit`, and
/// recording one as a capability would be the journal calling something a
/// capability because there was nowhere else to put it.
///
/// Digests rather than values, because this reaches the journal. It lives here
/// rather than in the broker for the reason everything else in this file does:
/// the VM has to be able to read one without being able to see the crate that
/// performs effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    Capability {
        cap: String,
        args: crate::Digest,
        /// The answer, or the message of the error that stopped it.
        outcome: std::result::Result<crate::Digest, String>,
    },
    Tool {
        tool: String,
        /// What the call was about, as a digest. The whole input: for an edit
        /// that is a path and a diff, and only one of those is safe to keep.
        input: crate::Digest,
        allowed: bool,
        /// Why not, when sic refused it.
        reason: String,
    },
}

// ---- the wire ----
//
// This file's own opening paragraph calls itself the future IPC boundary and
// says a `CapValue` can be written out and read back. This is that, and it
// stopped being future the day the agent answering a model call needed to reach
// a capability the broker performs: the request crosses a process boundary and
// comes back, in the same shape it crosses the one inside this process.

use crate::bin::{BinError, Reader, Writer};

type Result<T> = std::result::Result<T, BinError>;

impl CapValue {
    /// Writes a value.
    ///
    /// The tags are the ones a checkpoint already used, and a checkpoint calls
    /// this rather than keeping its own copy: two encodings of one type are two
    /// things that can disagree about it later.
    pub fn write(&self, w: &mut Writer) {
        match self {
            CapValue::Unit => w.u8(0),
            CapValue::Bool(v) => {
                w.u8(1);
                w.bool(*v);
            }
            CapValue::I64(v) => {
                w.u8(2);
                w.i64(*v);
            }
            CapValue::F64(v) => {
                w.u8(3);
                w.f64(*v);
            }
            CapValue::Str(s) => {
                w.u8(4);
                w.str(s);
            }
            CapValue::List(items) => {
                w.u8(5);
                w.u32(items.len() as u32);
                for item in items {
                    w.str(item);
                }
            }
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Result<CapValue> {
        Ok(match r.u8()? {
            0 => CapValue::Unit,
            1 => CapValue::Bool(r.bool()?),
            2 => CapValue::I64(r.i64()?),
            3 => CapValue::F64(r.f64()?),
            4 => CapValue::Str(r.str()?),
            5 => {
                // One byte is the smallest a string can be, which is what
                // stops a claimed length from allocating on a promise.
                let n = r.count(1)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(r.str()?);
                }
                CapValue::List(items)
            }
            other => {
                return Err(BinError::new(format!(
                    "unknown capability value tag {other}"
                )));
            }
        })
    }
}

impl CapRequest {
    pub fn write(&self, w: &mut Writer) {
        w.u32(self.index);
        w.str(&self.name);
        w.u32(self.args.len() as u32);
        for arg in &self.args {
            arg.write(w);
        }
        w.u32(self.task);
        w.u32(self.attempt);
        w.u32(self.timeout_ms);
        w.u32(self.conversation);
    }

    pub fn read(r: &mut Reader<'_>) -> Result<CapRequest> {
        let index = r.u32()?;
        let name = r.str()?;
        let count = r.count(1)?;
        let mut args = Vec::with_capacity(count);
        for _ in 0..count {
            args.push(CapValue::read(r)?);
        }
        Ok(CapRequest {
            index,
            name,
            args,
            task: r.u32()?,
            attempt: r.u32()?,
            timeout_ms: r.u32()?,
            conversation: r.u32()?,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<CapRequest> {
        let mut r = Reader::new(bytes);
        let request = CapRequest::read(&mut r)?;
        r.expect_end("a capability request")?;
        Ok(request)
    }
}

/// What comes back across the boundary: an outcome, or the error that stopped
/// it. `Result` does not travel on its own, so the wire names both.
impl CapOutcome {
    pub fn write(&self, w: &mut Writer) {
        match self {
            CapOutcome::Value(value) => {
                w.u8(0);
                value.write(w);
            }
            CapOutcome::Deferred { question } => {
                w.u8(1);
                w.str(question);
            }
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Result<CapOutcome> {
        Ok(match r.u8()? {
            0 => CapOutcome::Value(CapValue::read(r)?),
            1 => CapOutcome::Deferred { question: r.str()? },
            other => {
                return Err(BinError::new(format!(
                    "unknown capability outcome tag {other}"
                )));
            }
        })
    }
}

/// One answer, as it crosses the boundary: what happened, or why it did not.
pub fn answer_to_bytes(answer: &std::result::Result<CapOutcome, CapError>) -> Vec<u8> {
    let mut w = Writer::new();
    match answer {
        Ok(outcome) => {
            w.u8(0);
            outcome.write(&mut w);
        }
        Err(error) => {
            w.u8(1);
            w.str(&error.message);
        }
    }
    w.finish()
}

pub fn answer_from_bytes(bytes: &[u8]) -> Result<std::result::Result<CapOutcome, CapError>> {
    let mut r = Reader::new(bytes);
    let answer = match r.u8()? {
        0 => Ok(CapOutcome::read(&mut r)?),
        1 => Err(CapError::new(r.str()?)),
        other => return Err(BinError::new(format!("unknown answer tag {other}"))),
    };
    r.expect_end("a capability answer")?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CapRequest {
        CapRequest {
            index: 3,
            name: "process.capture".into(),
            args: vec![
                CapValue::Str("/usr/bin/git".into()),
                CapValue::List(vec!["rev-parse".into(), "HEAD".into()]),
            ],
            task: 2,
            attempt: 1,
            timeout_ms: 500,
            conversation: 7,
        }
    }

    /// The boundary this file opens by describing. A request that crosses it
    /// has to arrive as the request that was sent, field for field - nothing
    /// downstream re-derives any of it, and the broker authorizes against what
    /// it reads.
    #[test]
    fn a_request_survives_the_wire() {
        let bytes = request().to_bytes();
        assert_eq!(CapRequest::from_bytes(&bytes).unwrap(), request());
    }

    /// Every shape a value can take, because a format is checked rather than
    /// assumed - and because the shapes nothing produces today are the ones
    /// that will be produced without anybody rereading this.
    #[test]
    fn every_value_survives_the_wire() {
        for value in [
            CapValue::Unit,
            CapValue::Bool(true),
            CapValue::Bool(false),
            CapValue::I64(i64::MIN),
            CapValue::I64(0),
            CapValue::F64(-0.5),
            CapValue::Str(String::new()),
            CapValue::Str("\u{3053}\u{3093}".into()),
            CapValue::List(Vec::new()),
            CapValue::List(vec![String::new(), "two words".into()]),
        ] {
            let mut w = Writer::new();
            value.write(&mut w);
            let bytes = w.finish();
            let mut r = Reader::new(&bytes);
            assert_eq!(CapValue::read(&mut r).unwrap(), value);
            assert!(r.at_end());
        }
    }

    /// Both halves of what comes back. A failure is not an absence of an
    /// answer: it is an answer, and the far side has to be able to tell them
    /// apart without guessing.
    #[test]
    fn an_answer_survives_the_wire_whichever_it_is() {
        for answer in [
            Ok(CapOutcome::Value(CapValue::I64(-1))),
            Ok(CapOutcome::Deferred {
                question: "[claude] why is it slow?".into(),
            }),
            Err(CapError::new("`/bin/false` exited 1")),
        ] {
            let bytes = answer_to_bytes(&answer);
            assert_eq!(answer_from_bytes(&bytes).unwrap(), answer);
        }
    }

    /// Bytes from the other side of a boundary are not trusted to be ours.
    #[test]
    fn nonsense_on_the_wire_is_refused_rather_than_guessed() {
        assert!(CapRequest::from_bytes(&[]).is_err());
        assert!(answer_from_bytes(&[9]).is_err());
        // A tag that is not a value.
        let mut w = Writer::new();
        w.u8(0);
        w.u8(200);
        assert!(answer_from_bytes(&w.finish()).is_err());
        // Trailing bytes: a reader that stopped early read a different message
        // from the one that was sent.
        let mut bytes = request().to_bytes();
        bytes.push(0);
        assert!(CapRequest::from_bytes(&bytes).is_err());
    }
}
