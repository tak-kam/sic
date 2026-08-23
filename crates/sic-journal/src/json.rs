//! Writing events as JSON Lines.
//!
//! One event per line, append-only, so a file that is cut short still reads up
//! to the cut. The writer is by hand: serde would be a dependency, and the
//! shape here is small enough that the escaping rules are the whole of it.

use crate::{Event, EventKind};

/// Renders one event as a single line of JSON, without a trailing newline.
pub fn event_to_json(event: &Event) -> String {
    let mut out = String::with_capacity(160);
    out.push('{');
    field_u64(&mut out, "seq", event.seq, true);
    field_str(&mut out, "run", &event.run.to_string(), false);
    field_u64(&mut out, "task", event.task.0, false);
    field_u64(&mut out, "span", event.span.0, false);
    match event.parent {
        Some(parent) => field_u64(&mut out, "parent", parent.0, false),
        None => {
            out.push(',');
            out.push_str("\"parent\":null");
        }
    }
    field_str(&mut out, "event", event.kind.name(), false);

    match &event.kind {
        EventKind::RunStarted { workflow, args } => {
            field_str(&mut out, "workflow", workflow, false);
            field_str(&mut out, "args", &args.to_string(), false);
        }
        EventKind::RunCompleted { result } => {
            field_str(&mut out, "result", &result.to_string(), false);
        }
        EventKind::RunFailed { error } => {
            field_str(&mut out, "error", error, false);
        }
        EventKind::FunctionEntered { func } | EventKind::FunctionExited { func } => {
            field_str(&mut out, "func", func, false);
        }
        EventKind::CapabilityRequested { cap, args, attempt } => {
            field_str(&mut out, "cap", cap, false);
            field_str(&mut out, "args", &args.to_string(), false);
            field_u64(&mut out, "attempt", *attempt as u64, false);
        }
        EventKind::CapabilityCompleted {
            cap,
            result,
            attempt,
        } => {
            field_str(&mut out, "cap", cap, false);
            field_str(&mut out, "result", &result.to_string(), false);
            field_u64(&mut out, "attempt", *attempt as u64, false);
        }
        EventKind::CapabilityFailed {
            cap,
            error,
            attempt,
        } => {
            field_str(&mut out, "cap", cap, false);
            field_str(&mut out, "error", error, false);
            field_u64(&mut out, "attempt", *attempt as u64, false);
        }
        EventKind::TaskStarted { func } => field_str(&mut out, "func", func, false),
        EventKind::TaskCompleted { result } => {
            field_str(&mut out, "result", &result.to_string(), false)
        }
        EventKind::TaskFailed { error } => field_str(&mut out, "error", error, false),
        EventKind::TaskAbandoned => {}
        EventKind::BudgetConsumed {
            kind,
            amount,
            remaining,
        } => {
            field_str(&mut out, "budget", kind, false);
            field_u64(&mut out, "amount", *amount, false);
            field_u64(&mut out, "remaining", *remaining, false);
        }
        EventKind::RunSuspended { cap } | EventKind::RunResumed { cap } => {
            field_str(&mut out, "cap", cap, false);
        }
        EventKind::CheckpointWritten { digest, bytes } => {
            field_str(&mut out, "checkpoint", &digest.to_string(), false);
            field_u64(&mut out, "bytes", *bytes, false);
        }
    }
    out.push('}');
    out
}

fn field_u64(out: &mut String, name: &str, value: u64, first: bool) {
    if !first {
        out.push(',');
    }
    out.push_str(&format!("\"{name}\":{value}"));
}

fn field_str(out: &mut String, name: &str, value: &str, first: bool) {
    if !first {
        out.push(',');
    }
    out.push_str(&format!("\"{name}\":"));
    write_string(out, value);
}

/// Writes a JSON string, escaping what the grammar requires.
fn write_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Every other control character has to be escaped as \u; the rest,
            // including all non-ASCII text, is written as it is.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, EventKind, RunId, SpanId, TaskId};
    use sic_core::Digest;

    fn event(kind: EventKind) -> Event {
        Event {
            seq: 3,
            run: RunId(0x1234),
            task: TaskId(0),
            span: SpanId(2),
            parent: Some(SpanId(1)),
            kind,
        }
    }

    #[test]
    fn renders_a_capability_event() {
        let line = event_to_json(&event(EventKind::CapabilityRequested {
            cap: "fs.read".into(),
            args: Digest::of(b"abc"),
            attempt: 1,
        }));
        assert_eq!(
            line,
            "{\"seq\":3,\"run\":\"00000000000000000000000000001234\",\"task\":0,\"span\":2,\
             \"parent\":1,\"event\":\"capability_requested\",\"cap\":\"fs.read\",\
             \"args\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\"attempt\":1}"
        );
    }

    #[test]
    fn a_root_span_has_a_null_parent() {
        let mut e = event(EventKind::RunCompleted {
            result: Digest::of(b""),
        });
        e.parent = None;
        assert!(
            event_to_json(&e).contains("\"parent\":null"),
            "{}",
            event_to_json(&e)
        );
    }

    #[test]
    fn escapes_what_json_requires() {
        let line = event_to_json(&event(EventKind::RunFailed {
            error: "a \"quote\", a \\, a \n and a \u{1}".into(),
        }));
        assert!(
            line.contains("\"error\":\"a \\\"quote\\\", a \\\\, a \\n and a \\u0001\""),
            "{line}"
        );
    }

    #[test]
    fn non_ascii_text_is_written_as_is() {
        let line = event_to_json(&event(EventKind::FunctionEntered {
            func: "日本語".into(),
        }));
        assert!(line.contains("\"func\":\"日本語\""), "{line}");
    }
}
