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

use sic_core::{AgentAction, CapGrant, CapRequest, Digest, answer_from_bytes, answer_to_bytes};

use sic_core::reach_of;

/// The largest message either side will read. A length prefix is a promise,
/// and this is what stops one from being believed.
const MAX_FRAME: u32 = 1 << 20;

/// What is being asked of the run: what do you offer, perform this, or may the
/// agent use this tool.
const ASK_LIST: u8 = 0;
const ASK_CALL: u8 = 1;
const ASK_TOOL: u8 = 2;

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
        .filter(|(_, grant)| matches!(reach_of(grant), sic_core::Reach::Routed(_)))
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
        "process.exec" | "process.capture" | "process.run" => vec![Param::Str, Param::Strings],
        // The question, then what is being approved. An agent asking for an
        // approval has no value of the program's to show, so it fills the
        // second in with an empty string - which is exactly what the VM sends
        // for a `human.approve` call that stopped at the question.
        "human.approve" => vec![Param::Str, Param::Str],
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
    used: Vec<AgentAction>,
    /// Every tool name this manifest accounts for. Anything else is refused:
    /// see `Route::serve_tool`.
    tools: Vec<String>,
    /// How many tool uses are left for this answer, or `None` for no limit.
    /// The allowance is a number the program declared; counting it here is what
    /// makes it a bound rather than a note.
    allowance: Option<u32>,
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
            tools: Vec::new(),
            used: Vec::new(),
            allowance: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Names the tools the manifest accounts for. Everything else is refused.
    pub fn names(&mut self, tools: Vec<String>) {
        self.tools = tools;
    }

    /// Sets what this answer is allowed, before it starts.
    ///
    /// `None` is no limit; `Some(0)` is an agent that may not use a tool at
    /// all, which `serve_tool` already refuses and which no declaration could
    /// ask for until #86.
    pub fn allow(&mut self, tools: Option<u32>) {
        self.allowance = tools;
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
            Some((&ASK_TOOL, rest)) => self.serve_tool(&mut stream, rest),
            // Not a question this speaks. Saying nothing is right: the far side
            // is not a peer whose vocabulary this negotiates.
            _ => {}
        }
    }

    fn serve_call(&mut self, stream: &mut UnixStream, body: &[u8]) {
        let answer = match CapRequest::from_bytes(body) {
            Ok(request) => {
                let answer = crate::perform(&self.manifest, &request);
                self.used.push(AgentAction::Capability {
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

    /// Whether the agent may use a tool of its own.
    ///
    /// The rules the agent was started with cover the tools whose constraint a
    /// permission system can hold. This covers the one thing they cannot: a
    /// shell. `dontAsk` always permits a fixed set of read-only commands - `cat`
    /// among them - and the set is not configurable, so a rule scoping `Read`
    /// to a directory is not a bound on reading. A hook is: it runs before the
    /// rules and can refuse what they would have allowed.
    ///
    /// So the surface is decided here, by name, and everything the manifest
    /// does not account for is refused. A shell gets its own sentence because
    /// it is what an agent reaches for first and there is somewhere else for it
    /// to go: `process.exec` grants a binary, at an absolute path, sometimes
    /// pinned by digest, and the agent reaches that through this same socket
    /// where it is checked. A command string is not that and cannot be made
    /// into it.
    fn serve_tool(&mut self, stream: &mut UnixStream, body: &[u8]) {
        let mut r = sic_core::Reader::new(body);
        let (Ok(tool), Ok(input)) = (r.str(), r.str()) else {
            // Unreadable: the caller does not get a decision, and its own
            // failing closed is what happens next.
            return;
        };
        // Three reasons to refuse, and they are three different things to be
        // told. Denying by name decides the whole tool surface: the rules the
        // agent was started with are an allowlist, but some tools run whatever
        // the rules say - the read-only shell commands, and at least one that
        // was never named and ran anyway. A hook runs before any rule and can
        // refuse what they would have allowed, so the surface is decided here
        // and the rules are left to scope a path.
        let spent = self.allowance == Some(0);
        let unnamed = !self.tools.iter().any(|named| named == &tool);
        let refused = spent || unnamed;
        if let Some(left) = self.allowance.as_mut() {
            *left = left.saturating_sub(1);
        }
        let why = if spent {
            "this call has used every tool it was allowed".to_string()
        } else if matches!(tool.as_str(), "Bash" | "PowerShell") {
            // The one worth its own sentence: it is the tool an agent reaches
            // for first, and there is somewhere else for it to go.
            "no grant names a shell: a command string is not a binary, and `process.exec` is \
             offered to you as a tool instead"
                .to_string()
        } else {
            format!("the program's manifest does not account for `{tool}`")
        };

        self.used.push(AgentAction::Tool {
            tool: tool.clone(),
            input: Digest::of(input.as_bytes()),
            allowed: !refused,
            reason: match refused {
                true => why.clone(),
                false => String::new(),
            },
        });

        let mut w = sic_core::Writer::new();
        w.bool(refused);
        w.str(match refused {
            true => &why,
            false => "",
        });
        let _ = write_frame(stream, &w.finish());
    }

    /// What the agent did, since the last time this was asked.
    pub fn take_tool_uses(&mut self) -> Vec<AgentAction> {
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

/// Asks the run whether the agent may use one of its own tools.
///
/// `Ok(None)` is "nothing to say": the rules the agent was started with decide,
/// which is where a path scope belongs. `Ok(Some(reason))` is a refusal.
pub fn may_use(socket: &Path, tool: &str, input: &str) -> std::io::Result<Option<String>> {
    let mut w = sic_core::Writer::new();
    w.u8(ASK_TOOL);
    w.str(tool);
    w.str(input);
    let bytes = exchange(socket, &w.finish())?;
    let mut r = sic_core::Reader::new(&bytes);
    let bad =
        |e: sic_core::BinError| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string());
    let refused = r.bool().map_err(bad)?;
    let reason = r.str().map_err(bad)?;
    Ok(match refused {
        true => Some(reason),
        false => None,
    })
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
