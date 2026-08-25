//! Reading a journal back.
//!
//! Phase 4 said a reader belonged with replay and did not write one. Exporting
//! needs it, so it is here.
//!
//! A journal is append-only and a run can be killed mid-write, so the last line
//! may be a fragment. An unparseable line is skipped and counted rather than
//! failing the whole read: refusing to look at a run because its last line is
//! half-written would refuse exactly the runs worth looking at.

use sic_core::Digest;
use sic_json::Json;

use crate::{Event, EventKind, LogLevel, RunId, SpanId, TaskId};

/// An event and the wall clock the sink stamped it with.
///
/// The timestamp is not part of an `Event`: the journal reads no clock, and
/// `seq` is what orders a run. It is carried alongside because an exporter
/// needs it to give a span a duration.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedEvent {
    pub event: Event,
    pub ts_nanos: Option<u128>,
}

/// What reading a journal produced.
#[derive(Debug, Default)]
pub struct ReadResult {
    pub events: Vec<TimedEvent>,
    /// Lines that were not events, with the reason. Kept so a caller can say
    /// how much it did not understand.
    pub skipped: Vec<String>,
}

/// Reads a journal in JSON Lines form.
pub fn read_jsonl(text: &str) -> ReadResult {
    let mut result = ReadResult::default();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match sic_json::parse(line).map_err(|e| e.to_string()).and_then(
            |json| match event_from_json(&json) {
                Some(event) => Ok(TimedEvent {
                    event,
                    ts_nanos: timestamp_of(&json),
                }),
                None => Err("not a journal event".to_string()),
            },
        ) {
            Ok(event) => result.events.push(event),
            Err(reason) => result
                .skipped
                .push(format!("line {}: {reason}", number + 1)),
        }
    }
    result
}

/// The wall clock a sink wrote, if it wrote one.
pub fn timestamp_of(json: &Json) -> Option<u128> {
    match json.member("ts")? {
        Json::Int(v) => u128::try_from(*v).ok(),
        _ => None,
    }
}

/// Rebuilds an event from one line's JSON.
pub fn event_from_json(json: &Json) -> Option<Event> {
    let seq = int(json, "seq")? as u64;
    let run = RunId(u128::from_str_radix(string(json, "run")?, 16).ok()?);
    let task = TaskId(int(json, "task")? as u64);
    let span = SpanId(int(json, "span")? as u64);
    let parent = match json.member("parent") {
        Some(Json::Int(v)) => Some(SpanId(*v as u64)),
        _ => None,
    };
    let kind = kind_from_json(json)?;
    Some(Event {
        seq,
        run,
        task,
        span,
        parent,
        kind,
    })
}

fn kind_from_json(json: &Json) -> Option<EventKind> {
    let name = string(json, "event")?;
    Some(match name {
        "run_started" => EventKind::RunStarted {
            workflow: string(json, "workflow")?.to_string(),
            args: digest(json, "args")?,
        },
        "run_completed" => EventKind::RunCompleted {
            result: digest(json, "result")?,
        },
        "run_failed" => EventKind::RunFailed {
            error: string(json, "error")?.to_string(),
        },
        "function_entered" => EventKind::FunctionEntered {
            func: string(json, "func")?.to_string(),
        },
        "function_exited" => EventKind::FunctionExited {
            func: string(json, "func")?.to_string(),
        },
        "capability_requested" => EventKind::CapabilityRequested {
            cap: string(json, "cap")?.to_string(),
            args: digest(json, "args")?,
            attempt: int(json, "attempt")? as u32,
        },
        "capability_completed" => EventKind::CapabilityCompleted {
            cap: string(json, "cap")?.to_string(),
            result: digest(json, "result")?,
            attempt: int(json, "attempt")? as u32,
        },
        "capability_failed" => EventKind::CapabilityFailed {
            cap: string(json, "cap")?.to_string(),
            error: string(json, "error")?.to_string(),
            attempt: int(json, "attempt")? as u32,
        },
        "task_started" => EventKind::TaskStarted {
            func: string(json, "func")?.to_string(),
        },
        "task_completed" => EventKind::TaskCompleted {
            result: digest(json, "result")?,
        },
        "task_failed" => EventKind::TaskFailed {
            error: string(json, "error")?.to_string(),
        },
        "task_abandoned" => EventKind::TaskAbandoned,
        "run_suspended" => EventKind::RunSuspended {
            cap: string(json, "cap")?.to_string(),
        },
        "run_resumed" => EventKind::RunResumed {
            cap: string(json, "cap")?.to_string(),
        },
        "checkpoint_written" => EventKind::CheckpointWritten {
            digest: digest(json, "checkpoint")?,
            bytes: int(json, "bytes")? as u64,
        },
        // A journal line holds the digest, so what comes back is a line that
        // says one was logged and at what level. The text is in the run's
        // values file; `sic explain` reads it from there, the way it reads
        // what a person was asked.
        "logged" => EventKind::Logged {
            level: match string(json, "level")? {
                "debug" => LogLevel::Debug,
                "info" => LogLevel::Info,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                _ => return None,
            },
            message: string(json, "message")?.to_string(),
        },
        "tool_used" => EventKind::ToolUsed {
            tool: string(json, "tool")?.to_string(),
            input: digest(json, "input")?,
            allowed: boolean(json, "allowed")?,
            reason: string(json, "reason").unwrap_or("").to_string(),
        },
        "budget_consumed" => EventKind::BudgetConsumed {
            kind: string(json, "budget")?.to_string(),
            amount: int(json, "amount")? as u64,
            remaining: int(json, "remaining")? as u64,
        },
        _ => return None,
    })
}

