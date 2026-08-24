//! Where a recorded run lives, and how to find one again.
//!
//! A directory per run, named by its id, inside the project rather than the
//! home directory: a run belongs to the program it ran, and a program lives in
//! a repository.

use std::path::{Path, PathBuf};

use sic_journal::{Event, EventKind, RunId, TimedEvent};

/// The default place runs are kept, relative to where `sic` was invoked.
const DEFAULT_STORE: &str = ".sic/runs";
/// The environment variable that overrides it.
const STORE_VAR: &str = "SIC_RUNS";

pub const JOURNAL: &str = "journal.jsonl";
pub const PROGRAM: &str = "program.sicb";
/// What the broker answered. Values, unlike the journal - see
/// docs/design/runs.md.
pub const RESPONSES: &str = "responses.jsonl";
pub const CHECKPOINT: &str = "checkpoint.sicc";
/// What answered the run's model calls, when anything did - see
/// docs/design/driving.md §6.
pub const DRIVER: &str = "driver.json";
/// Which conversations the run has open. Written by the driver, not by this
/// crate: it is the driver's own state, and this only says where to keep it.
pub const CONVERSATIONS: &str = "conversations";

pub fn store_root() -> PathBuf {
    match std::env::var(STORE_VAR) {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(DEFAULT_STORE),
    }
}

pub fn run_dir(run: RunId) -> PathBuf {
    store_root().join(run.to_string())
}

/// Creates a run's directory.
pub fn create(run: RunId) -> Result<PathBuf, String> {
    let dir = run_dir(run);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
    Ok(dir)
}

/// Finds a run by a prefix of its id, the way a person types one.
pub fn find(prefix: &str) -> Result<PathBuf, String> {
    let root = store_root();
    let entries =
        std::fs::read_dir(&root).map_err(|e| format!("cannot read `{}`: {e}", root.display()))?;

    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) && entry.path().join(JOURNAL).exists() {
            matches.push(entry.path());
        }
    }
    matches.sort();
    match matches.len() {
        0 => Err(format!(
            "no run in `{}` starts with `{prefix}`",
            root.display()
        )),
        1 => Ok(matches.remove(0)),
        // Answering the wrong run's question is worse than asking for more of
        // the id.
        n => Err(format!("`{prefix}` matches {n} runs; use more of the id")),
    }
}

/// Every stored run, oldest first by id.
pub fn list() -> Result<Vec<PathBuf>, String> {
    let root = store_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut runs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(JOURNAL).exists())
        .collect();
    runs.sort();
    Ok(runs)
}

/// Reads a run's journal, saying what it could not read.
///
/// A journal is append-only and a run can be killed mid-write, so its last line
/// may be a fragment. Skipping it is right - refusing to look at a run because
/// its last line is half-written would refuse exactly the runs worth looking at
/// - but skipping it in silence is not, because of which line that usually is.
///
/// The last line of a journal is `run_completed`, `run_failed` or
/// `run_suspended`, and those three are the only ones `summarize` reads an
/// outcome from. So a run that is waiting for an answer becomes `unfinished`
/// and drops out of `sic runs --waiting` - the list a person or an agent works
/// from - without a word, and whatever was going to answer it never learns that
/// a line could not be read. `sic replay` has the second version of the same
/// problem: it reports the missing event as a determinism finding against the
/// VM, caused by half a line.
///
/// The warning is printed here rather than handed back so that every command
/// reading a recorded run inherits it. A caller that has to remember to ask is
/// a caller that will not.
pub fn read_journal(dir: &Path) -> Result<Vec<TimedEvent>, String> {
    let path = dir.join(JOURNAL);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let result = sic_journal::read_jsonl(&text);
    for skipped in &result.skipped {
        // Named, because `sic runs` reads many journals and a warning that did
        // not say which one would send somebody to the wrong run.
        eprintln!("warning: {}: skipped {skipped}", path.display());
    }
    Ok(result.events)
}

/// What a run's journal says about how it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Failed(String),
    /// Stopped waiting for something, and never picked up again.
    Waiting(String),
    /// The journal does not say - a run that was killed.
    Unfinished,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Failed(_) => "failed",
            Outcome::Waiting(_) => "waiting",
            Outcome::Unfinished => "unfinished",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Outcome::Failed(text) | Outcome::Waiting(text) => Some(text),
            _ => None,
        }
    }
}

