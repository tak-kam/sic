//! Where a driver comes from, and what its absence means.
//!
//! Driving an agent needs two things that are not everywhere: a unix socket for
//! the capabilities the agent reaches back through, and tmux for the pane it is
//! answering in. Neither is a thing on Windows, and tmux is not going to be.
//!
//! So this is one seam with two sides rather than a conditional at every call
//! site: `run`, `resume` and `attach` each build a `Session` and ask for a
//! driver, and what changes between platforms is what they get back.

use sic_core::CapGrant;

// The trait, for `info()` on the driver `open` builds. Only the unix side has
// one to call it on.
#[cfg(unix)]
use sic_broker::AgentDriver;
#[cfg(unix)]
pub use sic_broker::tmux::Session;

/// The same three facts, on a platform with nowhere to put them.
///
/// A struct rather than nothing, so that the three commands that build one read
/// the same on every platform. What differs is `open`, which is where the
/// difference actually is.
#[cfg(not(unix))]
#[derive(Debug)]
// Written and never read, which is the point: the three commands that build one
// read the same on every platform, and `open` below is where the difference is.
#[allow(dead_code)]
pub struct Session {
    pub run: String,
    pub continuing: bool,
    pub state: Option<std::path::PathBuf>,
}

/// Opens the driver a `--llm` spec names, or none when there is no spec.
///
/// `Ok(None)` is not a degraded driver: without one, `llm.invoke` defers and a
/// person answers it, which is what it did before any driver existed.
#[cfg(unix)]
pub fn open(
    spec: Option<&str>,
    session: Session,
    manifest: &[CapGrant],
    recording: Option<&std::path::Path>,
) -> Result<Option<Box<dyn sic_broker::AgentDriver>>, String> {
    let Some(spec) = spec else {
        return Ok(None);
    };
    // Before anything runs. A manifest that cannot be enforced against the
    // agent is worse than no manifest, because `sic plan` printed it.
    let authority = sic_broker::authority_of(manifest).map_err(|refused| refused.to_string())?;
    let mut driver = sic_broker::TmuxDriver::open(spec, session).map_err(|e| e.message)?;
    driver
        .authorize(authority, manifest.to_vec())
        .map_err(|e| e.message)?;
    let info = driver.info().clone();
    if let Some(dir) = recording {
        super::store::record_driver(dir, &info)?;
    }
    eprintln!(
        "llm.invoke answered by {} - {}, {}",
        info.driver, info.agent, info.multiplexer
    );
    Ok(Some(Box::new(driver)))
}

/// The same question where there is no driver to be had.
///
/// A spec is refused rather than ignored, and the message says which of the two
/// missing things it is: a person who asked for an agent and got a run that
/// quietly waited for a human would have been told nothing.
#[cfg(not(unix))]
pub fn open(
    spec: Option<&str>,
    _session: Session,
    _manifest: &[CapGrant],
    _recording: Option<&std::path::Path>,
) -> Result<Option<Box<dyn sic_broker::AgentDriver>>, String> {
    match spec {
        None => Ok(None),
        Some(spec) => Err(format!(
            "`--llm {spec}` needs tmux and a unix socket, and this build has neither; \
             without a driver `llm.invoke` waits for a person, which `sic attach` answers"
        )),
    }
}
