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
