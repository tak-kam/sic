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

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    RunStarted {
        workflow: String,
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

    /// A budgeted call site was used once. `remaining` is what is left, which
    /// is what makes a budget visible before it runs out rather than after.
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
            EventKind::TaskStarted { .. } => "task_started",
            EventKind::TaskCompleted { .. } => "task_completed",
            EventKind::TaskFailed { .. } => "task_failed",
            EventKind::TaskAbandoned => "task_abandoned",
            EventKind::BudgetConsumed { .. } => "budget_consumed",
            EventKind::RunSuspended { .. } => "run_suspended",
            EventKind::RunResumed { .. } => "run_resumed",
            EventKind::CheckpointWritten { .. } => "checkpoint_written",
        }
    }
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
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests;
