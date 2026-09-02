//! The execution journal.
//!
//! Everything observable about a run comes from here: durability, tracing,
//! metrics, logs, audit and replay are views of one event stream rather than
//! separate mechanisms that have to agree with each other.
//!
//! Two decisions shape the model.
//!
//! **Digests, not values.** An event records the digest of an argument or a
//! result, never the thing itself. Telemetry is an exfiltration path like any
//! other, and a default that copies values into it is a default that leaks
//! secrets. Recording values is possible, but it has to be asked for.
//!
//! **`seq` is the order.** The sequence number is assigned here and is
//! monotonic within a run. A wall clock is an observation a sink may add; it is
//! never what replay depends on, because then a run would stop being
//! reproducible the moment two events shared a timestamp.
//!
//! This crate performs no I/O and reads no clock. Where the events go is the
//! business of a `Sink`.

pub mod json;
pub mod read;
pub mod wire;

pub use read::{ReadResult, TimedEvent, read_jsonl};

use sic_core::{CapValue, Digest, Sha256};

/// Identifies one run. The value is supplied from outside, because generating
/// one means reading something this crate must not touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunId(pub u128);

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Identifies a concurrent task. Everything is task 0 until phase 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

/// Identifies a span: a run, a function activation, a capability call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Monotonic within a run. This, and not a timestamp, is the order.
    pub seq: u64,
    pub run: RunId,
    pub task: TaskId,
    pub span: SpanId,
    /// The span this one happened inside, giving the trace its shape at the
    /// moment it is recorded rather than reconstructing it later.
    pub parent: Option<SpanId>,
    pub kind: EventKind,
}