/// A one-line summary of a run, from its journal alone.
#[derive(Debug, Clone)]
pub struct Summary {
    pub run: RunId,
    pub workflow: String,
    pub outcome: Outcome,
    pub capability_calls: usize,
    pub events: usize,
}

pub fn summarize(events: &[TimedEvent]) -> Summary {
    let mut workflow = String::new();
    let mut outcome = Outcome::Unfinished;
    let mut capability_calls = 0;
    let mut run = RunId(0);

    for timed in events {
        run = timed.event.run;
        match &timed.event.kind {
            EventKind::RunStarted { workflow: name, .. } => workflow = name.clone(),
            EventKind::RunCompleted { .. } => outcome = Outcome::Completed,
            EventKind::RunFailed { error } => outcome = Outcome::Failed(error.clone()),
            EventKind::RunSuspended { cap } => outcome = Outcome::Waiting(cap.clone()),
            // A resumed run is no longer waiting; whatever comes next decides.
            EventKind::RunResumed { .. } => outcome = Outcome::Unfinished,
            EventKind::CapabilityRequested { .. } => capability_calls += 1,
            _ => {}
        }
    }

    Summary {
        run,
        workflow,
        outcome,
        capability_calls,
        events: events.len(),
    }
}

/// How deeply nested an event is, for indenting a summary.
///
/// Spans carry their parent, so depth is a walk up that chain rather than
/// anything reconstructed.
pub fn depth_of(event: &Event, events: &[TimedEvent]) -> usize {
    let mut depth = 0;
    let mut parent = event.parent;
    while let Some(span) = parent {
        depth += 1;
        if depth > 32 {
            break;
        }
        parent = events
            .iter()
            .find(|e| e.event.span == span)
            .and_then(|e| e.event.parent);
    }
    depth
}

/// Whether a waiting run's checkpoint still belongs to the program beside it.
///
/// A checkpoint carries the digest of the bytecode it came from, and resuming
/// against anything else would continue one program inside another. That is
/// computable before anybody spends another day waiting for an answer nobody
/// can use, and a run that cannot be picked up is a thing to say rather than a
/// thing to discover.
///
/// `None` means there is nothing to compare - no checkpoint, or no recorded
/// program - which is not the same as a mismatch and is not reported as one.
pub fn checkpoint_matches(dir: &Path) -> Option<bool> {
    let checkpoint = std::fs::read(dir.join(CHECKPOINT)).ok()?;
    let program = std::fs::read(dir.join(PROGRAM)).ok()?;
    let saved = sic_vm::Checkpoint::decode(&checkpoint).ok()?;
    Some(saved.program_digest == sic_core::Digest::of(&program))
}

/// What a waiting run is waiting for, read from its checkpoint.
pub fn pending_question(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join(CHECKPOINT)).ok()?;
    sic_vm::Checkpoint::decode(&bytes).ok().map(|c| c.question)
}

/// One recorded answer, as it goes into `responses.jsonl`.
pub struct Answer<'a> {
    pub value: &'a sic_core::CapValue,
    /// The question a person was asked, when one was. The broker's own answers
    /// have none, because nobody was asked.
    pub asked: Option<&'a str>,
    /// Why they answered that way, if they said. Free text a person wrote,
    /// which is why it lives here rather than in the journal.
    pub because: Option<&'a str>,
}

