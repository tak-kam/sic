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

/// `sic runs [--waiting]`: what has been recorded.
///
/// `--waiting` narrows it to the runs that stopped for an answer, and prints
/// what each one is waiting for. That is the list something answering runs -
/// a person, or an agent driving `sic` - works from.
pub fn list_waiting() -> ExitCode {
    let runs = match store::list() {
        Ok(runs) => runs,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let mut found = 0;
    for dir in runs {
        let Ok(events) = store::read_journal(&dir) else {
            continue;
        };
        let summary = store::summarize(&events);
        if !matches!(summary.outcome, store::Outcome::Waiting(_)) {
            continue;
        }
        let Some(question) = store::pending_question(&dir) else {
            continue;
        };
        found += 1;
        // The question is last, because it is the only field that can contain
        // spaces.
        println!(
            "{}  {:<10}  {:<14}  {question}",
            &summary.run.to_string()[..8],
            summary.workflow,
            summary.outcome.detail().unwrap_or("")
        );
    }
    if found == 0 {
        println!("nothing is waiting");
    }
    ExitCode::SUCCESS
}

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

/// `sic attach <id> [--value V]`: pick up a run that stopped for an answer.
///
/// Without a value it says what the run is waiting for and stops; with one it
/// answers and carries on. Everything it needs - the bytecode, the checkpoint,
/// where the journal goes - is in the run's directory, so a run is identified
/// by its id and nothing else has to be remembered.
///
/// The read-only form matters as much as the other: whatever is going to answer
/// has to be able to find out what the question is first.
pub fn attach(prefix: &str, value: Option<&str>, because: Option<&str>) -> ExitCode {
    let dir = match store::find(prefix) {
        Ok(dir) => dir,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let checkpoint_path = dir.join(store::CHECKPOINT);
    let Ok(checkpoint) = std::fs::read(&checkpoint_path) else {
        eprintln!("error: run `{}` is not waiting for anything", prefix);
        return ExitCode::from(EXIT_USAGE);
    };

    let program_path = dir.join(store::PROGRAM).to_string_lossy().into_owned();
    let program = match super::load_bytecode(&program_path) {
        Ok(program) => program,
        Err(code) => return code,
    };
    // The bytecode is the one the run started with, so the digest matches by
    // construction - which is the point of storing it beside the checkpoint.
    let digest = Digest::of(&sic_bytecode::encode(&program));

    let sink: Box<dyn sic_journal::Sink> =
        match super::journal::FileSink::append(&dir.join(store::JOURNAL).to_string_lossy()) {
            Ok(sink) => Box::new(sink),
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };

    let (mut vm, question) = match Vm::restore(&program, &checkpoint, digest, sink) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: cannot pick up `{}`: {e}", dir.display());
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let Some(cap) = vm.pending_capability().map(str::to_string) else {
        eprintln!("internal error: the checkpoint is not waiting for anything");
        return ExitCode::from(EXIT_FAILURE);
    };
    let Some(tag) = super::drive::capability_return_type(&program, &cap) else {
        eprintln!("error: `{cap}` is not a capability this program declares");
        return ExitCode::from(EXIT_FAILURE);
    };

    let Some(text) = value else {
        println!("waiting: {question}");
        println!(
            "answer:  sic attach {prefix} --value <{}>",
            tag.short_name()
        );
        return ExitCode::from(super::EXIT_SUSPENDED);
    };
    let answer = match super::drive::parse_answer(text, tag) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("error: {msg}, and `{cap}` returns {}", tag.short_name());
            return ExitCode::from(EXIT_USAGE);
        }
    };
    // Recorded so that replaying the run answers it the same way - and, since
    // a person answered this one, with what they were asked and why.
    let recorded = store::Answer {
        value: &answer,
        asked: Some(&question),
        because,
    };
    if let Err(msg) = store::record_answer(&dir, &recorded) {
        eprintln!("warning: {msg}");
    }

    let mut broker = sic_broker::Broker::new(super::drive::manifest(&program));
    let status = vm.resume(answer);
    let outcome = super::drive::drive_recording(&mut vm, &mut broker, status, Some(&dir));

    let still_waiting = matches!(outcome, super::drive::Outcome::Suspended { .. });
    let hint = format!("sic attach {prefix} --value <VALUE>");
    let code = super::run::finish(
        &mut vm,
        &program,
        outcome,
        Some(&checkpoint_path.to_string_lossy()),
        Some(&hint),
    );
    // A finished run that kept its checkpoint would keep showing up as waiting.
    if !still_waiting {
        std::fs::remove_file(&checkpoint_path).ok();
    }
    code
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
    // Reading a terminal user interface is a bet on a version, so a run whose
    // model calls an agent answered says which build of what answered them.
    if let Some(driver) = store::read_driver(&dir) {
        println!("  answered by {} at {}", driver.driver, driver.command);
        println!("              {}, {}", driver.agent, driver.multiplexer);
    }

    println!();
    for timed in &events {
        let Some(line) = explain_event(timed) else {
            continue;
        };
        let indent = "  ".repeat(store::depth_of(&timed.event, &events) + 1);
        println!("{indent}{line}");
    }

    // The journal records digests, so the one thing it cannot show is what a
    // person was asked and what they said about it. That is here.
    for asked in read_asked(&dir) {
        println!();
        println!("  asked a person:");
        for line in asked.question.lines() {
            println!("    {line}");
        }
        println!("    answered {}", asked.answer);
        if let Some(because) = &asked.because {
            println!("    because {because}");
        }
    }
    ExitCode::SUCCESS
}

