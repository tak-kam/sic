//! Driving an agent CLI in a tmux pane.
//!
//! One call, one pane: a window is opened in the run's session, the prompt is
//! pasted in, the pane is read until the answer is complete, and the window is
//! killed. See `docs/design/driving.md` §3.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sic_core::CapError;

use crate::agent::{
    AgentDriver, Ask, DriverInfo, Thread, answer_from, ask_text, check_size, new_marker_id,
};

/// Where tmux is looked for. Absolute paths only: `PATH` decides what is on a
/// machine, and what a run did should not.
const TMUX: &[&str] = &[
    "/usr/bin/tmux",
    "/bin/tmux",
    "/usr/local/bin/tmux",
    "/opt/homebrew/bin/tmux",
];

/// Directories an agent named without a path is looked for in, relative to
/// `HOME` first and then absolute. A compiled-in list, not a `PATH` search.
const IN_HOME: &[&str] = &[".local/bin", ".claude/local", ".bun/bin", ".npm-global/bin"];
const IN_ROOT: &[&str] = &["/usr/local/bin", "/usr/bin", "/opt/homebrew/bin"];

/// What the agent is allowed to inherit.
///
/// `HOME` because that is where its own login lives, `PATH` because it runs
/// tools with it. No credential variable is on this list: the agent
/// authenticates as itself, and sic never holds the secret it uses.
const KEEP_ENV: &[&str] = &[
    "HOME", "PATH", "TERM", "LANG", "LC_ALL", "LC_CTYPE", "USER", "SHELL", "TMPDIR",
];

/// The socket the driver's tmux server listens on. Its own, so that the
/// environment and the configuration of the panes sic drives are sic's.
const SOCKET: &str = "sic";

/// The pane's size. Wide, because a narrower pane wraps more and every wrap is
/// something `capture-pane -J` has to put back together.
const COLUMNS: &str = "200";
const ROWS: &str = "50";

/// How long the agent has to print anything at all before it is called broken.
const READY_DEADLINE: Duration = Duration::from_secs(60);
/// How long a whole answer may take.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30 * 60);
/// How often the pane is read. A person is not watching this number; an agent
/// takes seconds at best.
const POLL: Duration = Duration::from_millis(750);
/// A pause after the agent's first output, so that a prompt is not pasted into
/// an interface that is still drawing itself.
const SETTLE: Duration = Duration::from_millis(1500);

/// What the driver is being opened for.
#[derive(Debug, Clone)]
pub struct Session {
    /// The run. The tmux session is named after it, so a run continued in
    /// another process finds its own panes without anything having to be
    /// looked up.
    pub run: String,
    /// Whether this run is being continued rather than started. It decides what
    /// a missing pane means: nothing on a fresh run, and a lost conversation on
    /// one that is coming back.
    pub continuing: bool,
    /// Where the driver may write down which conversations it opened. Without
    /// one, a conversation cannot outlive the process.
    pub state: Option<PathBuf>,
}

#[derive(Debug)]
pub struct TmuxDriver {
    tmux: PathBuf,
    agent: PathBuf,
    name: String,
    info: DriverInfo,
    session: String,
    continuing: bool,
    state: Option<PathBuf>,
    /// The conversations this run has open, as `(conversation, task)`.
    open: Vec<(u32, u32)>,
}

