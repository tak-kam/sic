//! Answering `llm.invoke` by driving an agent CLI, instead of deferring it.
//!
//! What a driver is, and how an answer is recognised on a screen. The tmux
//! driver itself is in `tmux.rs`; everything here is a pure function of text,
//! because the protocol is the part worth testing and a pane is not.
//!
//! See `docs/design/driving.md`.

use std::sync::atomic::{AtomicU64, Ordering};

use sic_core::{CapError, Sha256};

/// The most a pane may hold before an answer is refused rather than truncated.
/// The same limit, for the same reason, as `process.capture`.
pub const MAX_ANSWER: usize = 1 << 20;

/// The characters a terminal user interface draws its frame with.
///
/// None of them can begin a line of an answer, which is what makes stripping
/// them safe. A `>` or a `#` would not qualify: those start quotes and
/// headings.
const DECORATION: &[char] = &['⏺', '⎿', '│', '╭', '╰', '╮', '╯', '─', '▌', '·', '•'];

/// What answered a run's model calls.
///
/// Reading a terminal user interface is a bet on what one version of one tool
/// prints, so a recorded run keeps which version that was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverInfo {
    /// The spec as it was asked for, `tmux:claude`.
    pub driver: String,
    /// The absolute path the agent was found at.
    pub command: String,
    /// What the agent says it is.
    pub agent: String,
    /// What the multiplexer says it is.
    pub multiplexer: String,
}

/// Which conversation a call belongs to.
///
/// The pair is the identity: the conversation number says which caller keeps
/// it, and the task says which of that caller's conversations this is. Two
/// agents that each remember must not end up in the same one, and neither must
/// the same agent running in two tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thread {
    pub task: u32,
    /// 0 for a call that starts fresh every time, which is the default and is
    /// what an agent without `memory: task` means.
    pub conversation: u32,
}

impl Thread {
    /// Whether anything has to be kept after the answer arrives.
    pub fn remembers(&self) -> bool {
        self.conversation != 0
    }
}

/// One question, and everything about how to ask it.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    pub prompt: &'a str,
    pub thread: Thread,
    /// Whether the answer has to be JSON, because the caller said what shape it
    /// must take. It decides how a wrapped line is put back together - see
    /// `fold` - so it is part of asking rather than part of the prompt.
    pub json: bool,
}

/// Something that can put a prompt in front of an agent and read the answer.
///
/// The broker holds one of these or none. None is not a degraded mode: it is
/// what `llm.invoke` means when nobody named a driver.
pub trait AgentDriver: std::fmt::Debug {
    /// The agent's name, which a grant has to name too.
    fn agent_name(&self) -> &str;

    fn info(&self) -> &DriverInfo;

