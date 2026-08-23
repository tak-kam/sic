//! Exporting a journal.

use sic_core::Digest;
use sic_journal::{Event, EventKind, RunId, SpanId, TaskId, TimedEvent};
use sic_otel::{Resource, metrics, traces};

fn event(seq: u64, span: u64, parent: Option<u64>, ts: u128, kind: EventKind) -> TimedEvent {
    TimedEvent {
        event: Event {
            seq,
            run: RunId(0x1234),
            task: TaskId(0),
            span: SpanId(span),
            parent: parent.map(SpanId),
            kind,
        },
        ts_nanos: Some(ts),
    }
}

/// A run that started, called a capability, and finished.
fn simple_run() -> Vec<TimedEvent> {
    let d = Digest::of(b"x");
    vec![
        event(
            0,
            0,
            None,
            100,
            EventKind::RunStarted {
                workflow: "main".into(),
                args: d,
            },
        ),
        event(
            1,
            1,
            Some(0),
            110,
            EventKind::FunctionEntered {
                func: "main".into(),
            },
        ),
        event(
            2,
            2,
            Some(1),
            120,
            EventKind::CapabilityRequested {
                cap: "fs.read".into(),
                args: d,
                attempt: 1,
            },
        ),
        event(
            3,
            2,
            Some(1),
            180,
            EventKind::CapabilityCompleted {
                cap: "fs.read".into(),
                result: d,
                attempt: 1,
            },
        ),
        event(
            4,
            1,
            Some(0),
            190,
            EventKind::FunctionExited {
                func: "main".into(),
            },
        ),
        event(5, 0, None, 200, EventKind::RunCompleted { result: d }),
    ]
}

#[test]
fn spans_are_paired_and_nested() {
    let json = traces(&simple_run(), &Resource::default());

    // The run span is the root, and the trace id is the run id.
    assert!(
        json.contains("\"traceId\":\"00000000000000000000000000001234\""),
        "{json}"
    );
    // Span ids are the journal's plus one: OTLP reserves all-zero.
    assert!(json.contains("\"spanId\":\"0000000000000001\""), "{json}");
    assert!(
        json.contains("\"parentSpanId\":\"0000000000000001\""),
        "{json}"
    );
    // The capability span leaves the process, so it is a client span.
    assert!(json.contains("\"name\":\"fs.read\",\"kind\":3"), "{json}");
    // Its duration is the pair of timestamps.
    assert!(json.contains("\"startTimeUnixNano\":\"120\""), "{json}");
    assert!(json.contains("\"endTimeUnixNano\":\"180\""), "{json}");
}

#[test]
fn a_span_that_never_closed_is_still_exported() {
    // A run killed mid-write is exactly the run worth looking at.
    let mut events = simple_run();
    events.truncate(3);
    let json = traces(&events, &Resource::default());
    assert!(json.contains("never finished"), "{json}");
    assert!(json.contains("\"code\":2"), "{json}");
}

#[test]
fn a_failure_becomes_an_error_status() {
    let mut events = simple_run();
    events.truncate(3);
    events.push(event(
        3,
        2,
        Some(1),
        180,
        EventKind::CapabilityFailed {
            cap: "fs.read".into(),
            error: "permission denied".into(),
            attempt: 1,
        },
    ));
    let json = traces(&events, &Resource::default());
    assert!(json.contains("permission denied"), "{json}");
}

#[test]
fn digests_stay_digests() {
    // Converting to another format is not a reason to start including values.
    let json = traces(&simple_run(), &Resource::default());
    assert!(json.contains("sha256:"), "{json}");
    assert!(json.contains("sic.args.digest"), "{json}");
}

#[test]
fn a_model_call_carries_the_genai_attributes() {
    let d = Digest::of(b"x");
    let events = vec![
        event(
            0,
            0,
            None,
            100,
            EventKind::RunStarted {
                workflow: "main".into(),
                args: d,
            },
        ),
        event(
            1,
            1,
            Some(0),
            110,
            EventKind::CapabilityRequested {
                cap: "llm.invoke".into(),
                args: d,
                attempt: 1,
            },
        ),
    ];
    let json = traces(&events, &Resource::default());
    assert!(json.contains("gen_ai.system"), "{json}");
    assert!(json.contains("gen_ai.operation.name"), "{json}");
}

#[test]
fn metrics_count_what_the_runtime_already_records() {
    let json = metrics(&simple_run(), &Resource::default());
    assert!(json.contains("sic.workflow.runs"), "{json}");
    assert!(json.contains("sic.capability.calls"), "{json}");
    // A counter nothing incremented is left out.
    assert!(!json.contains("sic.task.failed"), "{json}");
    // Counts are strings, because a JSON number is a double.
    assert!(json.contains("\"asInt\":\"1\""), "{json}");
    // These are deltas for one run; aggregating is a collector's job.
    assert!(json.contains("\"aggregationTemporality\":1"), "{json}");
}

#[test]
fn a_failed_run_counts_once_in_runs_and_once_in_failures() {
    let mut events = simple_run();
    events.pop();
    events.push(event(
        5,
        0,
        None,
        200,
        EventKind::RunFailed {
            error: "boom".into(),
        },
    ));
    let json = metrics(&events, &Resource::default());
    assert!(json.contains("sic.workflow.runs"), "{json}");
    assert!(json.contains("sic.workflow.failures"), "{json}");
}

#[test]
fn a_journal_without_timestamps_still_exports() {
    // `seq` is the order; a timestamp is an observation a sink added.
    let events: Vec<TimedEvent> = simple_run()
        .into_iter()
        .map(|mut e| {
            e.ts_nanos = None;
            e
        })
        .collect();
    let json = traces(&events, &Resource::default());
    assert!(json.contains("\"name\":\"main\""), "{json}");
    assert!(json.contains("\"startTimeUnixNano\":\"0\""), "{json}");
}