impl TmuxDriver {
    /// Opens the driver named by a spec, `tmux:claude`.
    ///
    /// Everything that can be checked before a run starts is checked here: that
    /// tmux exists, that the agent exists, and what version each of them is.
    /// A run that is going to fail for want of a tool should fail before it has
    /// done anything.
    pub fn open(spec: &str, session: Session) -> Result<TmuxDriver, CapError> {
        let (multiplexer, agent) = spec.split_once(':').ok_or_else(|| {
            CapError::new(format!(
                "`{spec}` is not a driver: write `<multiplexer>:<agent>`, as in `tmux:claude`"
            ))
        })?;
        if multiplexer != "tmux" {
            return Err(CapError::new(format!(
                "`{multiplexer}` is not a multiplexer this knows; `tmux` is the only one"
            )));
        }
        if agent.is_empty() {
            return Err(CapError::new(format!("`{spec}` names no agent")));
        }

        let tmux = find(TMUX.iter().map(PathBuf::from), "tmux")?;
        let path = resolve_agent(agent)?;
        // The name a grant has to match is the name that was asked for, or the
        // file name when a path was given.
        let name = match agent.starts_with('/') {
            true => file_name(&path),
            false => agent.to_string(),
        };

        let info = DriverInfo {
            driver: spec.to_string(),
            command: path.display().to_string(),
            agent: first_line(&version_of(&path, "--version")?),
            multiplexer: first_line(&version_of(&tmux, "-V")?),
        };
        // Read before anything is opened: what a run had open last time is how
        // a conversation that is gone can be told from one that never existed.
        let open = match &session.state {
            Some(path) => read_state(path),
            None => Vec::new(),
        };
        Ok(TmuxDriver {
            tmux,
            agent: path,
            name,
            info,
            session: format!("sic-{}", &session.run[..session.run.len().min(12)]),
            continuing: session.continuing,
            state: session.state,
            open,
        })
    }

    /// The run's session, made if this is the first call, with `window` as its
    /// first window.
    fn ensure_session(&self, window: &str) -> Result<(), CapError> {
        if self.tmux(&["has-session", "-t", &self.session]).is_ok() {
            return self.new_window(window);
        }
        // The pane starts where `sic` was started. A coding agent reads the
        // directory it is in, so this decides what it can see, and leaving it
        // to whatever tmux picked would leave that to chance.
        let here = self.here()?;
        self.tmux(&[
            "new-session",
            "-d",
            "-s",
            &self.session,
            "-n",
            window,
            "-c",
            &here,
            "-x",
            COLUMNS,
            "-y",
            ROWS,
            &sh_quote(&self.agent),
        ])?;
        Ok(())
    }

    fn new_window(&self, window: &str) -> Result<(), CapError> {
        let here = self.here()?;
        self.tmux(&[
            "new-window",
            "-d",
            "-t",
            &self.session,
            "-n",
            window,
            "-c",
            &here,
            &sh_quote(&self.agent),
        ])?;
        Ok(())
    }

    fn here(&self) -> Result<String, CapError> {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .map_err(|e| CapError::new(format!("cannot tell where this is running: {e}")))
    }

    fn window_exists(&self, window: &str) -> bool {
        match self.tmux(&["list-windows", "-F", "#{window_name}", "-t", &self.session]) {
            Ok(names) => names.lines().any(|n| n == window),
            Err(_) => false,
        }
    }

    /// Remembers that a conversation is open, so that a later process can tell
    /// a pane that was closed from one that was never made.
    fn remember(&mut self, thread: Thread) -> Result<(), CapError> {
        let entry = (thread.conversation, thread.task);
        if !self.open.contains(&entry) {
            self.open.push(entry);
        }
        let Some(path) = &self.state else {
            return Ok(());
        };
        let mut text = String::new();
        for (conversation, task) in &self.open {
            text.push_str(&format!("{conversation} {task}\n"));
        }
        std::fs::write(path, text).map_err(|e| {
            CapError::new(format!(
                "cannot record the conversation in `{}`: {e}",
                path.display()
            ))
        })
    }

