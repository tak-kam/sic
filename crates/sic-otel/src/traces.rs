//! Journal events into OTLP spans.
//!
//! The events already carry a span and a parent, recorded as they happened, so
//! a trace is a matter of pairing starts with ends rather than reconstructing a
//! tree afterwards.

use std::collections::HashMap;

use sic_journal::{EventKind, SpanId, TimedEvent};

use crate::json::Value;
use crate::{Resource, SCOPE_NAME, attr};

/// OTLP span kinds. A capability call leaves the process, so it is a client.
const KIND_INTERNAL: i64 = 1;
const KIND_CLIENT: i64 = 3;

const STATUS_UNSET: i64 = 0;
const STATUS_OK: i64 = 1;
const STATUS_ERROR: i64 = 2;

#[derive(Debug)]
struct SpanBuilder {
    span: SpanId,
    parent: Option<SpanId>,
    name: String,
    kind: i64,
    start: u128,
    end: Option<u128>,
    status: i64,
    status_message: Option<String>,
    attributes: Vec<(String, Value)>,
}

/// Renders a journal as an OTLP traces document.
pub fn traces(events: &[TimedEvent], resource: &Resource) -> String {
    let mut open: HashMap<SpanId, SpanBuilder> = HashMap::new();
    // Insertion order, so the output is stable and readable.
    let mut order: Vec<SpanId> = Vec::new();
    // Budget charges waiting for the span they belong to, which the request
    // after them opens.
    let mut charged: Vec<(SpanId, i64)> = Vec::new();
    let mut finished: Vec<SpanBuilder> = Vec::new();
    let mut trace_id = 0u128;
    let mut last_ts = 0u128;

    for timed in events {
        let event = &timed.event;
        trace_id = event.run.0;
        let ts = timed.ts_nanos.unwrap_or(0);
        last_ts = last_ts.max(ts);

        match &event.kind {
            EventKind::RunStarted { workflow, args } => {
                let mut span = start(
                    event.span,
                    event.parent,
                    workflow.clone(),
                    KIND_INTERNAL,
                    ts,
                );
                span.attributes
                    .push((attr::WORKFLOW.into(), Value::str(workflow.clone())));
                span.attributes
                    .push((attr::ARGS_DIGEST.into(), Value::str(args.to_string())));
                push(&mut open, &mut order, span);
            }
            EventKind::TaskStarted { func } => {
                let span = start(
                    event.span,
                    event.parent,
                    format!("task {func}"),
                    KIND_INTERNAL,
                    ts,
                );
                push(&mut open, &mut order, span);
            }
            EventKind::FunctionEntered { func } => {
                let mut span = start(event.span, event.parent, func.clone(), KIND_INTERNAL, ts);
                span.attributes
                    .push((attr::FUNCTION.into(), Value::str(func.clone())));
                push(&mut open, &mut order, span);
            }
            EventKind::CapabilityRequested { cap, args, attempt } => {
                let mut span = start(event.span, event.parent, cap.clone(), KIND_CLIENT, ts);
                span.attributes
                    .push((attr::CAPABILITY.into(), Value::str(cap.clone())));
                span.attributes
                    .push((attr::ATTEMPT.into(), Value::Int(*attempt as i64)));
                span.attributes
                    .push((attr::ARGS_DIGEST.into(), Value::str(args.to_string())));
                // A model call also follows the GenAI conventions. The model
                // name is the grant's constraint, which the journal does not
                // record, so it is omitted rather than guessed.
                if cap == "llm.invoke" {
                    span.attributes
                        .push((attr::GEN_AI_SYSTEM.into(), Value::str("sic")));
                    span.attributes
                        .push((attr::GEN_AI_OPERATION.into(), Value::str("invoke")));
                }
                // The charge is recorded before the request, because a call the
                // budget refuses must not leave a request behind it. So the
                // attribute is held until its span exists.
                if let Some(at) = charged
                    .iter()
                    .position(|(span_id, _)| *span_id == event.span)
                {
                    let (_, remaining) = charged.remove(at);
                    span.attributes
                        .push((attr::BUDGET_REMAINING.into(), Value::Int(remaining)));
                }
                push(&mut open, &mut order, span);
            }

            EventKind::RunCompleted { result } | EventKind::TaskCompleted { result } => {
                close(
                    &mut open,
                    &mut finished,
                    event.span,
                    ts,
                    STATUS_OK,
                    None,
                    |s| {
                        s.attributes
                            .push((attr::RESULT_DIGEST.into(), Value::str(result.to_string())));
                    },
                );
            }
            EventKind::CapabilityCompleted { result, .. } => {
                close(
                    &mut open,
                    &mut finished,
                    event.span,
                    ts,
                    STATUS_OK,
                    None,
                    |s| {
                        s.attributes
                            .push((attr::RESULT_DIGEST.into(), Value::str(result.to_string())));
                    },
                );
            }
            EventKind::FunctionExited { .. } => {
                close(
                    &mut open,
                    &mut finished,
                    event.span,
                    ts,
                    STATUS_OK,
                    None,
                    |_| {},
                );
            }
            EventKind::RunFailed { error }
            | EventKind::TaskFailed { error }
            | EventKind::CapabilityFailed { error, .. } => {
                close(
                    &mut open,
                    &mut finished,
                    event.span,
                    ts,
                    STATUS_ERROR,
                    Some(error.clone()),
                    |_| {},
                );
            }
            EventKind::TaskAbandoned => {
                close(
                    &mut open,
                    &mut finished,
                    event.span,
                    ts,
                    STATUS_ERROR,
                    Some("the run ended while this task was still going".into()),
                    |_| {},
                );
            }
            // A budget being spent belongs to the call that spent it, and the
            // event carries that call's span. It opens nothing: it is a fact
            // about a call that is about to happen, not a thing other things
            // happen inside - so it waits for the request that opens the span.
            EventKind::BudgetConsumed { remaining, .. } => {
                charged.push((event.span, *remaining as i64));
            }
            // These do not open or close a span, and do not belong on one. A
            // suspension and a checkpoint are how a run was carried out rather
            // than what the program did - the program did not ask for either -
            // and a tool use is the agent's, recorded beside the call it
            // happened inside rather than as an attribute of it.
            EventKind::RunSuspended { .. }
            | EventKind::RunResumed { .. }
            | EventKind::CheckpointWritten { .. }
            | EventKind::ToolUsed { .. } => {}
            // A log line is a log record, and this converts a journal into
            // traces and metrics. OTLP has a third signal for these and it is
            // its own document with its own shape; putting the text on a span
            // instead would be the wrong signal chosen because it was nearer.
            // See `docs/design/logging.md`.
            EventKind::Logged { .. } => {}
        }
    }

    // A charge whose call never left the VM. Nothing to attach it to, and
    // nothing lost: the run failed, and the failure says why.
    drop(charged);

    // A span that never closed - the run was killed - is exported with the last
    // timestamp seen and a status saying so. Dropping it would hide exactly the
    // runs worth looking at.
    for span_id in &order {
        if let Some(mut span) = open.remove(span_id) {
            span.end = Some(last_ts.max(span.start));
            span.status = STATUS_ERROR;
            span.status_message = Some("the span never finished".into());
            finished.push(span);
        }
    }
    finished.sort_by_key(|s| s.start);

    let spans: Vec<Value> = finished
        .iter()
        .map(|s| render(s, trace_id, events))
        .collect();

    Value::object(vec![(
        "resourceSpans",
        Value::Array(vec![Value::object(vec![
            ("resource", render_resource(resource)),
            (
                "scopeSpans",
                Value::Array(vec![Value::object(vec![
                    (
                        "scope",
                        Value::object(vec![
                            ("name", Value::str(SCOPE_NAME)),
                            ("version", Value::str(resource.service_version.clone())),
                        ]),
                    ),
                    ("spans", Value::Array(spans)),
                ])]),
            ),
        ])]),
    )])
    .to_json()
}