impl<'a> Answer<'a> {
    pub fn from_broker(value: &'a sic_core::CapValue) -> Answer<'a> {
        Answer {
            value,
            asked: None,
            because: None,
        }
    }
}

/// Appends one recorded answer.
///
/// These are values, unlike the journal. Keeping them in their own file means
/// the file that is safe to ship stays safe to ship, and the one that is not is
/// one file, named, in a directory you can delete.
pub fn record_answer(dir: &Path, answer: &Answer<'_>) -> Result<(), String> {
    use std::io::Write;

    let path = dir.join(RESPONSES);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    writeln!(file, "{}", answer_to_json(answer))
        .map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

fn answer_to_json(answer: &Answer<'_>) -> String {
    use sic_core::CapValue;
    let rendered = match answer.value {
        CapValue::Unit => "null".to_string(),
        CapValue::Bool(v) => v.to_string(),
        CapValue::I64(v) => v.to_string(),
        CapValue::F64(v) => format!("{v:?}"),
        CapValue::Str(s) => json_string(s),
        // No capability answers with one yet; an argument vector goes the
        // other way. Recording it as an array keeps the file readable if one
        // ever does.
        CapValue::List(items) => {
            let parts: Vec<String> = items.iter().map(|i| json_string(i)).collect();
            format!("[{}]", parts.join(","))
        }
    };
    let mut out = format!("{{\"value\":{rendered}");
    // An index on its own says nothing six months later. The question carries
    // the alternatives, so recording it is what keeps what was *not* chosen.
    if let Some(asked) = answer.asked {
        out.push_str(&format!(",\"asked\":{}", json_string(asked)));
    }
    if let Some(because) = answer.because {
        out.push_str(&format!(",\"because\":{}", json_string(because)));
    }
    out.push('}');
    out
}

/// One answer read back out of `responses.jsonl`.
///
/// The owned counterpart of `Answer`. Reading it lives beside writing it
/// because a file has one format: `driver.json` is written and read twenty
/// lines apart and has never disagreed with itself, while this file's shape was
/// decided here and worked out again twice in `runs.rs`, where a recorded list
/// was a shape neither reader had been told about.
pub struct Recorded {
    pub value: sic_core::CapValue,
    /// The question a person was asked, when one was.
    pub asked: Option<String>,
    /// Why they answered that way, if they said.
    pub because: Option<String>,
}

/// Every answer a run recorded, in the order it recorded them.
///
/// A run that called nothing has no answers to record, and replaying it is
/// still worth doing, so a file that is not there is no answers rather than an
/// error. A line that is there and is not an answer is an error: `replay`
/// answers a program's capability calls out of this file, and a file it can
/// only partly read is not a recording anything can be believed from.
pub fn read_answers(dir: &Path) -> Result<Vec<Recorded>, String> {
    let path = dir.join(RESPONSES);
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
        answers.push(answer_from_json(&json).ok_or_else(|| {
            format!(
                "line {} of {} is not a recorded answer",
                number + 1,
                path.display()
            )
        })?);
    }
    Ok(answers)
}

/// One line, in the vocabulary `answer_to_json` writes.
fn answer_from_json(json: &sic_json::Json) -> Option<Recorded> {
    use sic_core::CapValue;
    use sic_json::Json;

    let value = match json.member("value")? {
        Json::Null => CapValue::Unit,
        Json::Bool(v) => CapValue::Bool(*v),
        Json::Int(v) => CapValue::I64(*v),
        Json::Float(v) => CapValue::F64(*v),
        Json::Str(s) => CapValue::Str(s.clone()),
        // An argument vector, written as an array by `answer_to_json` above.
        // A `CapValue::List` holds strings and nothing more general, so an
        // array of anything else did not come from this writer.
        Json::Array(items) => {
            let mut strings = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Json::Str(s) => strings.push(s.clone()),
                    _ => return None,
                }
            }
            CapValue::List(strings)
        }
        Json::Object(_) => return None,
    };
    let text = |name: &str| match json.member(name) {
        Some(Json::Str(s)) => Some(s.clone()),
        _ => None,
    };
    Some(Recorded {
        value,
        asked: text("asked"),
        because: text("because"),
    })
}

