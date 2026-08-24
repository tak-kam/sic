//! `sic mcp`: the capabilities a program granted, offered to the agent
//! answering for it.
//!
//! The agent starts this, so it is a child of the agent and speaks the Model
//! Context Protocol on its stdin and stdout. It performs nothing. Every call is
//! forwarded to the socket the run is listening on, where it is authorized
//! against the program's manifest and performed by the same code that performs
//! a call from the VM - which is the whole point of routing rather than
//! translating. See `docs/design/authority.md` §4.
//!
//! Two revisions of the protocol are answered. The current one is stateless and
//! opens with `server/discover`; the one before it opens with `initialize`.
//! Answering whichever arrives costs a few lines and removes a whole class of
//! failure that would show up as a pane doing nothing.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use sic_broker::route::{Offered, Param, ask, json_string, list};
use sic_core::{CapOutcome, CapRequest, CapValue};

use super::EXIT_FAILURE;

/// Where the run is listening. Passed by the driver that started the agent;
/// there is no default, because a shim that guessed would be a shim that
/// connected to somebody else's run.
pub const SOCKET_VAR: &str = "SIC_ROUTE";

/// The revision this answers with when the client did not say which it wants.
///
/// Announcing a version rather than agreeing on one is how the first attempt at
/// this failed: the shim replied `2026-07-28` to a client that speaks the
/// revision before it, and the client refused the connection - which showed up
/// as a pane where the tool simply was not there.
const REVISION: &str = "2025-11-25";

pub fn run() -> ExitCode {
    let Some(socket) = std::env::var_os(SOCKET_VAR).map(PathBuf::from) else {
        eprintln!("error: `sic mcp` is started by a run, which sets {SOCKET_VAR}");
        return ExitCode::from(EXIT_FAILURE);
    };

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = answer_line(&line, &socket) {
            // One message per line, and nothing else on stdout ever: anything
            // that is not a protocol message breaks the stream.
            if writeln!(out, "{reply}").is_err() || out.flush().is_err() {
                break;
            }
        }
    }
    ExitCode::SUCCESS
}

/// One request in, one line out - or nothing, for a notification.
fn answer_line(line: &str, socket: &std::path::Path) -> Option<String> {
    let Ok(message) = sic_json::parse(line) else {
        // No id to answer against, so this is the one error that names none.
        return Some(error(
            &sic_json::Json::Null,
            -32700,
            "the line was not JSON",
        ));
    };
    let id = message
        .member("id")
        .cloned()
        .unwrap_or(sic_json::Json::Null);
    let method = match message.member("method") {
        Some(sic_json::Json::Str(m)) => m.clone(),
        _ => return Some(error(&id, -32600, "no method")),
    };
    // A notification has no id and takes no answer. Saying nothing is the
    // protocol; answering would be a message the client cannot match.
    message.member("id")?;

    Some(match method.as_str() {
        "server/discover" => discover(&id),
        // The revision before the stateless one. Its shape differs; what it
        // means does not.
        "initialize" => initialize(&id, &message),
        "tools/list" => tools_list(&id, socket),
        "tools/call" => tools_call(&id, &message, socket),
        "ping" => result(&id, "{}".into()),
        other => error(&id, -32601, &format!("no method `{other}`")),
    })
}

fn discover(id: &sic_json::Json) -> String {
    result(
        id,
        format!(
            "{{\"resultType\":\"complete\",\"supportedVersions\":[{}],\
             \"capabilities\":{{\"tools\":{{}}}},\
             \"_meta\":{{\"io.modelcontextprotocol/serverInfo\":{}}}}}",
            "\"2026-07-28\",\"2025-11-25\"",
            server_info()
        ),
    )
}

/// The older handshake, where the client says which revision it wants.
///
/// Its version is echoed back. This server has no revision-specific behaviour -
/// three methods, and the shape of a tool has not changed - so agreeing with
/// the client is both true and the only answer that connects.
fn initialize(id: &sic_json::Json, message: &sic_json::Json) -> String {
    let wanted = match message
        .member("params")
        .and_then(|p| p.member("protocolVersion"))
    {
        Some(sic_json::Json::Str(v)) => v.clone(),
        _ => REVISION.to_string(),
    };
    result(
        id,
        format!(
            "{{\"protocolVersion\":{},\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{}}}",
            json_string(&wanted),
            server_info()
        ),
    )
}

fn server_info() -> String {
    format!(
        "{{\"name\":\"sic\",\"version\":{}}}",
        json_string(crate::VERSION)
    )
}

/// What the run says it is offering.
///
/// Asked every time rather than cached: the manifest belongs to the run, and a
/// shim that remembered one would be a shim that could answer for a run that
/// had moved on.
fn tools_list(id: &sic_json::Json, socket: &std::path::Path) -> String {
    let offered = match list(socket) {
        Ok(tools) => tools,
        Err(e) => return error(id, -32603, &format!("the run did not answer: {e}")),
    };
    let tools: Vec<String> = offered.iter().map(tool_json).collect();
    result(
        id,
        format!(
            "{{\"resultType\":\"complete\",\"tools\":[{}]}}",
            tools.join(",")
        ),
    )
}

