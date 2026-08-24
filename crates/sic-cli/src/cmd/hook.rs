//! `sic hook`: what the agent may do with its own tools.
//!
//! The agent runs this before every tool call. It decides nothing itself - it
//! asks the run, which decides against the program's manifest - and it fails
//! closed, because a gate that opens when it cannot reach its authority is not
//! a gate. See `docs/design/authority.md` §7.
//!
//! Exit 2 is the mechanism, and the only one that works: a hook that returns a
//! `deny` decision does not override an allow rule, and `dontAsk` always permits
//! a fixed set of read-only shell commands whatever the rules say. Exiting 2
//! blocks before any rule is consulted, which is how reading gets bounded at
//! all.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use super::mcp::SOCKET_VAR;

/// What the agent is told when the tool is refused. It reaches the agent, so it
/// is written for whoever has to act on it.
const BLOCKED: u8 = 2;

pub fn run() -> ExitCode {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return refuse("sic could not read what tool this is about");
    }
    let Some(socket) = std::env::var_os(SOCKET_VAR).map(PathBuf::from) else {
        return refuse("sic is not reachable: this hook was run outside a run");
    };

    let Ok(message) = sic_json::parse(&input) else {
        return refuse("sic could not read what tool this is about");
    };
    let tool = match message.member("tool_name") {
        Some(sic_json::Json::Str(name)) => name.clone(),
        _ => return refuse("sic could not read what tool this is about"),
    };
    // What the call was about, rendered back. What it means is the agent's
    // vocabulary rather than sic's, so it is passed along whole instead of
    // being interpreted here - and the rest of the payload, which is about the
    // session and not about the tool, is left out.
    let detail = match message.member("tool_input") {
        Some(value) => rendered(value),
        None => String::new(),
    };

    match sic_broker::route::may_use(&socket, &tool, &detail) {
        // Nothing to say: the rules the agent was started with decide, which is
        // where a path scope belongs.
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(reason)) => refuse(&format!("sic refused it: {reason}")),
        // The distinction the run has to be able to make. Both block; only one
        // of them means the manifest said no.
        Err(e) => refuse(&format!(
            "sic could not be reached, so nothing authorized this ({e})"
        )),
    }
}

fn refuse(reason: &str) -> ExitCode {
    eprintln!("{reason}");
    ExitCode::from(BLOCKED)
}

/// A parsed value written back out.
///
/// `sic-json` reads and does not write, because until now nothing needed it to.
/// This is small enough to keep here rather than growing that crate for one
/// caller: what it produces is never read back, only digested and shown.
fn rendered(value: &sic_json::Json) -> String {
    use sic_json::Json;
    match value {
        Json::Null => "null".into(),
        Json::Bool(v) => v.to_string(),
        Json::Int(v) => v.to_string(),
        Json::Float(v) => format!("{v}"),
        Json::Str(s) => sic_json::quoted(s),
        Json::Array(items) => {
            let parts: Vec<String> = items.iter().map(rendered).collect();
            format!("[{}]", parts.join(","))
        }
        Json::Object(members) => {
            let parts: Vec<String> = members
                .iter()
                .map(|(name, value)| format!("{}:{}", sic_json::quoted(name), rendered(value)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}
