//! The capabilities the agent reaches through the broker.
//!
//! A grant that the agent's own permission system cannot enforce is not handed
//! to it in a weaker form. The agent's native tool is denied and the capability
//! is offered instead as a tool that arrives here, where it is authorized
//! against the same manifest and performed by the same code. See
//! `docs/design/authority.md` §4.
//!
//! There is no thread. The socket is served from the loop that is already
//! waiting for the agent to answer, which is the only time an agent can be
//! making a call: single-threaded, no lock, and a tool use cannot happen at a
//! moment when nothing is watching for it.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use sic_core::{CapGrant, CapRequest, Digest, ToolUse, answer_from_bytes, answer_to_bytes};

use crate::authority::reach_of;

/// The largest message either side will read. A length prefix is a promise,
/// and this is what stops one from being believed.
const MAX_FRAME: u32 = 1 << 20;

/// What is being asked of the run: what do you offer, or perform this.
const ASK_LIST: u8 = 0;
const ASK_CALL: u8 = 1;

/// One parameter of a routed capability, as the agent has to supply it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param {
    Str,
    Strings,
}

/// A capability offered to the agent as a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    /// The manifest index, so a call names the entry it is authorized against
    /// rather than a name the far side chose.
    pub index: u32,
    pub cap: String,
    pub constraint: String,
    pub params: Vec<Param>,
}

impl Offered {
    /// The name the agent calls it by. MCP allows a dot, but the tool shows up
    /// in a permission rule as `mcp__sic__<name>`, and one separator that never
    /// needs escaping is worth more than matching the manifest's spelling.
    pub fn tool_name(&self) -> String {
        self.cap.replace('.', "_")
    }
}

/// Every capability in the manifest that the agent has to reach through here.
pub fn offered(manifest: &[CapGrant]) -> Vec<Offered> {
    manifest
        .iter()
        .enumerate()
        .filter(|(_, grant)| matches!(reach_of(grant), crate::authority::Reach::Routed(_)))
        .map(|(index, grant)| Offered {
            index: index as u32,
            cap: grant.name.clone(),
            constraint: grant.constraint.clone(),
            params: params_of(&grant.name),
        })
        .collect()
}

/// What a capability takes, in the shape the agent has to send.
///
/// The signatures live in `sic-types`, which this crate must not depend on -
/// the broker is on the other side of a boundary from the compiler. They are
/// repeated here for the routed capabilities only, and a capability that grows
/// a parameter has to be added in both places, which is the cost of the
/// boundary rather than an oversight.
fn params_of(cap: &str) -> Vec<Param> {
    match cap {
        "process.exec" | "process.capture" => vec![Param::Str, Param::Strings],
        "human.approve" => vec![Param::Str],
        "human.choose" => vec![Param::Str, Param::Strings],
        _ => Vec::new(),
    }
}

/// The socket the agent's calls arrive on.
#[derive(Debug)]
pub struct Route {
    listener: UnixListener,
    path: PathBuf,
    manifest: Vec<CapGrant>,
    used: Vec<ToolUse>,
}