fn tool_json(tool: &Offered) -> String {
    let mut properties = Vec::new();
    let mut required = Vec::new();
    for (i, param) in tool.params.iter().enumerate() {
        let name = param_name(&tool.cap, i);
        let schema = match param {
            Param::Str => "{\"type\":\"string\"}".to_string(),
            Param::Strings => "{\"type\":\"array\",\"items\":{\"type\":\"string\"}}".to_string(),
        };
        properties.push(format!("{}:{schema}", json_string(name)));
        required.push(json_string(name));
    }
    format!(
        "{{\"name\":{},\"description\":{},\"inputSchema\":{{\"type\":\"object\",\
         \"properties\":{{{}}},\"required\":[{}],\"additionalProperties\":false}}}}",
        json_string(&tool.tool_name()),
        json_string(&format!(
            "The program's `{}` capability, granted for {:?}. Performed by sic \
             against the program's manifest, not by you.",
            tool.cap, tool.constraint
        )),
        properties.join(","),
        required.join(",")
    )
}

/// What a capability's parameters are called, for whoever is filling them in.
fn param_name(cap: &str, index: usize) -> &'static str {
    match (cap, index) {
        ("process.exec" | "process.capture", 0) => "path",
        ("process.exec" | "process.capture", 1) => "args",
        ("human.approve" | "human.choose", 0) => "question",
        ("human.choose", 1) => "options",
        _ => "argument",
    }
}

fn tools_call(id: &sic_json::Json, message: &sic_json::Json, socket: &std::path::Path) -> String {
    let params = message.member("params");
    let name = match params.and_then(|p| p.member("name")) {
        Some(sic_json::Json::Str(name)) => name.clone(),
        _ => return error(id, -32602, "no tool name"),
    };
    let offered = match list(socket) {
        Ok(tools) => tools,
        Err(e) => return error(id, -32603, &format!("the run did not answer: {e}")),
    };
    let Some(tool) = offered.iter().find(|t| t.tool_name() == name) else {
        return error(id, -32602, &format!("no tool `{name}`"));
    };

    let arguments = params.and_then(|p| p.member("arguments"));
    let mut args = Vec::new();
    for (i, param) in tool.params.iter().enumerate() {
        let field = arguments.and_then(|a| a.member(param_name(&tool.cap, i)));
        match (param, field) {
            (Param::Str, Some(sic_json::Json::Str(s))) => args.push(CapValue::Str(s.clone())),
            (Param::Strings, Some(sic_json::Json::Array(items))) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        sic_json::Json::Str(s) => out.push(s.clone()),
                        _ => return failed(id, "every element of that array has to be a string"),
                    }
                }
                args.push(CapValue::List(out));
            }
            // An argument of the wrong shape is the caller's mistake and it can
            // try again, so it is a failed call rather than a broken protocol.
            _ => {
                return failed(
                    id,
                    &format!(
                        "`{}` is missing or the wrong type",
                        param_name(&tool.cap, i)
                    ),
                );
            }
        }
    }

    let request = CapRequest {
        index: tool.index,
        name: tool.cap.clone(),
        args,
        task: 0,
        attempt: 1,
        timeout_ms: 0,
        conversation: 0,
        // Neither applies to a call the agent makes: the allowance is what let
        // this tool exist at all, and the deadline bounds the answer this call
        // is part of rather than the call.
        tools_left: 0,
        answer_ms: 0,
    };
    match ask(socket, &request).map(|bytes| sic_broker::route::answer(&bytes)) {
        Ok(Ok(CapOutcome::Value(value))) => result(
            id,
            format!(
                "{{\"resultType\":\"complete\",\"content\":[{{\"type\":\"text\",\"text\":{}}}],\
                 \"isError\":false}}",
                json_string(&rendered(&value))
            ),
        ),
        // A capability that cannot answer within the call suspends the run when
        // a program asks for it. There is no run to suspend here.
        Ok(Ok(CapOutcome::Deferred { question })) => failed(
            id,
            &format!("that has to be answered by a person, who was asked: {question}"),
        ),
        Ok(Err(error)) => failed(id, &error.message),
        Err(e) => error(id, -32603, &format!("the run did not answer: {e}")),
    }
}

/// A value as the agent reads it.
fn rendered(value: &CapValue) -> String {
    match value {
        CapValue::Unit => "done".into(),
        CapValue::Bool(v) => v.to_string(),
        CapValue::I64(v) => v.to_string(),
        CapValue::F64(v) => format!("{v}"),
        CapValue::Str(s) => s.clone(),
        CapValue::List(items) => items.join("\n"),
    }
}

/// A call that failed, which the caller may retry differently. Distinct from a
/// protocol error, which is not something the caller can act on.
fn failed(id: &sic_json::Json, message: &str) -> String {
    result(
        id,
        format!(
            "{{\"resultType\":\"complete\",\"content\":[{{\"type\":\"text\",\"text\":{}}}],\
             \"isError\":true}}",
            json_string(message)
        ),
    )
}

fn result(id: &sic_json::Json, body: String) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{body}}}",
        id_json(id)
    )
}

fn error(id: &sic_json::Json, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":{}}}}}",
        id_json(id),
        json_string(message)
    )
}

/// An id is echoed as it arrived: a number stays a number and a string stays a
/// string, because the client matches on it.
fn id_json(id: &sic_json::Json) -> String {
    match id {
        sic_json::Json::Int(v) => v.to_string(),
        sic_json::Json::Str(s) => json_string(s),
        _ => "null".into(),
    }
}
