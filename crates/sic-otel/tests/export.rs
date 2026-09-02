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
                program: Digest::of(b"bytecode"),
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
                program: Digest::of(b"bytecode"),
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

/// The distance between a call's two events, which a sink already stamped.
/// Nothing new is recorded; the number is read.
#[test]
fn a_call_answered_within_the_call_is_timed_from_its_own_two_events() {
    let json = metrics(&simple_run(), &Resource::default());
    assert!(json.contains("sic.capability.duration"), "{json}");
    assert!(json.contains("\"unit\":\"ms\""), "{json}");
    // 130ns to 140ns in `simple_run`, which is 0.00001ms - small, real, and
    // in the first bucket rather than rounded away.
    assert!(json.contains("\"count\":\"1\""), "{json}");
    // Nothing waited, so the other histogram is left out the way an
    // unincremented counter is.
    assert!(!json.contains("sic.capability.deferred"), "{json}");
}

/// A call answered after a suspension includes the whole wait, and a histogram
/// holding both would have buckets spanning four orders of magnitude.
#[test]
fn a_call_that_waited_is_measured_apart_from_one_that_did_not() {
    let d = Digest::of(b"x");
    let mut events = vec![
        event(
            0,
            0,
            None,
            0,
            EventKind::RunStarted {
                workflow: "main".into(),
                program: Digest::of(b"bytecode"),
                args: d,
            },
        ),
        // A call the broker answered: one millisecond.
        event(
            1,
            1,
            Some(0),
            1_000_000,
            EventKind::CapabilityRequested {
                cap: "fs.read".into(),
                args: d,
                attempt: 1,
            },
        ),
        event(
            2,
            1,
            Some(0),
            2_000_000,
            EventKind::CapabilityCompleted {
                cap: "fs.read".into(),
                result: d,
                attempt: 1,
            },
        ),
        // A call a person answered, two hours later.
        event(
            3,
            2,
            Some(0),
            3_000_000,
            EventKind::CapabilityRequested {
                cap: "human.approve".into(),
                args: d,
                attempt: 1,
            },
        ),
    ];
    events.push(event(
        4,
        0,
        None,
        3_100_000,
        EventKind::RunSuspended {
            cap: "human.approve".into(),
        },
    ));
    events.push(event(
        5,
        0,
        None,
        7_200_003_000_000,
        EventKind::RunResumed {
            cap: "human.approve".into(),
        },
    ));
    events.push(event(
        6,
        2,
        Some(0),
        7_200_003_000_000,
        EventKind::CapabilityCompleted {
            cap: "human.approve".into(),
            result: d,
            attempt: 1,
        },
    ));

    let json = metrics(&events, &Resource::default());
    assert!(json.contains("sic.capability.duration"), "{json}");
    assert!(json.contains("sic.capability.deferred"), "{json}");
    // One millisecond in the first, two hours in the second. If they shared a
    // histogram, neither number would be readable.
    assert!(json.contains("\"max\":\"1\""), "the answered call: {json}");
    assert!(
        json.contains("\"max\":\"7200000\""),
        "the waited call: {json}"
    );
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

/// A budget being spent belongs to the call that spent it.
///
/// `docs/design/observability.md` §3 named `sic.budget.remaining` all along and
/// nothing carried it. It is on the capability span rather than the enclosing
/// function's, because two budgeted sites in one function would otherwise write
/// to one place and a reader could not tell which had spent what.
#[test]
fn a_capability_span_says_what_is_left_of_the_budget() {
    let d = Digest::of(b"x");
    let events = vec![
        event(
            0,
            0,
            None,
            100,
            EventKind::RunStarted {
                workflow: "main".into(),
                program: Digest::of(b"bytecode"),
                args: d,
            },
        ),
        event(
            1,
            2,
            Some(0),
            120,
            EventKind::BudgetConsumed {
                kind: "calls".into(),
                amount: 1,
                remaining: 7,
            },
        ),
        event(
            2,
            2,
            Some(0),
            121,
            EventKind::CapabilityRequested {
                cap: "llm.invoke".into(),
                args: d,
                attempt: 1,
            },
        ),
        event(
            3,
            2,
            Some(0),
            130,
            EventKind::CapabilityCompleted {
                cap: "llm.invoke".into(),
                result: d,
                attempt: 1,
            },
        ),
        event(4, 0, None, 140, EventKind::RunCompleted { result: d }),
    ];

    let doc = traces(&events, &Resource::default());
    // On the span of the call, not on the run's: each span object starts with
    // its own `traceId`, so splitting on that is what separates them.
    let call = doc
        .split("{\"traceId\"")
        .find(|span| span.contains("\"name\":\"llm.invoke\""))
        .expect("a span for the call");
    assert!(call.contains("sic.budget.remaining"), "{call}");
    assert!(call.contains("\"intValue\":7"), "{call}");

    let run = doc
        .split("{\"traceId\"")
        .find(|span| span.contains("\"name\":\"main\""))
        .expect("a span for the run");
    assert!(!run.contains("sic.budget.remaining"), "{run}");
}
