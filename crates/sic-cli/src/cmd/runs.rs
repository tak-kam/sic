//! `sic runs`, `sic explain`, `sic inspect-run` and `sic replay`.
//!
//! None of these run a program except `replay`, and that one answers every
//! capability call from what was recorded rather than asking the broker. A
//! replay that called out would be a second run, with a second set of effects,
//! which is the opposite of what replaying is for.

use std::path::Path;
use std::process::ExitCode;

use sic_core::{CapValue, Digest};
use sic_journal::{EventKind, MemorySink, TimedEvent};
use sic_vm::{DEFAULT_FUEL, Status, Vm};

use super::store;
use super::{EXIT_FAILURE, EXIT_USAGE};

/// `sic runs`: what has been recorded.
pub fn list() -> ExitCode {
    let runs = match store::list() {
        Ok(runs) => runs,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    if runs.is_empty() {
        println!("no recorded runs in {}", store::store_root().display());
        return ExitCode::SUCCESS;
    }

    for dir in runs {
        let Ok(events) = store::read_journal(&dir) else {
            continue;
        };
        let summary = store::summarize(&events);
        let short = &summary.run.to_string()[..8];
        print!(
            "{short}  {:<10}  {:<10}  {} capability call(s)",
            summary.workflow,
            summary.outcome.label(),
            summary.capability_calls
        );
        if let Some(detail) = summary.outcome.detail() {
            print!("  {detail}");
        }
        println!();
    }
    ExitCode::SUCCESS
}

/// `sic explain <id>`: the summary a person reads when something went wrong.
pub fn explain(prefix: &str) -> ExitCode {
    let (dir, events) = match open(prefix) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let summary = store::summarize(&events);

    println!("run {}", summary.run);
    println!("  workflow   {}", summary.workflow);
    print!("  outcome    {}", summary.outcome.label());
    match summary.outcome.detail() {
        Some(detail) => println!("  ({detail})"),
        None => println!(),
    }
    println!("  events     {}", summary.events);
    println!("  stored in  {}", dir.display());
    if dir.join(store::CHECKPOINT).exists() {
        println!("  checkpoint present: `sic resume` can continue this run");
    }

    println!();
    for timed in &events {
        let Some(line) = explain_event(timed) else {
            continue;
        };
        let indent = "  ".repeat(store::depth_of(&timed.event, &events) + 1);
        println!("{indent}{line}");
    }
    ExitCode::SUCCESS
}

/// The one line an event is worth in a summary, or nothing.
fn explain_event(timed: &TimedEvent) -> Option<String> {
    Some(match &timed.event.kind {
        EventKind::TaskStarted { func } => format!("task {func}"),
        EventKind::CapabilityRequested { cap, attempt, .. } => {
            if *attempt > 1 {
                format!("call {cap} (attempt {attempt})")
            } else {
                format!("call {cap}")
            }
        }
        EventKind::CapabilityCompleted { cap, result, .. } => {
            format!("  {cap} answered {}", short(result))
        }
        EventKind::CapabilityFailed { cap, error, .. } => format!("  {cap} failed: {error}"),
        EventKind::RunSuspended { cap } => format!("waiting for {cap}"),
        EventKind::RunResumed { cap } => format!("resumed with {cap}"),
        EventKind::TaskFailed { error } => format!("task failed: {error}"),
        EventKind::TaskAbandoned => "task abandoned".to_string(),
        EventKind::BudgetConsumed { remaining, .. } => {
            format!("  budget: {remaining} left")
        }
        EventKind::RunFailed { error } => format!("failed: {error}"),
        // Function entries and exits are the shape, not the story; they are in
        // `inspect-run`.
        _ => return None,
    })
}

fn short(digest: &Digest) -> String {
    format!("sha256:{}", digest.short())
}

/// `sic inspect-run <id>`: every event, unabridged.
pub fn inspect(prefix: &str) -> ExitCode {
    let (_, events) = match open(prefix) {
        Ok(v) => v,
        Err(code) => return code,
    };
    for timed in &events {
        println!("{}", sic_journal::json::event_to_json(&timed.event));
    }
    ExitCode::SUCCESS
}

/// `sic replay <id>`: run the stored bytecode against the stored answers, and
/// compare the journal it produces with the one that was recorded.
pub fn replay(prefix: &str) -> ExitCode {
    let (dir, recorded) = match open(prefix) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let summary = store::summarize(&recorded);

    let program = match super::load_bytecode(&dir.join(store::PROGRAM).to_string_lossy()) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let answers = match read_responses(&dir) {
        Ok(answers) => answers,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let Some(entry) = program.func_by_name("main") else {
        eprintln!("error: the stored bytecode has no `main`");
        return ExitCode::from(EXIT_FAILURE);
    };

    println!("replaying {} ({})", summary.run, summary.workflow);

    // The sink is shared so the events can be read back after the VM has them.
    let sink = SharedSink::default();
    let journal = sic_journal::Journal::new(summary.run, Box::new(sink.clone()));
    let mut vm = Vm::with_journal(&program, DEFAULT_FUEL, journal);

    let mut status = vm.run(entry, &[]);
    let mut used = 0usize;
    let stopped_early = loop {
        match status {
            Status::Suspended(_) => {
                let Some(answer) = answers.get(used).cloned() else {
                    // The recording stops where the run stopped, or the program
                    // took a different path. Either way, saying so is the
                    // finding.
                    break Some("the recording has no answer for the next call");
                };
                used += 1;
                status = vm.resume(answer);
            }
            _ => break None,
        }
    };

    let replayed = sink.events();
    let differences = compare(&recorded, &replayed);
    for line in &differences {
        println!("  {line}");
    }
    if differences.is_empty() {
        println!("  {} events matched", replayed.len());
    }
    if let Some(reason) = stopped_early {
        println!("  stopped: {reason}");
    }

    if differences.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILURE)
    }
}