    /// One tmux command, with the environment cut down to the allowlist.
    fn tmux(&self, args: &[&str]) -> Result<String, CapError> {
        let mut command = Command::new(&self.tmux);
        command.args(server_args()).args(args);
        cleaned_env(&mut command);
        let out = command
            .output()
            .map_err(|e| CapError::new(format!("cannot run tmux: {e}")))?;
        if !out.status.success() {
            return Err(CapError::new(format!(
                "tmux {}: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Puts text in a tmux buffer without it passing through a shell.
    ///
    /// A prompt is text a program built, so it may contain anything. It reaches
    /// the pane as a paste rather than as keys, because `send-keys` reads names
    /// like `Enter` and `C-c` out of what it is given.
    fn load_buffer(&self, buffer: &str, text: &str) -> Result<(), CapError> {
        let mut child = Command::new(&self.tmux)
            .args(server_args())
            .args(["load-buffer", "-b", buffer, "-"])
            .stdin(Stdio::piped())
            .env_clear()
            .envs(kept_env())
            .spawn()
            .map_err(|e| CapError::new(format!("cannot run tmux: {e}")))?;
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(text.as_bytes())
            .map_err(|e| CapError::new(format!("cannot write the prompt to tmux: {e}")))?;
        let status = child
            .wait()
            .map_err(|e| CapError::new(format!("cannot wait for tmux: {e}")))?;
        if !status.success() {
            return Err(CapError::new("tmux load-buffer failed".to_string()));
        }
        Ok(())
    }

    fn screen(&self, session: &str) -> Result<String, CapError> {
        let text = self.tmux(&["capture-pane", "-p", "-J", "-S", "-", "-t", session])?;
        check_size(&text)?;
        Ok(text)
    }

    /// Waits for the agent to draw something, then for it to settle.
    fn wait_ready(&self, session: &str) -> Result<(), CapError> {
        let deadline = Instant::now() + READY_DEADLINE;
        loop {
            if !self.screen(session)?.trim().is_empty() {
                std::thread::sleep(SETTLE);
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(CapError::new(format!(
                    "`{}` printed nothing within {}s, so it is not ready to be asked",
                    self.agent.display(),
                    READY_DEADLINE.as_secs()
                )));
            }
            std::thread::sleep(POLL);
        }
    }

    /// The whole of one call, once the pane exists.
    fn converse(&self, target: &str, ask: Ask<'_>, fresh: bool) -> Result<String, CapError> {
        let id = new_marker_id();
        // A pane that has answered before is ready by definition; only one that
        // has just been started has to be waited for.
        if fresh {
            self.wait_ready(target)?;
        }

        let buffer = format!("sic-{id}");
        self.load_buffer(&buffer, &ask_text(ask.prompt, &id, ask.json))?;
        // `-p` pastes in bracketed-paste mode, so an interface that knows about
        // pastes takes the newlines as text rather than as twenty submissions.
        self.tmux(&["paste-buffer", "-p", "-d", "-b", &buffer, "-t", target])?;
        self.tmux(&["send-keys", "-t", target, "Enter"])?;

        let deadline = Instant::now() + ANSWER_DEADLINE;
        loop {
            if let Some(answer) = answer_from(&self.screen(target)?, &id, ask.json) {
                return Ok(answer);
            }
            if Instant::now() >= deadline {
                return Err(CapError::new(format!(
                    "`{}` did not finish an answer within {} minutes",
                    self.name,
                    ANSWER_DEADLINE.as_secs() / 60
                )));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl AgentDriver for TmuxDriver {
    fn agent_name(&self) -> &str {
        &self.name
    }

    fn info(&self) -> &DriverInfo {
        &self.info
    }

    fn ask(&mut self, ask: Ask<'_>) -> Result<String, CapError> {
        match ask.thread.remembers() {
            true => self.ask_remembering(ask),
            false => self.ask_once(ask),
        }
    }

    fn finish(&mut self, waiting: bool) {
        // A run that stopped to be continued keeps whatever it was holding: it
        // will come back, and it should not come back to a stranger. A run that
        // is over keeps nothing, because the journal already holds the prompts
        // and the answers, and what a pane has beyond that is context nobody
        // can ask for any more.
        if waiting {
            return;
        }
        let _ = self.tmux(&["kill-session", "-t", &self.session]);
        if let Some(path) = &self.state {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl TmuxDriver {
    /// A call with no memory: a window of its own, closed when the answer is
    /// in. The journal already holds the prompt and the answer, so the pane
    /// keeps nothing the record does not.
    fn ask_once(&mut self, ask: Ask<'_>) -> Result<String, CapError> {
        let window = format!("ask-{}", new_marker_id());
        self.ensure_session(&window)?;
        let target = format!("{}:{window}", self.session);
        let answer = self.converse(&target, ask, true);
        let _ = self.tmux(&["kill-window", "-t", &target]);
        answer
    }

    /// A call that continues a conversation: one window per conversation and
    /// task, kept for as long as the run can still come back to it.
    fn ask_remembering(&mut self, ask: Ask<'_>) -> Result<String, CapError> {
        let thread = ask.thread;
        let window = format!("c{}t{}", thread.conversation, thread.task);
        let target = format!("{}:{window}", self.session);
        let mut fresh = false;
        if !self.window_exists(&window) {
            // A conversation this run is known to have opened, whose pane is
            // not there, is one that cannot be continued. Starting a fresh one
            // would change what the run means without saying so.
            if self.continuing && self.open.contains(&(thread.conversation, thread.task)) {
                return Err(CapError::new(format!(
                    "the conversation this run was holding for task {} is gone: its pane was \
                     closed, or the machine it was on restarted. It cannot be continued",
                    thread.task
                )));
            }
            self.ensure_session(&window)?;
            self.remember(thread)?;
            fresh = true;
        }
        self.converse(&target, ask, fresh)
    }
}

/// The conversations a run had open, as it left them.
pub(crate) fn read_state(path: &Path) -> Vec<(u32, u32)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let conversation = parts.next()?.parse().ok()?;
            let task = parts.next()?.parse().ok()?;
            Some((conversation, task))
        })
        .collect()
}

/// The options that come before any tmux command.
///
/// Its own socket, because `new-session` in somebody else's server runs in that
/// server's environment; no configuration file, because a status line or a
/// changed default shell changes what `capture-pane` returns; `-u` because
/// tmux otherwise decides whether the pane is UTF-8 from the locale it happens
/// to have been started with, and an answer that depends on that is an answer
/// that arrives as mojibake on a machine with no `LANG`.
fn server_args() -> [&'static str; 5] {
    ["-u", "-L", SOCKET, "-f", "/dev/null"]
}

fn kept_env() -> Vec<(String, String)> {
    KEEP_ENV
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|v| ((*name).to_string(), v)))
        .collect()
}

fn cleaned_env(command: &mut Command) {
    command.env_clear();
    command.envs(kept_env());
}

/// The first of these paths that exists.
fn find(candidates: impl Iterator<Item = PathBuf>, what: &str) -> Result<PathBuf, CapError> {
    let mut looked = Vec::new();
    for path in candidates {
        if path.is_file() {
            return Ok(path);
        }
        looked.push(path.display().to_string());
    }
    Err(CapError::new(format!(
        "no `{what}` at any of {}",
        looked.join(", ")
    )))
}

/// Where an agent named without a path is looked for.
fn resolve_agent(agent: &str) -> Result<PathBuf, CapError> {
    if agent.starts_with('/') {
        let path = PathBuf::from(agent);
        if !path.is_file() {
            return Err(CapError::new(format!("there is no `{agent}`")));
        }
        return Ok(path);
    }
    if agent.contains('/') {
        return Err(CapError::new(format!(
            "`{agent}` is a relative path; name the agent, or give an absolute path"
        )));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let candidates = IN_HOME
        .iter()
        .filter_map(move |dir| home.as_ref().map(|h| h.join(dir).join(agent)))
        .chain(IN_ROOT.iter().map(|dir| Path::new(dir).join(agent)));
    find(candidates, agent)
}

/// What a tool says it is. A tool that cannot say fails the driver rather than
/// being recorded as unknown: reading its output is a bet on its version.
fn version_of(tool: &Path, flag: &str) -> Result<String, CapError> {
    let mut command = Command::new(tool);
    command.arg(flag);
    cleaned_env(&mut command);
    let out = command
        .output()
        .map_err(|e| CapError::new(format!("cannot run `{} {flag}`: {e}", tool.display())))?;
    if !out.status.success() {
        return Err(CapError::new(format!(
            "`{} {flag}` failed: {}",
            tool.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One argument for the shell tmux runs a pane's command with.
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}
