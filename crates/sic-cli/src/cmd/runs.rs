//! `sic runs`, `sic explain`, `sic inspect-run`, `sic replay` and `sic recheck`.
//!
//! None of these run a program except the last two, and both answer every
//! capability call from what was recorded rather than asking the broker. One
//! that called out would be a second run, with a second set of effects, which
//! is the opposite of what either is for.
//!
//! The two are different claims. `replay` runs the *stored* bytecode, so a
//! difference means sic changed. `recheck` compiles a source file and runs
//! that, so a difference means the program did. See `docs/design/runs.md` §4
//! and §5.

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
        // Said only when it is not true, because a column that always reads
        // "resumable" is noise. A run nobody can pick up is still listed: it is
        // waiting, and what changed is that saying so is now honest about
        // whether waiting will end.
        if store::checkpoint_matches(&dir) == Some(false) {
            println!(
                "          this one cannot be picked up: its checkpoint belongs to \
                 different bytecode"
            );
        }
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
pub fn attach(
    prefix: &str,
    value: Option<&str>,
    because: Option<&str>,
    llm: Option<&str>,
) -> ExitCode {
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
            Ok(sink) => Box::new(super::journal::LogSink::around(Box::new(sink), Some(&dir))),
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };

    // A recorded program is a file with ordinary permissions, so it is checked
    // before it is picked up again - the run that wrote it proves nothing about
    // what is in it now.
    if let Err(code) = super::verified(&program, super::From::File(&program_path)) {
        return code;
    }

    let (mut vm, question) = match Vm::restore(&program, &checkpoint, digest, sink) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: cannot pick up `{}`: {e}", dir.display());
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let answer = match super::drive::answer_for(&program, &vm, value) {
        Ok(answer) => answer,
        Err(super::drive::Needs::Reported(code)) => return code,
        Err(super::drive::Needs::Answer(tag)) => {
            // Reading the question is a step of its own, so this is an answer
            // that has not arrived yet rather than a command used wrongly.
            println!("waiting: {question}");
            println!(
                "answer:  sic attach {prefix} --value <{}>",
                tag.short_name()
            );
            return ExitCode::from(super::EXIT_SUSPENDED);
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

    // Continuing, not starting: the run's session is named after its id, so a
    // conversation it was holding is found without anything being looked up -
    // and a pane that is gone is a failure rather than a fresh start.
    let session = super::driver::Session {
        run: run_id_of(&dir),
        continuing: true,
        state: Some(dir.join(store::CONVERSATIONS)),
    };
    let grants = super::drive::manifest(&program);
    let mut broker = match super::driver::open(llm, session, &grants, None) {
        Ok(Some(driver)) => sic_broker::Broker::with_driver(grants, driver),
        Ok(None) => sic_broker::Broker::new(grants),
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let status = vm.resume(answer);
    let outcome = super::drive::drive_recording(&mut vm, &mut broker, status, Some(&dir));

    let still_waiting = matches!(outcome, super::drive::Outcome::Suspended { .. });
    broker.finish(still_waiting);
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
        // What the agent was told, as digests. A file that was not there is
        // said too: an empty list cannot tell "looked and found nothing" from
        // "did not look".
        for instruction in &driver.instructions {
            match &instruction.digest {
                Some(digest) => println!("              told by {} {digest}", instruction.path),
                None => println!("              no {}", instruction.path),
            }
        }
    }

    println!();
    // A budget charge is emitted before the call it pays for, because a call
    // the budget refuses must not leave a request behind - that is the order
    // #32 established and the reason is recorded on it. So the charge is held
    // until its call arrives and printed with it; printed in order, it landed
    // above the call and indented under the previous one.
    //
    // The same shape as the OTLP exporter's fix in #28. This is the reader
    // that was not part of it.
    let mut charged: Vec<(sic_journal::SpanId, u64)> = Vec::new();
    // What the program said, in the order it said it. The journal holds the
    // digest of each line, so the text comes from the run's values file and is
    // matched by position - the same way `responses.jsonl` answers a replay.
    let said = store::read_logs(&dir);
    let mut logged = 0usize;
    for timed in &events {
        if let EventKind::BudgetConsumed { remaining, .. } = &timed.event.kind {
            charged.push((timed.event.span, *remaining));
            continue;
        }
        if let EventKind::Logged { level, .. } = &timed.event.kind {
            let text = match said.get(logged) {
                Some(text) => text.clone(),
                // A run that was not recorded kept no text, and a digest is
                // not a line anybody can read. Saying which is better than
                // printing a hash where a sentence goes.
                None => "(not kept: the run was not recorded)".to_string(),
            };
            logged += 1;
            let indent = "  ".repeat(store::depth_of(&timed.event, &events) + 1);
            println!("{indent}{}: {text}", level.name());
            continue;
        }
        let Some(mut line) = explain_event(timed) else {
            continue;
        };
        if matches!(timed.event.kind, EventKind::CapabilityRequested { .. }) {
            if let Some(at) = charged.iter().position(|(s, _)| *s == timed.event.span) {
                let (_, remaining) = charged.remove(at);
                line.push_str(&format!("  (budget: {remaining} left)"));
            }
        }
        let indent = "  ".repeat(store::depth_of(&timed.event, &events) + 1);
        println!("{indent}{line}");
    }
    // A charge whose call never arrived. Nothing in the VM produces one - the
    // budget refuses before it charges - so this is a journal that was cut
    // between the two, and dropping it would be this reader deciding a run
    // spent nothing because it could not see what it spent.
    for (_, remaining) in &charged {
        println!("  budget: {remaining} left, for a call this journal does not have");
    }

    // The journal records digests, so the one thing it cannot show is what a
    // person was asked and what they said about it. That is here.
    //
    // A file that cannot be read is worth saying so about and no reason to stop:
    // `explain` is what a person reads when something has already gone wrong.
    let answers = match store::read_answers(&dir) {
        Ok(answers) => answers,
        Err(msg) => {
            eprintln!("warning: {msg}");
            Vec::new()
        }
    };
    // A line has a question exactly when somebody was asked; the broker's own
    // answers have none, and are skipped rather than reported as decisions.
    for recorded in &answers {
        let Some(question) = &recorded.asked else {
            continue;
        };
        println!();
        println!("  asked a person:");
        for line in question.lines() {
            println!("    {line}");
        }
        println!("    answered {}", as_recorded(&recorded.value));
        if let Some(because) = &recorded.because {
            println!("    because {because}");
        }
    }
    ExitCode::SUCCESS
}

/// A recorded answer, as a person reads it back.
///
/// Not the same job as `mcp::for_the_agent`, which renders a `CapValue` as the
/// text of an answer. This one is read next to other values in a listing, so a
/// string is quoted and a list is bracketed.
fn as_recorded(value: &CapValue) -> String {
    match value {
        CapValue::Unit => "null".to_string(),
        CapValue::Bool(v) => v.to_string(),
        CapValue::I64(v) => v.to_string(),
        CapValue::F64(v) => format!("{v:?}"),
        CapValue::Str(s) => format!("{s:?}"),
        CapValue::List(items) => {
            let parts: Vec<String> = items.iter().map(|i| format!("{i:?}")).collect();
            format!("[{}]", parts.join(", "))
        }
        CapValue::Exit { code, output } => format!("exited {code}, printed {output:?}"),
    }
}

/// The run a directory holds, which is what it is named.
fn run_id_of(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
        // The agent's own tools, which are not capabilities. A refused one is
        // the more interesting line of the two, so it says why.
        EventKind::ToolUsed {
            tool,
            allowed,
            reason,
            ..
        } => match allowed {
            true => format!("the agent used {tool}"),
            false => format!("the agent was refused {tool}: {reason}"),
        },
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

    let program_path = dir.join(store::PROGRAM).to_string_lossy().into_owned();
    let program = match super::load_bytecode(&program_path) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // The same check the run itself passed. A replay reads the program back off
    // a disk anybody could have written to since, so passing it once is not a
    // fact about the file being read now.
    if let Err(code) = super::verified(&program, super::From::File(&program_path)) {
        return code;
    }
    let answers = match store::read_answers(&dir) {
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
                let Some(answer) = answers.get(used).map(|a| a.value.clone()) else {
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

/// One capability call, as the comparison sees it.
///
/// The name and the digest of the arguments, and nothing else. That pair is the
/// question the recorded answer was an answer to; see `docs/design/runs.md` §5.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asked {
    cap: String,
    args: Digest,
}

fn calls_of<'a>(events: impl Iterator<Item = &'a EventKind>) -> Vec<Asked> {
    events
        .filter_map(|kind| match kind {
            EventKind::CapabilityRequested { cap, args, .. } => Some(Asked {
                cap: cap.clone(),
                args: *args,
            }),
            _ => None,
        })
        .collect()
}

/// `sic recheck <RUN-ID> <FILE.sic>`: is this program still one those answers fit?
///
/// A recorded run is a case the program has been through with real answers, and
/// the question before shipping an edit is whether they still apply. It is not
/// the question `replay` asks - that one runs the *stored* bytecode and
/// establishes determinism, so it failing means sic changed and this failing
/// means the program did.
///
/// It cannot tell you whether a waiting run will resume, because that is
/// already answered: a recorded run keeps its own bytecode and `sic attach`
/// picks it up against that. See `docs/design/runs.md` §5.
pub fn recheck(prefix: &str, source: &str) -> ExitCode {
    let (dir, recorded) = match open(prefix) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let summary = store::summarize(&recorded);

    let program = match super::compile_source(source) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if let Err(code) = super::verified(&program, super::From::Compiler(source)) {
        return code;
    }
    let answers = match store::read_answers(&dir) {
        Ok(answers) => answers,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let Some(entry) = program.func_by_name("main") else {
        eprintln!("error: `{source}` has no `main`");
        return ExitCode::from(EXIT_FAILURE);
    };

    println!(
        "rechecking {} ({}) against {source}",
        summary.run, summary.workflow
    );

    let wanted = calls_of(recorded.iter().map(|t| &t.event.kind));
    // The recording stops where the run stopped, so running out of answers at
    // exactly that point is the recording ending rather than the program
    // diverging.
    let recording_is_complete = !matches!(summary.outcome, store::Outcome::Waiting(_));

    let sink = SharedSink::default();
    let journal = sic_journal::Journal::new(summary.run, Box::new(sink.clone()));
    let mut vm = Vm::with_journal(&program, DEFAULT_FUEL, journal);

    let mut findings: Vec<String> = Vec::new();
    let mut matched = 0usize;
    let mut status = vm.run(entry, &[]);
    loop {
        match status {
            Status::Suspended(_) => {
                let asked = calls_of(sink.events().iter().map(|e| &e.kind));
                let index = asked.len() - 1;
                let now = &asked[index];
                match wanted.get(index) {
                    Some(before) if before == now => {}
                    Some(before) if before.cap != now.cap => {
                        findings.push(format!(
                            "call {}: the recording answered `{}`, this program asks `{}`",
                            index + 1,
                            before.cap,
                            now.cap
                        ));
                        break;
                    }
                    Some(_) => {
                        findings.push(format!(
                            "call {}: `{}`, with different arguments than the recording answered",
                            index + 1,
                            now.cap
                        ));
                        break;
                    }
                    None => {
                        if recording_is_complete {
                            findings.push(format!(
                                "call {}: `{}`, which the recording has no answer for",
                                index + 1,
                                now.cap
                            ));
                        }
                        break;
                    }
                }
                // The call lined up. Counted here rather than after the answer
                // is found, because what is being counted is calls that match
                // the recording - and the last call of a waiting run is one
                // the recording has but never got an answer to.
                matched += 1;
                let Some(answer) = answers.get(index).map(|a| a.value.clone()) else {
                    if recording_is_complete {
                        findings.push(format!(
                            "call {}: `{}` lines up, and there is no recorded answer to give it",
                            index + 1,
                            now.cap
                        ));
                    }
                    break;
                };
                status = vm.resume(answer);
            }
            Status::Failed(ref info) => {
                findings.push(format!("the program failed: {}", info.describe()));
                break;
            }
            _ => break,
        }
    }

    if findings.is_empty() && matched < wanted.len() {
        findings.push(format!(
            "the program made {matched} of the recording's {} calls, so it no longer goes where the run went",
            wanted.len()
        ));
    }

    if findings.is_empty() {
        println!("  {matched} of {} calls matched", wanted.len());
        ExitCode::SUCCESS
    } else {
        println!("  {matched} of {} calls matched", wanted.len());
        for line in &findings {
            println!("  {line}");
        }
        ExitCode::from(EXIT_FAILURE)
    }
}