/// Records what is going to answer this run's model calls.
///
/// Not a journal event: the journal has a fixed vocabulary of events about what
/// the *program* did, and records digests rather than values. Which build of
/// which tool was on this machine is neither, and it is exactly what a person
/// reading a run's answers back needs to know, because reading a terminal user
/// interface is a bet on a version.
pub fn record_driver(dir: &Path, info: &sic_broker::DriverInfo) -> Result<(), String> {
    // Every path that was looked at, with a digest or without one. A file that
    // was not there is as much a fact about the run as one that was.
    let instructions: Vec<String> = info
        .instructions
        .iter()
        .map(|i| match &i.digest {
            Some(digest) => format!(
                "{{\"path\":{},\"sha256\":{}}}",
                json_string(&i.path),
                json_string(&digest.to_string())
            ),
            None => format!("{{\"path\":{},\"absent\":true}}", json_string(&i.path)),
        })
        .collect();
    let json = format!(
        "{{\"driver\":{},\"command\":{},\"agent\":{},\"multiplexer\":{},\"instructions\":[{}]}}\n",
        json_string(&info.driver),
        json_string(&info.command),
        json_string(&info.agent),
        json_string(&info.multiplexer),
        instructions.join(","),
    );
    let path = dir.join(DRIVER);
    std::fs::write(&path, json).map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

/// What answered a recorded run, if anything did.
pub fn read_driver(dir: &Path) -> Option<sic_broker::DriverInfo> {
    let text = std::fs::read_to_string(dir.join(DRIVER)).ok()?;
    let json = sic_json::parse(&text).ok()?;
    let field = |name: &str| match json.member(name) {
        Some(sic_json::Json::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let instructions = match json.member("instructions") {
        Some(sic_json::Json::Array(items)) => items
            .iter()
            .map(|item| sic_broker::agent::Instruction {
                path: match item.member("path") {
                    Some(sic_json::Json::Str(p)) => p.clone(),
                    _ => String::new(),
                },
                digest: match item.member("sha256") {
                    Some(sic_json::Json::Str(text)) => digest_from(text),
                    _ => None,
                },
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(sic_broker::DriverInfo {
        driver: field("driver"),
        command: field("command"),
        agent: field("agent"),
        multiplexer: field("multiplexer"),
        instructions,
    })
}

/// A digest as it was written: `sha256:` and 64 hex characters.
///
/// Anything else reads as absent rather than as a digest. A record that cannot
/// be read is not a record, and inventing one would be worse than saying
/// nothing - the whole point of keeping these is that they can be compared.
fn digest_from(text: &str) -> Option<sic_core::Digest> {
    let hex = text.strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(sic_core::Digest::from_bytes(bytes))
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sic_core::CapValue;

    /// One answer, through the writer's vocabulary and back.
    fn round_trip(value: &CapValue) -> Recorded {
        let answer = Answer {
            value,
            asked: Some("what should we deploy?"),
            because: Some("it is the only one that builds"),
        };
        let line = answer_to_json(&answer);
        let json = sic_json::parse(&line).expect("the writer writes JSON");
        answer_from_json(&json).expect("the reader reads what the writer writes")
    }

    /// `answer_to_json` writes a list as an array on purpose. While the reading
    /// lived in `runs.rs`, an array was a shape neither reader had been told
    /// about, and `replay` called the whole file not a recording because of it.
    #[test]
    fn a_list_answer_reads_back_as_a_list() {
        let value = CapValue::List(vec!["send-keys".into(), "a \"quoted\" one".into()]);
        assert_eq!(round_trip(&value).value, value);
    }

    #[test]
    fn every_answer_shape_survives_the_round_trip() {
        for value in [
            CapValue::Unit,
            CapValue::Bool(true),
            CapValue::I64(-7),
            CapValue::F64(0.5),
            CapValue::Str("a\nb\t\"c\"".into()),
            CapValue::List(Vec::new()),
        ] {
            let recorded = round_trip(&value);
            assert_eq!(recorded.value, value, "{value:?} did not survive");
            assert_eq!(recorded.asked.as_deref(), Some("what should we deploy?"));
            assert_eq!(
                recorded.because.as_deref(),
                Some("it is the only one that builds")
            );
        }
    }

    /// The broker's own answers have no question, because nobody was asked.
    #[test]
    fn an_answer_nobody_was_asked_for_carries_no_question() {
        let value = CapValue::Str("hello".into());
        let line = answer_to_json(&Answer::from_broker(&value));
        let json = sic_json::parse(&line).expect("the writer writes JSON");
        let recorded = answer_from_json(&json).expect("a broker answer is an answer");
        assert_eq!(recorded.value, value);
        assert!(recorded.asked.is_none());
        assert!(recorded.because.is_none());
    }
}
