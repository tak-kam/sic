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

/// Reads a run's journal.
pub fn read_journal(dir: &Path) -> Result<Vec<TimedEvent>, String> {
    let path = dir.join(JOURNAL);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let result = sic_journal::read_jsonl(&text);
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

/// Records what is going to answer this run's model calls.
///
/// Not a journal event: the journal has a fixed vocabulary of events about what
/// the *program* did, and records digests rather than values. Which build of
/// which tool was on this machine is neither, and it is exactly what a person
/// reading a run's answers back needs to know, because reading a terminal user
/// interface is a bet on a version.
pub fn record_driver(dir: &Path, info: &sic_broker::DriverInfo) -> Result<(), String> {
    let json = format!(
        "{{\"driver\":{},\"command\":{},\"agent\":{},\"multiplexer\":{}}}\n",
        json_string(&info.driver),
        json_string(&info.command),
        json_string(&info.agent),
        json_string(&info.multiplexer),
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
    Some(sic_broker::DriverInfo {
        driver: field("driver"),
        command: field("command"),
        agent: field("agent"),
        multiplexer: field("multiplexer"),
    })
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