/// One question a person answered, read back out of `responses.jsonl`.
struct Asked {
    question: String,
    answer: String,
    because: Option<String>,
}

/// The answers a person gave, in the order they gave them.
///
/// A line has a question exactly when somebody was asked; the broker's own
/// answers have none, and are skipped rather than reported as decisions.
fn read_asked(dir: &Path) -> Vec<Asked> {
    let path = dir.join(store::RESPONSES);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(json) = sic_json::parse(line) else {
            continue;
        };
        let Some(sic_json::Json::Str(question)) = json.member("asked") else {
            continue;
        };
        let answer = match json.member("value") {
            Some(sic_json::Json::Str(s)) => format!("{s:?}"),
            Some(sic_json::Json::Int(v)) => v.to_string(),
            Some(sic_json::Json::Bool(v)) => v.to_string(),
            Some(other) => other.kind().to_string(),
            None => continue,
        };
        let because = match json.member("because") {
            Some(sic_json::Json::Str(s)) => Some(s.clone()),
            _ => None,
        };
        out.push(Asked {
            question: question.clone(),
            answer,
            because,
        });
    }
    out
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
///
/// Suspending, checkpointing and resuming are left out of the comparison. They
/// record how a run was carried out (in how many sittings the answers arrived)
/// rather than what the program did. A run that stopped twice for a person is
/// the same run as one that was answered immediately.
fn compare(recorded: &[TimedEvent], replayed: &[sic_journal::Event]) -> Vec<String> {
    let original: Vec<&sic_journal::Event> = recorded
        .iter()
        .map(|t| &t.event)
        .filter(|e| is_about_the_program(&e.kind))
        .collect();
    let again: Vec<&sic_journal::Event> = replayed
        .iter()
        .filter(|e| is_about_the_program(&e.kind))
        .collect();

    let mut differences = Vec::new();
    for (i, replayed_event) in again.iter().enumerate() {
        let Some(recorded_event) = original.get(i) else {
            differences.push(format!(
                "the replay produced {} which the recording does not have",
                replayed_event.kind.name()
            ));
            break;
        };
        if recorded_event.kind != replayed_event.kind {
            differences.push(format!(
                "seq {}: recorded {}, replayed {}",
                recorded_event.seq,
                describe(&recorded_event.kind),
                describe(&replayed_event.kind)
            ));
            break;
        }
    }
    if again.len() < original.len() && differences.is_empty() {
        // Not a mismatch in itself: a suspended run's recording stops where the
        // run stopped.
        differences.push(format!(
            "the replay produced {} of {} events",
            again.len(),
            original.len()
        ));
    }
    differences
}

/// Whether an event says something about what the program did, rather than
/// about how its execution was arranged.
fn is_about_the_program(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::RunSuspended { .. }
            | EventKind::RunResumed { .. }
            | EventKind::CheckpointWritten { .. }
    )
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
