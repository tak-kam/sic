//! An event, written out and read back whole.
//!
//! `json.rs` writes a journal file and deliberately loses something: a
//! `Logged` message becomes its digest, because a journal file is the run's
//! account and holds digests. That is right for a file and wrong for a wire.
//!
//! When the VM is a separate process its events reach the sink by crossing
//! one, and a sink is the code that decides what a person sees - so what
//! arrives has to be the event the VM emitted rather than the entry the file
//! would have kept. See `docs/design/processes.md` §3.
//!
//! Binary rather than JSON for the same reason `sic-core`'s wire is: this is
//! read by the process on the other end and never by a person, and a length
//! that is a promise is a length that can be refused.

use sic_core::{BinError, Digest, Reader, Writer};

use crate::{Event, EventKind, LogLevel, RunId, SpanId, TaskId};

type Result<T> = std::result::Result<T, BinError>;

impl Event {
    pub fn write(&self, w: &mut Writer) {
        w.u64(self.seq);
        w.u128(self.run.0);
        w.u64(self.task.0);
        w.u64(self.span.0);
        match self.parent {
            Some(parent) => {
                w.bool(true);
                w.u64(parent.0);
            }
            None => w.bool(false),
        }
        self.kind.write(w);
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Event> {
        Ok(Event {
            seq: r.u64()?,
            run: RunId(r.u128()?),
            task: TaskId(r.u64()?),
            span: SpanId(r.u64()?),
            parent: match r.bool()? {
                true => Some(SpanId(r.u64()?)),
                false => None,
            },
            kind: EventKind::read(r)?,
        })
    }
}

/// A digest is thirty-two bytes and always the same thirty-two.
fn digest(w: &mut Writer, d: &Digest) {
    w.bytes(d.bytes());
}

fn read_digest(r: &mut Reader<'_>) -> Result<Digest> {
    let bytes = r.take(32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(Digest::from_bytes(out))
}

impl EventKind {
    /// The tags are this wire's own and are not the file format's. Nothing
    /// stores them, so appending is the only rule: a reader that meets one it
    /// does not know is talking to a `sic` that is not this one, and saying so
    /// is better than guessing.
    pub fn write(&self, w: &mut Writer) {
        match self {
            EventKind::RunStarted { workflow, args } => {
                w.u8(0);
                w.str(workflow);
                digest(w, args);
            }
            EventKind::RunCompleted { result } => {
                w.u8(1);
                digest(w, result);
            }
            EventKind::RunFailed { error } => {
                w.u8(2);
                w.str(error);
            }
            EventKind::FunctionEntered { func } => {
                w.u8(3);
                w.str(func);
            }
            EventKind::FunctionExited { func } => {
                w.u8(4);
                w.str(func);
            }
            EventKind::CapabilityRequested { cap, args, attempt } => {
                w.u8(5);
                w.str(cap);
                digest(w, args);
                w.u32(*attempt);
            }
            EventKind::CapabilityCompleted {
                cap,
                result,
                attempt,
            } => {
                w.u8(6);
                w.str(cap);
                digest(w, result);
                w.u32(*attempt);
            }
            EventKind::CapabilityFailed {
                cap,
                error,
                attempt,
            } => {
                w.u8(7);
                w.str(cap);
                w.str(error);
                w.u32(*attempt);
            }
            EventKind::TaskStarted { func } => {
                w.u8(8);
                w.str(func);
            }
            EventKind::TaskCompleted { result } => {
                w.u8(9);
                digest(w, result);
            }
            EventKind::TaskFailed { error } => {
                w.u8(10);
                w.str(error);
            }
            EventKind::TaskAbandoned => w.u8(11),
            EventKind::ToolUsed {
                tool,
                input,
                allowed,
                reason,
            } => {
                w.u8(12);
                w.str(tool);
                digest(w, input);
                w.bool(*allowed);
                w.str(reason);
            }
            EventKind::BudgetConsumed {
                kind,
                amount,
                remaining,
            } => {
                w.u8(13);
                w.str(kind);
                w.u64(*amount);
                w.u64(*remaining);
            }
            EventKind::RunSuspended { cap } => {
                w.u8(14);
                w.str(cap);
            }
            EventKind::RunResumed { cap } => {
                w.u8(15);
                w.str(cap);
            }
            EventKind::CheckpointWritten { digest: d, bytes } => {
                w.u8(16);
                digest(w, d);
                w.u64(*bytes);
            }
            // The whole reason this codec exists rather than JSON: the text,
            // not the digest of it.
            EventKind::Logged { level, message } => {
                w.u8(17);
                w.u8(match level {
                    LogLevel::Debug => 0,
                    LogLevel::Info => 1,
                    LogLevel::Warn => 2,
                    LogLevel::Error => 3,
                });
                w.str(message);
            }
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Result<EventKind> {
        Ok(match r.u8()? {
            0 => EventKind::RunStarted {
                workflow: r.str()?,
                args: read_digest(r)?,
            },
            1 => EventKind::RunCompleted {
                result: read_digest(r)?,
            },
            2 => EventKind::RunFailed { error: r.str()? },
            3 => EventKind::FunctionEntered { func: r.str()? },
            4 => EventKind::FunctionExited { func: r.str()? },
            5 => EventKind::CapabilityRequested {
                cap: r.str()?,
                args: read_digest(r)?,
                attempt: r.u32()?,
            },
            6 => EventKind::CapabilityCompleted {
                cap: r.str()?,
                result: read_digest(r)?,
                attempt: r.u32()?,
            },
            7 => EventKind::CapabilityFailed {
                cap: r.str()?,
                error: r.str()?,
                attempt: r.u32()?,
            },
            8 => EventKind::TaskStarted { func: r.str()? },
            9 => EventKind::TaskCompleted {
                result: read_digest(r)?,
            },
            10 => EventKind::TaskFailed { error: r.str()? },
            11 => EventKind::TaskAbandoned,
            12 => EventKind::ToolUsed {
                tool: r.str()?,
                input: read_digest(r)?,
                allowed: r.bool()?,
                reason: r.str()?,
            },
            13 => EventKind::BudgetConsumed {
                kind: r.str()?,
                amount: r.u64()?,
                remaining: r.u64()?,
            },
            14 => EventKind::RunSuspended { cap: r.str()? },
            15 => EventKind::RunResumed { cap: r.str()? },
            16 => EventKind::CheckpointWritten {
                digest: read_digest(r)?,
                bytes: r.u64()?,
            },
            17 => EventKind::Logged {
                level: LogLevel::from_code(r.u8()?)
                    .ok_or_else(|| BinError::new("unknown log level"))?,
                message: r.str()?,
            },
            other => return Err(BinError::new(format!("unknown event tag {other}"))),
        })
    }
}
