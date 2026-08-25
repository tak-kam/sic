//! Writing an execution journal to a file, and naming a run.
//!
//! The journal crate reads no clock and opens no file, which is what keeps a
//! run reproducible. Both of those are here instead, on the outside, where a
//! timestamp is what it actually is: an observation about a run, not part of
//! it.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use sic_core::Sha256;
use sic_journal::{Event, RunId, Sink, json::event_to_json};

/// Appends events to a file as JSON Lines.
#[derive(Debug)]
pub struct FileSink {
    out: BufWriter<File>,
}

impl FileSink {
    pub fn create(path: &str) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("cannot write `{path}`: {e}"))?;
        Ok(Self {
            out: BufWriter::new(file),
        })
    }

    /// Opens a journal to continue writing to.
    ///
    /// A resumed run is the same run, so its events belong in the same file,
    /// after the ones that are already there.
    pub fn append(path: &str) -> Result<Self, String> {
        let file = File::options()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("cannot append to `{path}`: {e}"))?;
        Ok(Self {
            out: BufWriter::new(file),
        })
    }
}

impl Sink for FileSink {
    fn emit(&mut self, event: &Event) {
        let line = event_to_json(event);
        // The wall clock is added here, as a field in front of the event's own
        // ones. The journal never orders by it: `seq` is the order.
        let line = match now_nanos() {
            Some(nanos) => format!("{{\"ts\":{nanos},{}", &line[1..]),
            None => line,
        };
        // A journal that cannot be written must not take the run down with it,
        // but it must not be silent either.
        if let Err(e) = writeln!(self.out, "{line}") {
            eprintln!("warning: cannot write to the journal: {e}");
        }
    }
}

impl Drop for FileSink {
    fn drop(&mut self) {
        if let Err(e) = self.out.flush() {
            eprintln!("warning: the journal may be incomplete: {e}");
        }
    }
}

fn now_nanos() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos())
}

/// A fresh run id.
///
/// This has to be unique, not unguessable: it names a run in a journal. It is
/// derived from the clock and the process id so that two runs never collide,
/// and hashed so that it does not read as a timestamp anyone might rely on.
pub fn new_run_id() -> RunId {
    let mut h = Sha256::new();
    h.update(&now_nanos().unwrap_or(0).to_le_bytes());
    h.update(&(std::process::id() as u64).to_le_bytes());
    let digest = h.finish();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.bytes()[..16]);
    RunId(u128::from_le_bytes(bytes))
}

/// Shows what the program said, and keeps it when the run is being kept.
///
/// Always wrapped around whatever sink a run has, including the one that
/// writes nothing: `log` is what a program says about itself while it works,
/// and a person watching should not have to have passed `--journal` to see it.
///
/// Two destinations because a log line has two audiences, and they are one
/// statement rather than two decisions: it goes where a person can see it, and
/// it is kept where the run is kept. That is what `responses.jsonl` already
/// does with what a capability answered.
///
/// stderr rather than stdout, because stdout is the value the program returned
/// and `sic run` prints it there. A line saying what happened must not be
/// mistaken for what came out.
#[derive(Debug)]
pub struct LogSink {
    inner: Box<dyn Sink>,
    /// Where the text is kept, for a run that is being recorded. `None` is a
    /// run nobody asked to keep, and its lines are shown and not written -
    /// the same promise `responses.jsonl` makes.
    values: Option<BufWriter<File>>,
}

impl LogSink {
    pub fn around(inner: Box<dyn Sink>, keep_in: Option<&std::path::Path>) -> Self {
        let values = keep_in.and_then(|dir| {
            let path = dir.join(super::store::LOGS);
            match File::options().create(true).append(true).open(&path) {
                Ok(file) => Some(BufWriter::new(file)),
                Err(e) => {
                    eprintln!(
                        "warning: cannot keep log lines in `{}`: {e}",
                        path.display()
                    );
                    None
                }
            }
        });
        Self { inner, values }
    }
}

impl Sink for LogSink {
    fn emit(&mut self, event: &Event) {
        if let sic_journal::EventKind::Logged { level, message } = &event.kind {
            eprintln!("{}: {message}", level.name());
            if let Some(out) = self.values.as_mut() {
                let line = format!(
                    "{{\"level\":{},\"message\":{}}}",
                    sic_json::quoted(level.name()),
                    sic_json::quoted(message)
                );
                if let Err(e) = writeln!(out, "{line}") {
                    eprintln!("warning: cannot keep a log line: {e}");
                }
            }
        }
        self.inner.emit(event);
    }
}