/// Compares two journals event by event.
///
/// A difference is a real finding: the VM changed, the compiler changed, or
/// something in the run was not as deterministic as it claimed to be.
fn compare(recorded: &[TimedEvent], replayed: &[sic_journal::Event]) -> Vec<String> {
    let mut differences = Vec::new();
    for (i, replayed_event) in replayed.iter().enumerate() {
        let Some(original) = recorded.get(i) else {
            differences.push(format!(
                "seq {i}: the replay produced {} which the recording does not have",
                replayed_event.kind.name()
            ));
            break;
        };
        if original.event.kind != replayed_event.kind {
            differences.push(format!(
                "seq {i}: recorded {}, replayed {}",
                describe(&original.event.kind),
                describe(&replayed_event.kind)
            ));
            break;
        }
    }
    if replayed.len() < recorded.len() && differences.is_empty() {
        // Not a mismatch in itself: a suspended run's recording stops where the
        // run stopped.
        differences.push(format!(
            "the replay produced {} of {} events",
            replayed.len(),
            recorded.len()
        ));
    }
    differences
}

fn describe(kind: &EventKind) -> String {
    match kind {
        EventKind::CapabilityCompleted { cap, result, .. } => {
            format!("{cap} -> {}", short(result))
        }
        EventKind::RunCompleted { result } => format!("completed with {}", short(result)),
        EventKind::RunFailed { error } => format!("failed: {error}"),
        other => other.name().to_string(),
    }
}

fn open(prefix: &str) -> Result<(std::path::PathBuf, Vec<TimedEvent>), ExitCode> {
    let dir = store::find(prefix).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(EXIT_USAGE)
    })?;
    let events = store::read_journal(&dir).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(EXIT_FAILURE)
    })?;
    if events.is_empty() {
        eprintln!("error: `{}` has no journal events", dir.display());
        return Err(ExitCode::from(EXIT_FAILURE));
    }
    Ok((dir, events))
}

/// The answers the broker gave, in order.
fn read_responses(dir: &Path) -> Result<Vec<CapValue>, String> {
    let path = dir.join(store::RESPONSES);
    // A run that called nothing has no answers to record, and replaying it is
    // still worth doing.
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let mut answers = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let json = sic_json::parse(line)
            .map_err(|e| format!("line {} of {}: {e}", number + 1, path.display()))?;
        answers.push(cap_value_from_json(&json).ok_or_else(|| {
            format!(
                "line {} of {} is not a recorded answer",
                number + 1,
                path.display()
            )
        })?);
    }
    Ok(answers)
}

fn cap_value_from_json(json: &sic_json::Json) -> Option<CapValue> {
    use sic_json::Json;
    Some(match json.member("value")? {
        Json::Null => CapValue::Unit,
        Json::Bool(v) => CapValue::Bool(*v),
        Json::Int(v) => CapValue::I64(*v),
        Json::Float(v) => CapValue::F64(*v),
        Json::Str(s) => CapValue::Str(s.clone()),
        _ => return None,
    })
}

/// A sink that stays readable after the journal takes ownership of it.
#[derive(Debug, Clone, Default)]
struct SharedSink(std::rc::Rc<std::cell::RefCell<MemorySink>>);

impl SharedSink {
    fn events(&self) -> Vec<sic_journal::Event> {
        self.0.borrow().events.clone()
    }
}

impl sic_journal::Sink for SharedSink {
    fn emit(&mut self, event: &sic_journal::Event) {
        self.0.borrow_mut().emit(event);
    }
}