impl Route {
    /// Opens the socket. Nothing is served until `serve_pending` is called.
    pub fn open(path: PathBuf, manifest: Vec<CapGrant>) -> std::io::Result<Route> {
        // A stale socket from a run that was killed would otherwise make this
        // fail for a reason that has nothing to do with this run.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Route {
            listener,
            path,
            manifest,
            used: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Answers whatever is waiting, and returns.
    ///
    /// Never blocks: this runs inside the loop that is also watching a pane, so
    /// a caller that took its time would stall the thing it is watching for.
    pub fn serve_pending(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => self.serve(stream),
                // Nothing waiting, which is the usual case.
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                // A socket that cannot be accepted from is not something this
                // loop can fix, and it must not become a spin.
                Err(_) => return,
            }
        }
    }

    /// One connection: one question, one answer.
    ///
    /// One rather than many because a connection that could ask twice would be
    /// a connection whose second question arrives after the run stopped
    /// listening.
    fn serve(&mut self, mut stream: UnixStream) {
        // A blocking read, now that there is something to read: the agent is
        // waiting for this answer and nothing else is going to happen first.
        let _ = stream.set_nonblocking(false);
        let Some(body) = read_frame(&mut stream) else {
            return;
        };
        match body.split_first() {
            Some((&ASK_LIST, _)) => {
                let _ = write_frame(&mut stream, &offered_to_bytes(&offered(&self.manifest)));
            }
            Some((&ASK_CALL, rest)) => self.serve_call(&mut stream, rest),
            // Not a question this speaks. Saying nothing is right: the far side
            // is not a peer whose vocabulary this negotiates.
            _ => {}
        }
    }

    fn serve_call(&mut self, stream: &mut UnixStream, body: &[u8]) {
        let answer = match CapRequest::from_bytes(body) {
            Ok(request) => {
                let answer = crate::perform(&self.manifest, &request);
                self.used.push(ToolUse {
                    cap: request.name.clone(),
                    args: digest_of_args(&request),
                    outcome: match &answer {
                        Ok(sic_core::CapOutcome::Value(v)) => Ok(digest_of_value(v)),
                        Ok(sic_core::CapOutcome::Deferred { question }) => {
                            Ok(Digest::of(question.as_bytes()))
                        }
                        Err(e) => Err(e.message.clone()),
                    },
                });
                answer
            }
            Err(e) => Err(sic_core::CapError::new(format!(
                "the broker could not read that request: {e}"
            ))),
        };
        let _ = write_frame(stream, &answer_to_bytes(&answer));
    }

    /// What the agent asked for, since the last time this was asked.
    pub fn take_tool_uses(&mut self) -> Vec<ToolUse> {
        std::mem::take(&mut self.used)
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        // A socket outliving its run would be a door into a manifest nobody is
        // enforcing any more.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Asks the broker to perform one call, from the other side.
pub fn ask(socket: &Path, request: &CapRequest) -> std::io::Result<Vec<u8>> {
    let mut body = vec![ASK_CALL];
    body.extend_from_slice(&request.to_bytes());
    exchange(socket, &body)
}

/// Asks the run what it is offering.
///
/// Asked rather than remembered: the manifest belongs to the run, and a caller
/// that cached one could answer for a run that had moved on.
pub fn list(socket: &Path) -> std::io::Result<Vec<Offered>> {
    let bytes = exchange(socket, &[ASK_LIST])?;
    offered_from_bytes(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

fn exchange(socket: &Path, body: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket)?;
    write_frame(&mut stream, body)?;
    read_frame(&mut stream).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "the run closed without answering",
        )
    })
}

fn offered_to_bytes(tools: &[Offered]) -> Vec<u8> {
    let mut w = sic_core::Writer::new();
    w.u32(tools.len() as u32);
    for tool in tools {
        w.u32(tool.index);
        w.str(&tool.cap);
        w.str(&tool.constraint);
        w.u32(tool.params.len() as u32);
        for param in &tool.params {
            w.u8(match param {
                Param::Str => 0,
                Param::Strings => 1,
            });
        }
    }
    w.finish()
}

fn offered_from_bytes(bytes: &[u8]) -> Result<Vec<Offered>, sic_core::BinError> {
    let mut r = sic_core::Reader::new(bytes);
    let count = r.count(1)?;
    let mut tools = Vec::with_capacity(count);
    for _ in 0..count {
        let index = r.u32()?;
        let cap = r.str()?;
        let constraint = r.str()?;
        let params_len = r.count(1)?;
        let mut params = Vec::with_capacity(params_len);
        for _ in 0..params_len {
            params.push(match r.u8()? {
                0 => Param::Str,
                1 => Param::Strings,
                other => {
                    return Err(sic_core::BinError::new(format!(
                        "unknown parameter kind {other}"
                    )));
                }
            });
        }
        tools.push(Offered {
            index,
            cap,
            constraint,
            params,
        });
    }
    r.expect_end("what a run offers")?;
    Ok(tools)
}

/// The answer to one call, read back.
pub fn answer(bytes: &[u8]) -> Result<sic_core::CapOutcome, sic_core::CapError> {
    match answer_from_bytes(bytes) {
        Ok(answer) => answer,
        Err(e) => Err(sic_core::CapError::new(format!(
            "the broker's answer could not be read: {e}"
        ))),
    }
}

fn write_frame(stream: &mut UnixStream, body: &[u8]) -> std::io::Result<()> {
    stream.write_all(&(body.len() as u32).to_le_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return None;
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

fn digest_of_args(request: &CapRequest) -> Digest {
    let mut hash = sic_core::Sha256::new();
    for arg in &request.args {
        hash.update(&value_bytes(arg));
    }
    hash.finish()
}

fn digest_of_value(value: &sic_core::CapValue) -> Digest {
    Digest::of(&value_bytes(value))
}

fn value_bytes(value: &sic_core::CapValue) -> Vec<u8> {
    let mut w = sic_core::Writer::new();
    value.write(&mut w);
    w.finish()
}

/// A JSON string, for the one small document this crate has to write.
///
/// Hand-written like every other serializer here. A path is the only thing it
/// ever holds, and a path can contain a quote or a backslash.
pub fn json_string(value: &str) -> String {
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