/// How much a logged line matters, as the program said it.
///
/// Repeated here rather than taken from `sic-ir`: the journal is downstream of
/// the compiler and must not depend on it, which is the same reason `sic-core`
/// holds the capability types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }

    /// The four, by the numbers the bytecode uses.
    pub fn from_code(code: u8) -> Option<LogLevel> {
        Some(match code {
            0 => LogLevel::Debug,
            1 => LogLevel::Info,
            2 => LogLevel::Warn,
            3 => LogLevel::Error,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    /// `program` is the bytecode's digest, and it is what ties this file to a
    /// plan.
    ///
    /// Without it a journal says which function a run entered and what it was
    /// called with, and nothing about which program that function was in - so
    /// two runs of two different programs that both start at `main` and take no
    /// arguments differ in a timestamp and a run id. Every neighbouring
    /// artifact already carries the digest: a checkpoint refuses to restore
    /// against different bytecode, `sic plan` prints it on its second line, and
    /// a recorded run keeps the bytes themselves. The journal is the one a
    /// person is handed on its own, which is the one that most needed to be
    /// able to say what it is a record of.
    RunStarted {
        workflow: String,
        program: Digest,
        args: Digest,
    },
    RunCompleted {
        result: Digest,
    },
    RunFailed {
        error: String,
    },

    FunctionEntered {
        func: String,
    },
    FunctionExited {
        func: String,
    },

    /// `attempt` counts from 1. A retried call records every attempt, so an
    /// audit shows what happened rather than only what finally worked.
    CapabilityRequested {
        cap: String,
        args: Digest,
        attempt: u32,
    },
    CapabilityCompleted {
        cap: String,
        result: Digest,
        attempt: u32,
    },
    CapabilityFailed {
        cap: String,
        error: String,
        attempt: u32,
    },
    /// An answer arrived and did not fit the type the agent declared.
    ///
    /// Not a `CapabilityFailed`: the broker did its job and the model replied,
    /// and an account that called that a failure of the call would be hiding
    /// what a reader of a bad run most needs to see. The digest is the answer,
    /// so a recorded run's values file has the document itself; `error` is what
    /// `FROM_JSON` would have said.
    ///
    /// It is followed by another `CapabilityRequested` when an attempt is left,
    /// and by nothing when there is not - the run ends on the same failure it
    /// would have ended on before any of this existed.
    AnswerRejected {
        cap: String,
        result: Digest,
        error: String,
        attempt: u32,
    },

    TaskStarted {
        func: String,
    },
    TaskCompleted {
        result: Digest,
    },
    TaskFailed {
        error: String,
    },
    /// The run ended while this task was still going.
    TaskAbandoned,

    /// A tool of the agent's own, used while answering a model call.
    ///
    /// Not a capability: the manifest does not name `Bash` or `Edit`, and
    /// recording one as a capability call would be the journal calling
    /// something a capability because there was nowhere else to put it. What
    /// the call was about is a digest, for the reason everything else here is.
    ToolUsed {
        tool: String,
        input: Digest,
        allowed: bool,
        /// Why not, when sic refused it. Empty otherwise.
        reason: String,
    },

    /// A budgeted call site was used once. `remaining` is what is left, which
    /// is what makes a budget visible before it runs out rather than after.
    ///
    /// "Used" means the call happened. A call the budget refused emits nothing:
    /// it is followed by no `CapabilityRequested`, nothing was asked, and an
    /// account that billed for it would be describing work that did not occur.
    BudgetConsumed {
        kind: String,
        amount: u64,
        remaining: u64,
    },

    /// The run stopped because a capability could not answer yet.
    RunSuspended {
        cap: String,
    },
    /// The run picked up again from a checkpoint.
    RunResumed {
        cap: String,
    },
    /// A checkpoint was produced. The digest identifies it; the size is what
    /// makes durable execution's cost visible.
    CheckpointWritten {
        digest: Digest,
        bytes: u64,
    },
    /// What the program said about itself.
    ///
    /// A sink is code the CLI owns and may show a person what the program
    /// said; a journal file is the run's account and holds digests, which is
    /// the split `docs/design/runs.md` §2 already made for what a capability
    /// answered.
    Logged {
        level: LogLevel,
        /// The text as the VM emits it, and the digest when this event has
        /// been read back out of a `journal.jsonl` - because the digest is
        /// what that file holds. Whoever wants the text after the fact reads
        /// the run's values file, which is where `sic explain` gets it.
        message: String,
    },
}

impl EventKind {
    /// The name the event is written under.
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::RunStarted { .. } => "run_started",
            EventKind::RunCompleted { .. } => "run_completed",
            EventKind::RunFailed { .. } => "run_failed",
            EventKind::FunctionEntered { .. } => "function_entered",
            EventKind::FunctionExited { .. } => "function_exited",
            EventKind::CapabilityRequested { .. } => "capability_requested",
            EventKind::CapabilityCompleted { .. } => "capability_completed",
            EventKind::CapabilityFailed { .. } => "capability_failed",
            EventKind::AnswerRejected { .. } => "answer_rejected",
            EventKind::TaskStarted { .. } => "task_started",
            EventKind::TaskCompleted { .. } => "task_completed",
            EventKind::TaskFailed { .. } => "task_failed",
            EventKind::TaskAbandoned => "task_abandoned",
            EventKind::ToolUsed { .. } => "tool_used",
            EventKind::BudgetConsumed { .. } => "budget_consumed",
            EventKind::RunSuspended { .. } => "run_suspended",
            EventKind::RunResumed { .. } => "run_resumed",
            EventKind::CheckpointWritten { .. } => "checkpoint_written",
            EventKind::Logged { .. } => "logged",
        }
    }

    /// The event as a journal file records it.
    ///
    /// Writing a `Logged` event to `journal.jsonl` deliberately loses
    /// something: the message becomes its digest, because a file is the run's
    /// account and an account holds digests. Reading that line back therefore
    /// gives an event whose message *is* the digest, and comparing it with the
    /// event the VM emitted - which holds the text - compares two spellings of
    /// one thing. Anything holding both puts the VM's side in this form first;
    /// `sic replay` is the one that does, and `docs/design/runs.md` §4 says
    /// why it compares logs at all.
    ///
    /// Every other event survives the round trip unchanged, so for all of them
    /// this is the identity. Call it on an event as the VM emitted it: an
    /// entry already read out of a file is in this form and digesting it again
    /// would be digesting a digest.
    pub fn as_recorded(&self) -> std::borrow::Cow<'_, EventKind> {
        match self {
            EventKind::Logged { level, message } => std::borrow::Cow::Owned(EventKind::Logged {
                level: *level,
                message: recorded_message(message),
            }),
            other => std::borrow::Cow::Borrowed(other),
        }
    }
}

/// What a journal line holds in place of a logged message.
///
/// One function rather than two because a writer and a reader that disagreed
/// about this would make every replay of every program that logs report a
/// difference - which is exactly what happened while `json.rs` was the only
/// place that knew.
pub(crate) fn recorded_message(message: &str) -> String {
    Digest::of(message.as_bytes()).to_string()
}