    /// Asks, and does not return until there is a whole answer or a failure.
    fn ask(&mut self, ask: Ask<'_>) -> Result<String, CapError>;

    /// What the agent asked the broker to do while answering, since the last
    /// time this was asked.
    ///
    /// A driver that offers the agent nothing answers with nothing, which is
    /// why this has a default: it is not every driver's business.
    fn take_tool_uses(&mut self) -> Vec<sic_core::ToolUse> {
        Vec::new()
    }

    /// The run is over.
    ///
    /// `waiting` means it stopped to be continued later, so anything holding a
    /// conversation has to survive this process - a run that comes back should
    /// not come back to a stranger.
    fn finish(&mut self, waiting: bool);
}

/// The line an agent prints before its answer.
pub fn begin_marker(id: &str) -> String {
    format!("<<<SIC-BEGIN-{id}>>>")
}

/// The line an agent prints after it.
pub fn end_marker(id: &str) -> String {
    format!("<<<SIC-END-{id}>>>")
}

/// What is actually looked for on the screen.
///
/// Less than the whole marker, and deliberately: the instructions spell the
/// marker as `<<<S` and `IC-BEGIN-{id}>>>` to be joined, so the split falls
/// between the `S` and the `IC`. Any screen holding `SIC-BEGIN-{id}` as one
/// piece therefore holds something an agent joined - the echo cannot.
///
/// Searching for this rather than for the whole marker costs nothing and buys
/// the case that turned up the first time an agent used a tool and then
/// answered: it printed `<<SIC-BEGIN-...>>`, having lost an angle bracket in
/// the joining. The answer was right and the run waited half an hour for a
/// marker that was three characters away from the one it wanted.
fn begin_needle(id: &str) -> String {
    format!("SIC-BEGIN-{id}")
}

fn end_needle(id: &str) -> String {
    format!("SIC-END-{id}")
}

/// A fresh id, so that a marker left on screen by an earlier answer cannot end
/// this one.
pub fn new_marker_id() -> String {
    let mut hash = Sha256::new();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hash.update(&nanos.to_le_bytes());
    hash.update(&(std::process::id() as u64).to_le_bytes());
    // A counter, because two calls in the same process can land in the same
    // nanosecond and two ids that are equal would end each other's answers.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    hash.update(&NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.finish().hex()[..8].to_string()
}

/// What is typed into the pane: the prompt, and how to mark the answer.
///
/// The markers are spelled in two pieces to be joined, and that is not
/// decoration. Whatever is typed into a pane is echoed back into it, so
/// instructions containing the literal marker would put a complete-looking
/// answer on screen before the agent had answered anything. Spelled this way,
/// "the marker appeared" and "the agent printed it" are the same statement.
pub fn ask_text(prompt: &str, id: &str, json: bool) -> String {
    // An interface that wraps a long line puts a line break inside whatever it
    // was drawing, so an answer that has to survive one is asked for on a
    // single line. `fold` puts back together what happens anyway.
    let shape = match json {
        true => "Print the JSON on a single line.\n",
        false => "",
    };
    format!(
        "{prompt}\n\n\
         ---\n\
         When you have the answer, print it alone between two marker lines, \
         with nothing else between them.\n\
         A marker line is `<<<S` and `IC-BEGIN-{id}>>>` joined with nothing \
         between them. The closing marker is the same, with END where BEGIN is.\n\
         {shape}\
         Print nothing after the closing marker.\n"
    )
}

/// The answer on a captured screen, if the whole of one is there.
///
/// The last begin marker rather than the first: an agent that answered twice -
/// a retry inside its own conversation - has the answer that counts last.
pub fn answer_from(screen: &str, id: &str, json: bool) -> Option<String> {
    // The answer is the lines strictly between the two marker lines, which is
    // what the instructions ask for. Taking whole lines is also what makes the
    // rest of a marker line - the brackets, and whatever the interface drew
    // around them - not part of it.
    let found = screen.rfind(&begin_needle(id))?;
    let start = screen[found..].find('\n').map(|n| found + n + 1)?;
    let ends = start + screen[start..].find(&end_needle(id))?;
    let stop = screen[..ends].rfind('\n').unwrap_or(ends);
    let text = clean(&screen[start..stop]);
    Some(match json {
        true => fold(&text),
        false => text,
    })
}

/// Puts a wrapped line back together.
///
/// A terminal user interface draws an answer at the width it has, and a line
/// too long for that comes back with a break in the middle of whatever it was
/// drawing - inside a JSON string, where a literal newline is not even legal.
/// Nothing on the screen says which breaks are the answer's and which are the
/// interface's.
///
/// For JSON there is no need to tell them apart: the grammar requires
/// whitespace nowhere, and a newline inside a string is invalid, so joining
/// every line with nothing between them repairs a wrap exactly and leaves a
/// document that was already whole unchanged. Prose has no such property, which
/// is why this is not done to it - and why an answer meant to be read by a
/// person comes back with the interface's line breaks in it.
fn fold(text: &str) -> String {
    text.lines().collect::<Vec<&str>>().join("")
}

/// Strips what the interface drew from what the agent said.
///
/// Leading whitespace goes with the decoration, so indentation inside an answer
/// is not preserved. What an `agent` declaration reads back is JSON, where that
/// does not matter, and a driver that guessed at which spaces were the frame's
/// and which were the answer's would be worse than one that says so.
fn clean(region: &str) -> String {
    let mut out = String::new();
    for line in region.lines() {
        let line = line.trim_end();
        let stripped = line.trim_start_matches(|c: char| c.is_whitespace() || is_decoration(c));
        // A line that is nothing but frame is the input box, not a blank line
        // in the answer.
        if stripped.is_empty() && !line.is_empty() {
            continue;
        }
        out.push_str(stripped);
        out.push('\n');
    }
    out.trim_matches('\n').to_string()
}

fn is_decoration(c: char) -> bool {
    DECORATION.contains(&c)
}

/// Refuses a screen too large to be an answer.
pub fn check_size(screen: &str) -> Result<(), CapError> {
    if screen.len() > MAX_ANSWER {
        return Err(CapError::new(format!(
            "the pane holds {} bytes, over the {MAX_ANSWER} byte limit",
            screen.len()
        )));
    }
    Ok(())
}