fn string<'a>(json: &'a Json, name: &str) -> Option<&'a str> {
    match json.member(name)? {
        Json::Str(s) => Some(s),
        _ => None,
    }
}

fn boolean(json: &Json, name: &str) -> Option<bool> {
    match json.member(name)? {
        Json::Bool(v) => Some(*v),
        _ => None,
    }
}

fn int(json: &Json, name: &str) -> Option<i64> {
    match json.member(name)? {
        Json::Int(v) => Some(*v),
        _ => None,
    }
}

/// A digest as written: `sha256:` followed by 64 hex characters.
fn digest(json: &Json, name: &str) -> Option<Digest> {
    Digest::parse(string(json, name)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::event_to_json;

    fn round_trip(kind: EventKind) {
        let event = Event {
            seq: 7,
            run: RunId(0xdead_beef),
            task: TaskId(2),
            span: SpanId(3),
            parent: Some(SpanId(1)),
            kind,
        };
        let line = event_to_json(&event);
        let result = read_jsonl(&line);
        assert!(result.skipped.is_empty(), "{:?}", result.skipped);
        assert_eq!(result.events[0].event, event);
    }

    #[test]
    fn every_event_kind_survives_a_round_trip() {
        let d = Digest::of(b"x");
        round_trip(EventKind::RunStarted {
            workflow: "main".into(),
            args: d,
        });
        round_trip(EventKind::RunCompleted { result: d });
        round_trip(EventKind::RunFailed {
            error: "boom".into(),
        });
        round_trip(EventKind::FunctionEntered {
            func: "main".into(),
        });
        round_trip(EventKind::FunctionExited {
            func: "main".into(),
        });
        round_trip(EventKind::CapabilityRequested {
            cap: "fs.read".into(),
            args: d,
            attempt: 2,
        });
        round_trip(EventKind::CapabilityCompleted {
            cap: "fs.read".into(),
            result: d,
            attempt: 1,
        });
        round_trip(EventKind::CapabilityFailed {
            cap: "fs.read".into(),
            error: "no".into(),
            attempt: 3,
        });
        round_trip(EventKind::TaskStarted {
            func: "work".into(),
        });
        round_trip(EventKind::TaskCompleted { result: d });
        round_trip(EventKind::TaskFailed { error: "no".into() });
        round_trip(EventKind::TaskAbandoned);
        round_trip(EventKind::RunSuspended {
            cap: "llm.invoke".into(),
        });
        round_trip(EventKind::RunResumed {
            cap: "llm.invoke".into(),
        });
        round_trip(EventKind::CheckpointWritten {
            digest: d,
            bytes: 42,
        });
        round_trip(EventKind::BudgetConsumed {
            kind: "calls".into(),
            amount: 1,
            remaining: 3,
        });
    }

    #[test]
    fn a_truncated_last_line_is_skipped_and_counted() {
        // A run killed mid-write leaves a fragment, and refusing to read the
        // rest because of it would refuse the runs worth looking at.
        let good = event_to_json(&Event {
            seq: 0,
            run: RunId(1),
            task: TaskId(0),
            span: SpanId(0),
            parent: None,
            kind: EventKind::TaskAbandoned,
        });
        let text = format!("{good}\n{{\"seq\": 1, \"ru");
        let result = read_jsonl(&text);
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.skipped.len(), 1);
        assert!(
            result.skipped[0].starts_with("line 2:"),
            "{:?}",
            result.skipped
        );
    }

    #[test]
    fn a_line_that_is_not_an_event_is_skipped() {
        let result = read_jsonl("{\"hello\": 1}\n[]\n");
        assert!(result.events.is_empty());
        assert_eq!(result.skipped.len(), 2);
    }

    #[test]
    fn the_timestamp_a_sink_added_is_readable() {
        let json = sic_json::parse("{\"ts\":123,\"seq\":0}").unwrap();
        assert_eq!(timestamp_of(&json), Some(123));
        let json = sic_json::parse("{\"seq\":0}").unwrap();
        assert_eq!(timestamp_of(&json), None);
    }
}