fn start(span: SpanId, parent: Option<SpanId>, name: String, kind: i64, ts: u128) -> SpanBuilder {
    SpanBuilder {
        span,
        parent,
        name,
        kind,
        start: ts,
        end: None,
        status: STATUS_UNSET,
        status_message: None,
        attributes: Vec::new(),
    }
}

fn push(open: &mut HashMap<SpanId, SpanBuilder>, order: &mut Vec<SpanId>, span: SpanBuilder) {
    order.push(span.span);
    open.insert(span.span, span);
}

fn close(
    open: &mut HashMap<SpanId, SpanBuilder>,
    finished: &mut Vec<SpanBuilder>,
    span_id: SpanId,
    ts: u128,
    status: i64,
    message: Option<String>,
    decorate: impl FnOnce(&mut SpanBuilder),
) {
    let Some(mut span) = open.remove(&span_id) else {
        return;
    };
    span.end = Some(ts.max(span.start));
    span.status = status;
    span.status_message = message;
    decorate(&mut span);
    finished.push(span);
}

fn render(span: &SpanBuilder, trace_id: u128, events: &[TimedEvent]) -> Value {
    let task = events
        .iter()
        .find(|e| e.event.span == span.span)
        .map(|e| e.event.task.0)
        .unwrap_or(0);

    let mut attributes = vec![
        Value::attribute(attr::RUN_ID, Value::str(format!("{trace_id:032x}"))),
        Value::attribute(attr::TASK_ID, Value::Int(task as i64)),
    ];
    for (key, value) in &span.attributes {
        attributes.push(Value::attribute(key, value.clone()));
    }

    let mut status = vec![("code", Value::Int(span.status))];
    if let Some(message) = &span.status_message {
        status.push(("message", Value::str(message.clone())));
    }

    let mut fields = vec![
        ("traceId", Value::str(format!("{trace_id:032x}"))),
        ("spanId", Value::str(span_id_hex(span.span))),
        ("name", Value::str(span.name.clone())),
        ("kind", Value::Int(span.kind)),
        // OTLP writes 64-bit times as strings: a JSON number is a double and
        // would lose the low bits of a nanosecond timestamp.
        ("startTimeUnixNano", Value::str(span.start.to_string())),
        (
            "endTimeUnixNano",
            Value::str(span.end.unwrap_or(span.start).to_string()),
        ),
        ("attributes", Value::Array(attributes)),
        ("status", Value::object(status)),
    ];
    if let Some(parent) = span.parent {
        fields.insert(2, ("parentSpanId", Value::str(span_id_hex(parent))));
    }
    Value::object(fields)
}

/// A span id as OTLP wants it: sixteen hex characters, never all zero.
fn span_id_hex(span: SpanId) -> String {
    format!("{:016x}", span.0 + 1)
}

fn render_resource(resource: &Resource) -> Value {
    Value::object(vec![(
        "attributes",
        Value::Array(vec![
            Value::attribute("service.name", Value::str(resource.service_name.clone())),
            Value::attribute(
                "service.version",
                Value::str(resource.service_version.clone()),
            ),
        ]),
    )])
}