/// Where events go. Implementations that write to a file or a socket live
/// outside this crate, which is what keeps the journal itself effect-free.
pub trait Sink: std::fmt::Debug {
    fn emit(&mut self, event: &Event);
}

/// Drops every event. The default, so that recording is something a run opts
/// into rather than something it has to opt out of.
#[derive(Debug, Default)]
pub struct NullSink;

impl Sink for NullSink {
    fn emit(&mut self, _event: &Event) {}
}

/// Keeps events in memory, for tests and for anything that wants the stream
/// without a file.
#[derive(Debug, Default)]
pub struct MemorySink {
    pub events: Vec<Event>,
}

impl Sink for MemorySink {
    fn emit(&mut self, event: &Event) {
        self.events.push(event.clone());
    }
}

/// Assigns sequence numbers and span ids, and hands events to a sink.
#[derive(Debug)]
pub struct Journal {
    run: RunId,
    seq: u64,
    next_span: u64,
    sink: Box<dyn Sink>,
}

impl Journal {
    pub fn new(run: RunId, sink: Box<dyn Sink>) -> Self {
        Self {
            run,
            seq: 0,
            next_span: 0,
            sink,
        }
    }

    /// A journal that records nothing.
    pub fn discard() -> Self {
        Self::new(RunId(0), Box::new(NullSink))
    }

    /// Continues an existing run's journal after a checkpoint.
    ///
    /// The counters carry over so that the stream stays one sequence across
    /// however many processes the run takes: the whole point of the journal is
    /// that a resumed run is the same run.
    pub fn resumed(run: RunId, seq: u64, next_span: u64, sink: Box<dyn Sink>) -> Self {
        Self {
            run,
            seq,
            next_span,
            sink,
        }
    }

    /// The next sequence number, for writing a checkpoint.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The next span id, for writing a checkpoint.
    pub fn next_span_id(&self) -> u64 {
        self.next_span
    }

    pub fn run_id(&self) -> RunId {
        self.run
    }

    /// How many events have been recorded.
    pub fn count(&self) -> u64 {
        self.seq
    }

    pub fn new_span(&mut self) -> SpanId {
        let id = SpanId(self.next_span);
        self.next_span += 1;
        id
    }

    pub fn emit(&mut self, span: SpanId, parent: Option<SpanId>, kind: EventKind) {
        self.emit_for(TaskId(0), span, parent, kind);
    }

    /// Records an event belonging to a particular task.
    pub fn emit_for(
        &mut self,
        task: TaskId,
        span: SpanId,
        parent: Option<SpanId>,
        kind: EventKind,
    ) {
        let event = Event {
            seq: self.seq,
            run: self.run,
            task,
            span,
            parent,
            kind,
        };
        self.seq += 1;
        self.sink.emit(&event);
    }
}

/// The digest of a sequence of values.
///
/// Each value is hashed with a tag and an explicit length, so that no two
/// different sequences can produce the same input to the hash - `["ab"]` and
/// `["a", "b"]` have to differ.
pub fn digest_values(values: &[CapValue]) -> Digest {
    let mut h = Sha256::new();
    h.update(&(values.len() as u64).to_le_bytes());
    for value in values {
        match value {
            CapValue::Unit => h.update(&[0]),
            CapValue::Bool(v) => {
                h.update(&[1]);
                h.update(&[*v as u8]);
            }
            CapValue::I64(v) => {
                h.update(&[2]);
                h.update(&v.to_le_bytes());
            }
            CapValue::F64(v) => {
                h.update(&[3]);
                h.update(&v.to_bits().to_le_bytes());
            }
            CapValue::Str(s) => {
                h.update(&[4]);
                h.update(&(s.len() as u64).to_le_bytes());
                h.update(s.as_bytes());
            }
            CapValue::List(items) => {
                h.update(&[5]);
                h.update(&(items.len() as u64).to_le_bytes());
                for item in items {
                    h.update(&(item.len() as u64).to_le_bytes());
                    h.update(item.as_bytes());
                }
            }
            CapValue::Exit { code, output } => {
                h.update(&[6]);
                h.update(&code.to_le_bytes());
                h.update(&(output.len() as u64).to_le_bytes());
                h.update(output.as_bytes());
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests;
