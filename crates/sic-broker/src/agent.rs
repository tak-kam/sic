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

/// Something that can put a prompt in front of an agent and read the answer.
///
/// The broker holds one of these or none. None is not a degraded mode: it is
/// what `llm.invoke` means when nobody named a driver.
pub trait AgentDriver: std::fmt::Debug {
    /// The agent's name, which a grant has to name too.
    fn agent_name(&self) -> &str;

    fn info(&self) -> &DriverInfo;

    /// Asks, and does not return until there is a whole answer or a failure.
    fn ask(&mut self, prompt: &str) -> Result<String, CapError>;
}

/// The line an agent prints before its answer.
pub fn begin_marker(id: &str) -> String {
    format!("<<<SIC-BEGIN-{id}>>>")
}

/// The line an agent prints after it.
pub fn end_marker(id: &str) -> String {
    format!("<<<SIC-END-{id}>>>")
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
pub fn ask_text(prompt: &str, id: &str) -> String {
    format!(
        "{prompt}\n\n\
         ---\n\
         When you have the answer, print it alone between two marker lines, \
         with nothing else between them.\n\
         A marker line is `<<<S` and `IC-BEGIN-{id}>>>` joined with nothing \
         between them. The closing marker is the same, with END where BEGIN is.\n\
         Print nothing after the closing marker.\n"
    )
}

/// The answer on a captured screen, if the whole of one is there.
///
/// The last begin marker rather than the first: an agent that answered twice -
/// a retry inside its own conversation - has the answer that counts last.
pub fn answer_from(screen: &str, id: &str) -> Option<String> {
    let begin = begin_marker(id);
    let end = end_marker(id);
    let start = screen.rfind(&begin)? + begin.len();
    let stop = start + screen[start..].find(&end)?;
    Some(clean(&screen[start..stop]))
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
