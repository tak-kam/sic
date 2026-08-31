//! Journal events into OTLP metrics.
//!
//! These are counts the runtime produces on its own. A program adds no
//! instrumentation to be measured, which is the point of section 25: what can
//! be counted is what the runtime already records.
//!
//! One journal gives one data point per counter. Aggregating across runs is
//! what a collector is for.

use std::collections::BTreeMap;

use sic_journal::{EventKind, TimedEvent};

use crate::json::Value;
use crate::{Resource, SCOPE_NAME, attr};

/// OTLP aggregation temporality: these counts are for one run, so they are
/// deltas rather than a running total.
const DELTA: i64 = 1;

/// A counter, split by the attributes worth splitting it by.
#[derive(Default)]
struct Counter {
    /// Keyed by the capability or workflow name, or empty for no split.
    points: BTreeMap<String, u64>,
}

impl Counter {
    fn add(&mut self, key: &str, amount: u64) {
        *self.points.entry(key.to_string()).or_insert(0) += amount;
    }
}

/// Where a call's time is put, in milliseconds.
///
/// Two sets rather than one, because the two histograms below measure things
/// four orders of magnitude apart: a `fs.read` that took 40ms and a
/// `human.approve` answered on Thursday do not belong in one set of buckets,
/// and a set wide enough for both tells you nothing about either.
const CALL_BOUNDS: &[f64] = &[1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0];
const WAIT_BOUNDS: &[f64] = &[
    1_000.0,
    10_000.0,
    60_000.0,
    600_000.0,
    3_600_000.0,
    21_600_000.0,
    86_400_000.0,
];

/// How long calls took, split by capability.
#[derive(Default)]
struct Durations {
    /// Keyed by the capability's name.
    points: BTreeMap<String, Vec<f64>>,
}

impl Durations {
    fn add(&mut self, cap: &str, millis: f64) {
        self.points.entry(cap.to_string()).or_default().push(millis);
    }

    fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// Renders a journal as an OTLP metrics document.
pub fn metrics(events: &[TimedEvent], resource: &Resource) -> String {
    let mut runs = Counter::default();
    let mut run_failures = Counter::default();
    let mut capability_calls = Counter::default();
    let mut capability_failures = Counter::default();
    // Kept apart from the failures on purpose: a model that answers in the
    // wrong shape is not a broker that could not answer, and an operator
    // watching one of those wants the other left out of the line.
    let mut answers_rejected = Counter::default();
    let mut tasks_started = Counter::default();
    let mut tasks_failed = Counter::default();
    let mut agent_invocations = Counter::default();
    let mut checkpoints = Counter::default();

    // The distance between a call's two events, which is a measurement the
    // sink already made: it stamped both. `docs/design/observability.md` §5
    // used to say no duration was measured and then say where the number was,
    // three words later.
    let mut answered = Durations::default();
    let mut waited = Durations::default();
    // Calls that have been requested and not yet completed, and whether the
    // run stopped while they were open.
    let mut open: Vec<(sic_journal::SpanId, String, u128, bool)> = Vec::new();

    let mut workflow = String::new();
    let mut start = None;
    let mut end = 0u128;

    for timed in events {
        let ts = timed.ts_nanos.unwrap_or(0);
        if start.is_none() {
            start = Some(ts);
        }
        end = end.max(ts);

        match &timed.event.kind {
            EventKind::RunStarted { workflow: name, .. } => {
                workflow = name.clone();
                runs.add(name, 1);
            }
            EventKind::RunFailed { .. } => run_failures.add(&workflow, 1),
            EventKind::CapabilityRequested { cap, .. } => {
                capability_calls.add(cap, 1);
                open.push((timed.event.span, cap.clone(), ts, false));
                // An agent is a model call at this level; the exporter does not
                // know what an agent is either.
                if cap == "llm.invoke" {
                    agent_invocations.add(cap, 1);
                }
            }
            // The run stopped, so every call open at that moment was waiting
            // across it - which is what puts them in the other histogram.
            EventKind::RunSuspended { .. } => {
                for call in open.iter_mut() {
                    call.3 = true;
                }
            }
            EventKind::CapabilityCompleted { .. } => {
                if let Some(at) = open.iter().position(|c| c.0 == timed.event.span) {
                    let (_, cap, from, deferred) = open.remove(at);
                    let millis = ts.saturating_sub(from) as f64 / 1_000_000.0;
                    match deferred {
                        true => waited.add(&cap, millis),
                        false => answered.add(&cap, millis),
                    }
                }
            }
            // A call that failed took as long as it took, and how long a
            // refusal takes is not what either histogram is for. It is counted
            // above and dropped here.
            EventKind::CapabilityFailed { cap, .. } => {
                capability_failures.add(cap, 1);
                open.retain(|c| c.0 != timed.event.span);
            }
            EventKind::AnswerRejected { cap, .. } => {
                answers_rejected.add(cap, 1);
                open.retain(|c| c.0 != timed.event.span);
            }
            EventKind::TaskStarted { .. } => tasks_started.add(&workflow, 1),
            EventKind::TaskFailed { .. } => tasks_failed.add(&workflow, 1),
            EventKind::CheckpointWritten { .. } => checkpoints.add(&workflow, 1),
            _ => {}
        }
    }

    let start = start.unwrap_or(0);
    let mut all = vec![
        sum(
            "sic.workflow.runs",
            "{run}",
            &runs,
            attr::WORKFLOW,
            start,
            end,
        ),
        sum(
            "sic.workflow.failures",
            "{run}",
            &run_failures,
            attr::WORKFLOW,
            start,
            end,
        ),
        sum(
            "sic.capability.calls",
            "{call}",
            &capability_calls,
            attr::CAPABILITY,
            start,
            end,
        ),
        sum(
            "sic.capability.failures",
            "{call}",
            &capability_failures,
            attr::CAPABILITY,
            start,
            end,
        ),
        sum(
            "sic.capability.answers_rejected",
            "{answer}",
            &answers_rejected,
            attr::CAPABILITY,
            start,
            end,
        ),
        sum(
            "sic.task.started",
            "{task}",
            &tasks_started,
            attr::WORKFLOW,
            start,
            end,
        ),
        sum(
            "sic.task.failed",
            "{task}",
            &tasks_failed,
            attr::WORKFLOW,
            start,
            end,
        ),
        sum(
            "sic.agent.invocations",
            "{call}",
            &agent_invocations,
            attr::CAPABILITY,
            start,
            end,
        ),
        sum(
            "sic.checkpoints.written",
            "{checkpoint}",
            &checkpoints,
            attr::WORKFLOW,
            start,
            end,
        ),
    ];
    all.push(histogram(
        "sic.capability.duration",
        &answered,
        CALL_BOUNDS,
        start,
        end,
    ));
    all.push(histogram(
        "sic.capability.deferred",
        &waited,
        WAIT_BOUNDS,
        start,
        end,
    ));

    // A counter nothing incremented says nothing; leaving it out keeps the
    // document about what happened.
    all.retain(|metric| metric.is_some());
    let all: Vec<Value> = all.into_iter().flatten().collect();

    Value::object(vec![(
        "resourceMetrics",
        Value::Array(vec![Value::object(vec![
            ("resource", render_resource(resource)),
            (
                "scopeMetrics",
                Value::Array(vec![Value::object(vec![
                    (
                        "scope",
                        Value::object(vec![
                            ("name", Value::str(SCOPE_NAME)),
                            ("version", Value::str(resource.service_version.clone())),
                        ]),
                    ),
                    ("metrics", Value::Array(all)),
                ])]),
            ),
        ])]),
    )])
    .to_json()
}

fn sum(
    name: &str,
    unit: &str,
    counter: &Counter,
    attribute: &str,
    start: u128,
    end: u128,
) -> Option<Value> {
    if counter.points.is_empty() {
        return None;
    }
    let points: Vec<Value> = counter
        .points
        .iter()
        .map(|(key, count)| {
            let mut attributes = Vec::new();
            if !key.is_empty() {
                attributes.push(Value::attribute(attribute, Value::str(key.clone())));
            }
            Value::object(vec![
                ("attributes", Value::Array(attributes)),
                ("startTimeUnixNano", Value::str(start.to_string())),
                ("timeUnixNano", Value::str(end.to_string())),
                ("asInt", Value::str(count.to_string())),
            ])
        })
        .collect();

    Some(Value::object(vec![
        ("name", Value::str(name)),
        ("unit", Value::str(unit)),
        (
            "sum",
            Value::object(vec![
                ("dataPoints", Value::Array(points)),
                ("aggregationTemporality", Value::Int(DELTA)),
                ("isMonotonic", Value::Bool(true)),
            ]),
        ),
    ]))
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

/// How long calls took, as an OTLP histogram.
///
/// The numbers come from the two timestamps a sink wrote around each call, so
/// nothing had to be measured that was not already: see
/// `docs/design/observability.md` §5.
///
/// `min` and `max` are sent as well as the buckets. A run produces a handful of
/// calls rather than a stream, and with counts that small the buckets say
/// almost nothing while the extremes say most of what there is.
fn histogram(
    name: &str,
    durations: &Durations,
    bounds: &[f64],
    start: u128,
    end: u128,
) -> Option<Value> {
    if durations.is_empty() {
        return None;
    }
    let points: Vec<Value> = durations
        .points
        .iter()
        .map(|(cap, millis)| {
            let mut counts = vec![0u64; bounds.len() + 1];
            for value in millis {
                let bucket = bounds
                    .iter()
                    .position(|b| value <= b)
                    .unwrap_or(bounds.len());
                counts[bucket] += 1;
            }
            let sum: f64 = millis.iter().sum();
            let low = millis.iter().copied().fold(f64::INFINITY, f64::min);
            let high = millis.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            Value::object(vec![
                (
                    "attributes",
                    Value::Array(vec![Value::attribute(
                        attr::CAPABILITY,
                        Value::str(cap.clone()),
                    )]),
                ),
                ("startTimeUnixNano", Value::str(start.to_string())),
                ("timeUnixNano", Value::str(end.to_string())),
                ("count", Value::str(millis.len().to_string())),
                ("sum", Value::str(format!("{sum}"))),
                ("min", Value::str(format!("{low}"))),
                ("max", Value::str(format!("{high}"))),
                (
                    "bucketCounts",
                    Value::Array(counts.iter().map(|c| Value::str(c.to_string())).collect()),
                ),
                (
                    "explicitBounds",
                    Value::Array(bounds.iter().map(|b| Value::str(format!("{b}"))).collect()),
                ),
            ])
        })
        .collect();

    Some(Value::object(vec![
        ("name", Value::str(name)),
        ("unit", Value::str("ms")),
        (
            "histogram",
            Value::object(vec![
                ("dataPoints", Value::Array(points)),
                ("aggregationTemporality", Value::Int(DELTA)),
            ]),
        ),
    ]))
}
