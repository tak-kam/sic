//! The capability broker.
//!
//! This is the only crate in the workspace that touches the outside world. It
//! receives a request, decides whether the manifest allows it, performs it, and
//! returns a value.
//!
//! Authorization happens here even though the compiler already checked that the
//! module declared the capability. The broker does not trust the bytecode it is
//! serving: once these are two processes, the manifest is the whole contract
//! between them, and a check that only runs on the VM's side is not a check.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use sic_core::{AgentAction, CapError, CapGrant, CapOutcome, CapRequest, CapValue, Sha256};

pub mod agent;
// Driving an agent needs a unix socket and it needs tmux, and neither is a
// thing on Windows. They were not conditional and the tree stopped compiling
// for `x86_64-pc-windows-msvc` the day `route` was added - which nothing
// noticed, because no CI job compiled for a target the release publishes.
//
// `AgentDriver` itself is portable and stays: a driver is a trait, and the two
// implementations that need a unix are what is conditional.
#[cfg(unix)]
pub mod route;
#[cfg(unix)]
pub mod tmux;

pub use agent::{AgentDriver, Ask, DriverInfo, Thread};
#[cfg(unix)]
pub use route::{Offered, Param, Route};
// Re-exported because every caller that has a broker also wants these, and the
// classification is pure enough to live at the boundary rather than here.
pub use sic_core::{Authority, Reach, Refused, Rule, authority_of};
#[cfg(unix)]
pub use tmux::TmuxDriver;

/// The largest file `fs.read` will return. A capability that can exhaust the
/// host's memory is not much of a boundary.
pub const MAX_READ_BYTES: u64 = 1 << 20;

/// How often a timed-out child process is checked. Small enough that a deadline
/// is roughly honoured, large enough that waiting is not a spin.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The most a program may print into a value.
///
/// A capability that reads unbounded output is a way to exhaust memory through
/// something that looks like it just runs `git`. Crossing this fails the call
/// rather than truncating: see `docs/design/output.md`.
const MAX_OUTPUT: usize = 1 << 20;

/// How much of a file is hashed at a time. An executable can be large, and
/// reading one into memory to check it would be its own problem.
const HASH_CHUNK: usize = 64 * 1024;

#[derive(Debug)]
pub struct Broker {
    manifest: Vec<CapGrant>,
    /// What answers `llm.invoke`, when anything does. `None` is not a degraded
    /// mode: it is what the capability means when nobody named a driver.
    driver: Option<Box<dyn AgentDriver>>,
}

impl Broker {
    pub fn new(manifest: Vec<CapGrant>) -> Self {
        Self {
            manifest,
            driver: None,
        }
    }

    /// A broker that can put a prompt in front of an agent instead of
    /// suspending the run so a person can paste the answer in.
    pub fn with_driver(manifest: Vec<CapGrant>, driver: Box<dyn AgentDriver>) -> Self {
        Self {
            manifest,
            driver: Some(driver),
        }
    }

    pub fn manifest(&self) -> &[CapGrant] {
        &self.manifest
    }

    /// Tells whatever is answering model calls that the run is over.
    ///
    /// `waiting` means the run stopped to be continued later, which is the one
    /// case where a conversation has to outlive the process holding it.
    pub fn finish(&mut self, waiting: bool) {
        if let Some(driver) = self.driver.as_mut() {
            driver.finish(waiting);
        }
    }

    /// Performs one capability call.
    ///
    /// The result is `Deferred` when the effect cannot answer within the call.
    /// The run is then suspended and continues once the answer arrives, which
    /// is what durable execution is for.
    pub fn call(&mut self, request: &CapRequest) -> Result<CapOutcome, CapError> {
        // Cloned so the borrow of the manifest ends here: performing a call
        // must not be able to change what it was authorized against.
        let grant = authorize(&self.manifest, request)?.clone();
        match grant.name.as_str() {
            // The one capability that needs the broker rather than only the
            // grant, because what answers it is held here.
            "llm.invoke" => match self.driver.as_mut() {
                Some(driver) => llm_driven(&grant, request, driver.as_mut()),
                None => llm_invoke(&grant, request),
            },
            _ => perform_granted(&grant, request),
        }
    }

    /// What the agent's routed calls were, since the last time this was asked.
    ///
    /// They are drained rather than kept because the caller puts them in the
    /// journal, and an event recorded twice describes something that happened
    /// once.
    pub fn take_tool_uses(&mut self) -> Vec<AgentAction> {
        match self.driver.as_mut() {
            Some(driver) => driver.take_tool_uses(),
            None => Vec::new(),
        }
    }
}

/// Checks that the request names an entry that exists and agrees with it.
///
/// Both the index and the name are checked. A request whose index and name
/// disagree comes from a broken or hostile caller, and is not something to
/// reconcile.
///
/// A free function because the agent's own calls arrive somewhere else and are
/// authorized against the same manifest by the same code. A check that only
/// ran on one of the two paths would not be a check.
pub fn authorize<'a>(
    manifest: &'a [CapGrant],
    request: &CapRequest,
) -> Result<&'a CapGrant, CapError> {
    let grant = manifest
        .get(request.index as usize)
        .ok_or_else(|| CapError::new(format!("no capability {} in the manifest", request.index)))?;
    if grant.name != request.name {
        return Err(CapError::new(format!(
            "capability {} is `{}`, but the request says `{}`",
            request.index, grant.name, request.name
        )));
    }
    Ok(grant)
}

/// Performs a call the grant already allows.
///
/// `llm.invoke` is not here: what answers it is held by a `Broker`, and nothing
/// else is. That is also what stops an agent from summoning another one - the
/// path its calls arrive on cannot reach this.
pub fn perform_granted(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    match grant.name.as_str() {
        "fs.read" => fs_read(grant, request),
        "fs.write" => fs_write(grant, request),
        "process.exec" => process_exec(grant, request),
        "process.capture" => process_capture(grant, request),
        "process.run" => process_run(grant, request),
        "git.status" => git_status(grant, request),
        "git.rev_parse" => git_rev_parse(grant, request),
        "human.approve" => human_approve(grant, request),
        "human.choose" => human_choose(grant, request),
        other => Err(CapError::new(format!(
            "`{other}` is in the manifest but this broker cannot perform it"
        ))),
    }
}

/// Performs one call for the agent, against the program's manifest.
///
/// The same authorization and the same code as a call from the VM. That is the
/// whole argument for routing rather than translating: a routed effect is not
/// the same effect with a similar policy, it is the same call.
pub fn perform(manifest: &[CapGrant], request: &CapRequest) -> Result<CapOutcome, CapError> {
    let grant = authorize(manifest, request)?.clone();
    if grant.name == "llm.invoke" {
        return Err(CapError::new(
            "an agent may not summon another agent".to_string(),
        ));
    }
    perform_granted(&grant, request)
}

// ---- the capabilities ----

fn fs_read(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let path = allowed_path(grant, string_arg(request, 0, 1)?)?;

    let size = std::fs::metadata(&path)
        .map_err(|e| CapError::new(format!("cannot read `{}`: {e}", path.display())))?
        .len();
    if size > MAX_READ_BYTES {
        return Err(CapError::new(format!(
            "`{}` is {size} bytes, over the {MAX_READ_BYTES} byte limit",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CapError::new(format!("cannot read `{}`: {e}", path.display())))?;
    Ok(CapOutcome::Value(CapValue::Str(text)))
}

fn fs_write(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let path = allowed_path(grant, string_arg(request, 0, 2)?)?;
    let data = string_arg(request, 1, 2)?.to_string();
    std::fs::write(&path, data)
        .map_err(|e| CapError::new(format!("cannot write `{}`: {e}", path.display())))?;
    Ok(CapOutcome::Value(CapValue::Unit))
}

/// Runs a program and returns what it printed, when it succeeded.
///
/// A non-zero exit is a failure rather than a value: what a program printed on
/// its way to failing is not an answer, and the exit code is what
/// `process.exec` is for. `docs/design/output.md` says why the two are separate
/// capabilities.
fn process_capture(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    // Draining a pipe and honouring a deadline at once needs a reader thread,
    // and telling a program its call was bounded when it was not is worse than
    // refusing the deadline.
    reject_timeout(request)?;
    let (path, args) = exec_target(grant, request)?;

    let mut command = std::process::Command::new(&path);
    command.args(&args);
    as_the_grant_says(&mut command, grant);
    let out = command
        .output()
        .map_err(|e| CapError::new(format!("cannot run `{}`: {e}", path.display())))?;

    // A truncated answer that looks whole would parse, validate, and be wrong.
    if out.stdout.len() > MAX_OUTPUT {
        return Err(CapError::new(format!(
            "`{}` printed more than {MAX_OUTPUT} bytes",
            path.display()
        )));
    }
    match out.status.code() {
        Some(0) => {}
        Some(code) => {
            // stderr is not a value the program receives, but it is the only
            // useful part of a failure, so it travels in the error.
            let said = String::from_utf8_lossy(&out.stderr);
            let said = said.trim();
            let tail = if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            };
            return Err(CapError::new(format!(
                "`{}` exited {code}{tail}",
                path.display()
            )));
        }
        None => {
            return Err(CapError::new(format!(
                "`{}` was terminated by a signal",
                path.display()
            )));
        }
    }
    let text = String::from_utf8(out.stdout).map_err(|e| {
        CapError::new(format!(
            "`{}` printed something that is not UTF-8 (at byte {})",
            path.display(),
            e.utf8_error().valid_up_to()
        ))
    })?;
    Ok(CapOutcome::Value(CapValue::Str(text)))
}

/// The path and arguments a call may use, or the reason it may not.
///
/// Shared by `process.exec` and `process.capture`: they perform different
/// effects, and what a grant permits is the same question for both.
/// Runs a program and answers with both facts: the code it exited with, and
/// what it printed.
///
/// The difference from `process_capture` is one `match` it does not have. A
/// non-zero exit is an answer rather than an error, because the caller asked
/// for the code - which is what makes a linter, a diff, or a failing test
/// suite reachable at all. See `docs/design/output.md` §9.
///
/// Everything else is the same and deliberately so: the environment is
/// cleared, the output is capped before it is a value, and text that is not
/// UTF-8 is refused rather than replaced.
fn process_run(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    // Same reason as `process_capture`: draining a pipe and honouring a
    // deadline at once needs a reader thread.
    reject_timeout(request)?;
    let (path, args) = exec_target(grant, request)?;

    let mut command = std::process::Command::new(&path);
    command.args(&args);
    as_the_grant_says(&mut command, grant);
    let out = command
        .output()
        .map_err(|e| CapError::new(format!("cannot run `{}`: {e}", path.display())))?;

    if out.stdout.len() > MAX_OUTPUT {
        return Err(CapError::new(format!(
            "`{}` printed more than {MAX_OUTPUT} bytes",
            path.display()
        )));
    }
    // A signal is still not an exit code, and reporting one would say the
    // program finished when it did not.
    let Some(code) = out.status.code() else {
        return Err(CapError::new(format!(
            "`{}` was terminated by a signal",
            path.display()
        )));
    };
    let output = String::from_utf8(out.stdout).map_err(|e| {
        CapError::new(format!(
            "`{}` printed something that is not UTF-8 (at byte {})",
            path.display(),
            e.utf8_error().valid_up_to()
        ))
    })?;
    Ok(CapOutcome::Value(CapValue::Exit {
        code: code as i64,
        output,
    }))
}

/// Gives a command the environment and the directory the grant names.
///
/// The environment is cleared first and then filled from the grant, so a child
/// gets exactly what the manifest says and nothing that happened to be in the
/// shell that started `sic`. That was already true - the clearing is not new -
/// and what is new is that a program can say what it needs instead of doing
/// without.
///
/// A grant with no `in` leaves the directory alone, which means the child
/// inherits the one `sic` was started in. `sic plan` says which of the two it
/// is, because a reader who is not told assumes the grant is the whole of it.
fn as_the_grant_says(command: &mut std::process::Command, grant: &CapGrant) {
    command.env_clear();
    for (name, value) in &grant.env {
        command.env(name, value);
    }
    if !grant.dir.is_empty() {
        command.current_dir(&grant.dir);
    }
}

/// Checks a pinned binary against what is on disk.
///
/// A path says where to look, not what is there. When the grant pins the
/// contents they are checked on every call: a check that ran earlier tells you
/// what was true earlier.
fn check_pin(grant: &CapGrant, path: &std::path::Path) -> Result<(), CapError> {
    if grant.pin.is_empty() {
        return Ok(());
    }
    let found = hash_file(path)?;
    if found != grant.pin.to_ascii_lowercase() {
        return Err(CapError::new(format!(
            "`{}` is sha256:{found}, but the grant pins sha256:{}",
            path.display(),
            grant.pin
        )));
    }
    Ok(())
}

fn exec_target(grant: &CapGrant, request: &CapRequest) -> Result<(PathBuf, Vec<String>), CapError> {
    let args = exec_args(request)?;
    let path = allowed_path(grant, string_arg(request, 0, request.args.len())?)?;

    // An executable is never resolved through PATH: what runs is decided by the
    // grant, not by the environment the run happens to have.
    if !path.is_absolute() {
        return Err(CapError::new(format!(
            "`{}` is not an absolute path, and process.exec does not search PATH",
            path.display()
        )));
    }

    check_pin(grant, &path)?;

    // Two rules, and the difference between them is the point. A grant that
    // names a prefix allows anything starting with it. A grant that names
    // nothing allows nothing: an empty prefix is not "anything", it is a grant
    // written before arguments existed, still meaning what it meant then.
    let allowed = if grant.args.is_empty() {
        args.is_empty()
    } else {
        args.len() >= grant.args.len() && args[..grant.args.len()] == grant.args[..]
    };
    if !allowed {
        return Err(CapError::new(format!(
            "`{}` was called with {}, but the grant allows {}",
            path.display(),
            render_args(&args),
            if grant.args.is_empty() {
                "no arguments".to_string()
            } else {
                format!("only arguments starting {}", render_args(&grant.args))
            }
        )));
    }

    Ok((path, args))
}

fn process_exec(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    let (path, args) = exec_target(grant, request)?;

    let mut command = std::process::Command::new(&path);
    command.args(&args);
    as_the_grant_says(&mut command, grant);
    let status = if request.timeout_ms == 0 {
        command
            .status()
            .map_err(|e| CapError::new(format!("cannot run `{}`: {e}", path.display())))?
    } else {
        run_with_deadline(&mut command, &path, request.timeout_ms)?
    };

    // A process killed by a signal has no exit code, and reporting one would
    // say it finished when it did not.
    match status.code() {
        Some(code) => Ok(CapOutcome::Value(CapValue::I64(code as i64))),
        None => Err(CapError::new(format!(
            "`{}` was terminated by a signal",
            path.display()
        ))),
    }
}

/// Asking a person to approve something.
///
/// A person is not in this process, so this never answers within the call. The
/// grant's constraint says what the approval is about, and it travels with the
/// question so that whoever answers - and whoever audits it later - can see
/// which grant was exercised.
fn human_approve(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let question = string_arg(request, 0, 1)?;
    Ok(CapOutcome::Deferred {
        question: format!("[{}] {question}", grant.constraint),
    })
}

/// Asking a person which one.
///
/// Like `human.approve`, this never answers within the call. The alternatives
/// travel with the question, numbered, because whoever answers has to be able
/// to read them without the source in front of them - and the answer is which
/// one, so the number is the answer.
fn human_choose(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let question = string_arg(request, 0, 2)?;
    let options = request.args[1].as_list().ok_or_else(|| {
        CapError::new(format!(
            "argument 1 of `{}` must be a List<String>, got {}",
            request.name,
            request.args[1].type_name()
        ))
    })?;
    if options.is_empty() {
        return Err(CapError::new(
            "a decision with no alternatives is not one".to_string(),
        ));
    }
    // Numbered from zero, because the number a person reads is the number they
    // answer with, and the answer is an index. Counting from one would put an
    // off-by-one in a translation layer forever.
    let mut text = format!("[{}] {question}", grant.constraint);
    for (i, option) in options.iter().enumerate() {
        text.push_str(&format!("\n  {i}. {option}"));
    }
    Ok(CapOutcome::Deferred { question: text })
}

/// Runs a child and kills it if it outlives its deadline.
///
/// The broker is the only side with a clock, which is why the deadline is
/// enforced here rather than in the VM.
fn run_with_deadline(
    command: &mut std::process::Command,
    path: &Path,
    timeout_ms: u32,
) -> Result<std::process::ExitStatus, CapError> {
    let mut child = command
        .spawn()
        .map_err(|e| CapError::new(format!("cannot run `{}`: {e}", path.display())))?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(e) => {
                return Err(CapError::new(format!(
                    "cannot wait for `{}`: {e}",
                    path.display()
                )));
            }
        }
        if Instant::now() >= deadline {
            // A process that ignored its deadline is killed and reaped, so it
            // cannot outlive the run that asked for it.
            let _ = child.kill();
            let _ = child.wait();
            return Err(CapError::new(format!(
                "`{}` did not finish within {timeout_ms}ms",
                path.display()
            )));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The sha256 of a file, read in chunks so that a large executable is not read
/// into memory to check it.
fn hash_file(path: &Path) -> Result<String, CapError> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| CapError::new(format!("cannot read `{}`: {e}", path.display())))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| CapError::new(format!("cannot read `{}`: {e}", path.display())))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finish().hex())
}

/// Refuses a deadline this capability cannot honour.
///
/// Ignoring one would be worse than refusing it: the program would be told the
/// call was bounded when it was not.
fn reject_timeout(request: &CapRequest) -> Result<(), CapError> {
    if request.timeout_ms != 0 {
        return Err(CapError::new(format!(
            "`{}` cannot honour a timeout in v0.1",
            request.name
        )));
    }
    Ok(())
}

/// Asking a model.
///
/// This broker defers it, as it does an approval. Calling a model means HTTPS,
/// which means TLS, and writing one by hand is not the kind of
/// dependency-freedom this project is after. The deferred mechanism already
/// exists and is the right shape: the run suspends, something outside answers,
/// the run continues - and a checkpoint means the answer can arrive minutes
/// later or in another process.
fn llm_invoke(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let (asked, _) = asked_for(request)?;
    Ok(CapOutcome::Deferred {
        question: format!("[{}] {asked}", grant.constraint),
    })
}

/// The whole of what is being asked: the prompt, and the shape the answer has
/// to take when the caller said one.
///
/// Composed here rather than in the driver so that a person answering a
/// deferred call is told exactly what a model would have been told. They are
/// answering the same question.
fn asked_for(request: &CapRequest) -> Result<(String, bool), CapError> {
    let prompt = string_arg_of(request, 0)?;
    let shape = match request.args.len() {
        1 => "",
        2 => string_arg_of(request, 1)?,
        n => {
            return Err(CapError::new(format!(
                "`{}` takes 1 or 2 argument(s), got {n}",
                request.name
            )));
        }
    };
    if shape.is_empty() {
        return Ok((prompt.to_string(), false));
    }
    Ok((
        format!("{prompt}\n\nReply with JSON of this shape, and nothing else:\n{shape}"),
        true,
    ))
}

/// Asking a model that is running in a pane.
///
/// The grant has to name the agent that is going to answer. A program that asks
/// for one model and is answered by whichever one happened to be installed
/// would leave a manifest recording a claim that was not true, which is worse
/// than the call failing.
fn llm_driven(
    grant: &CapGrant,
    request: &CapRequest,
    driver: &mut dyn AgentDriver,
) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let (asked, json) = asked_for(request)?;
    if grant.constraint != driver.agent_name() {
        return Err(CapError::new(format!(
            "the grant asks `{}` to answer, but the driver runs `{}`",
            grant.constraint,
            driver.agent_name()
        )));
    }
    let answer = driver.ask(Ask {
        prompt: &asked,
        thread: Thread {
            task: request.task,
            conversation: request.conversation,
        },
        tools: request.tools_left,
        deadline_ms: request.answer_ms,
        json,
    })?;
    Ok(CapOutcome::Value(CapValue::Str(answer)))
}

/// Resolves the path an argument names, refusing anything the grant does not
/// cover.
fn allowed_path(grant: &CapGrant, requested: &str) -> Result<PathBuf, CapError> {
    // A `..` component is refused before anything else. Comparing paths that
    // can climb out of their prefix is where this kind of check usually fails,
    // and the grant is checked too in case the manifest itself is tainted.
    for (what, path) in [
        ("the requested path", requested),
        ("the grant", grant.constraint.as_str()),
    ] {
        if Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(CapError::new(format!(
                "{what} `{path}` contains `..`, which is not allowed"
            )));
        }
    }

    if requested != grant.constraint {
        return Err(CapError::new(format!(
            "`{}` may only be used with `{}`, not `{requested}`",
            grant.name, grant.constraint
        )));
    }
    // A grant names a path, and this is where that stops. What the path
    // resolves to is the machine's business: a symbolic link is followed, the
    // way the shell and every other program on that machine follow one.
    //
    // Refusing links was tried and is not available. `/bin` is a link to
    // `/usr/bin` on any system that merged them, so a rule refusing a link
    // anywhere along a path refuses `/bin/sh` - and a rule with an exception
    // for the links a distribution happens to ship is not a rule.
    //
    // What a plan therefore promises is "this program may open this path", not
    // "this path is not a link" and not "these bytes". The answer to the last
    // one is a pin, which `process.exec` has and `fs.read` does not - see
    // `docs/design/capabilities.md`.
    Ok(PathBuf::from(requested))
}

/// The argument vector a call passed, which may not be there at all.
///
/// Leaving it off means passing nothing, so that a program written before
/// arguments existed says exactly what it said then.
fn exec_args(request: &CapRequest) -> Result<Vec<String>, CapError> {
    match request.args.len() {
        1 => Ok(Vec::new()),
        2 => request.args[1]
            .as_list()
            .map(|a| a.to_vec())
            .ok_or_else(|| {
                CapError::new(format!(
                    "argument 1 of `{}` must be a List<String>, got {}",
                    request.name,
                    request.args[1].type_name()
                ))
            }),
        n => Err(CapError::new(format!(
            "`{}` takes 1 or 2 argument(s), got {n}",
            request.name
        ))),
    }
}

/// An argument vector as a person reads it in an error.
fn render_args(args: &[String]) -> String {
    if args.is_empty() {
        return "no arguments".to_string();
    }
    let quoted: Vec<String> = args.iter().map(|a| format!("{a:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

/// The argument at `index`, which must be a string.
///
/// The verifier already checked the types, but the broker re-checks them: it is
/// on the other side of a boundary from whoever produced that bytecode.
fn string_arg(request: &CapRequest, index: usize, expected: usize) -> Result<&str, CapError> {
    if request.args.len() != expected {
        return Err(CapError::new(format!(
            "`{}` takes {expected} argument(s), got {}",
            request.name,
            request.args.len()
        )));
    }
    string_arg_of(request, index)
}

/// The argument at `index`, which must be a string, without saying how many
/// there should have been.
fn string_arg_of(request: &CapRequest, index: usize) -> Result<&str, CapError> {
    request.args[index].as_str().ok_or_else(|| {
        CapError::new(format!(
            "argument {index} of `{}` must be a String, got {}",
            request.name,
            request.args[index].type_name()
        ))
    })
}

/// What git is told on every call, before anything the grant says.
///
/// This list is the reason `git` is a capability at all. A manifest can pin
/// the binary and clear the environment, and neither of those reaches
/// `.git/config`, `~/.gitconfig` or `/etc/gitconfig` - and any of the three
/// can name a program git will then run:
///
/// | what | reaches |
/// |------|---------|
/// | `core.pager`, `core.editor`, `diff.external` | a command line |
/// | `.git/hooks` | executables that arrived with the repository |
/// | `credential.helper` | a program, named in configuration |
/// | `protocol.ext` | a remote URL turned into a command |
///
/// A repository is data that came from somewhere. Its hooks are not.
///
/// `docs/design/git.md` §2.
const FIRST: &[&str] = &[
    // Nothing here writes, so a hook has nothing to run on. Set anyway: a
    // repository may name a `hooksPath` that a later read triggers, and a list
    // that is right for the reason rather than for today's calls is the one
    // that stays right.
    "-c",
    "core.hooksPath=/dev/null",
    // Each of these is a command line in a configuration file.
    "-c",
    "core.pager=cat",
    "-c",
    "core.editor=false",
    "-c",
    "diff.external=",
    // A URL that becomes a command, which is the one line in a config file
    // that reaches the network and a shell in the same step.
    "-c",
    "protocol.ext.allow=never",
    // A program named in configuration, asked for by anything that touches a
    // remote. Nothing here does; it costs one line to be sure.
    "-c",
    "credential.helper=",
    // And no pager process at all, whatever the config said.
    "--no-pager",
];

/// The environment git gets, which is nothing plus the two that say so.
///
/// `env_clear` alone is not enough: git reads `/etc/gitconfig` and
/// `$HOME/.gitconfig` by paths of its own, and with no `HOME` it falls back to
/// the passwd entry rather than to nothing. These say it in git's own words.
fn only_this_repository(command: &mut std::process::Command) {
    command.env_clear();
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    // Asked for by `credential.helper` and by anything that opens a terminal.
    // There is nobody to ask: this run's person is answering `sic`, not git.
    command.env("GIT_TERMINAL_PROMPT", "0");
}

/// Builds the call, with the grant's binary and directory and none of its
/// environment.
///
/// A `git` grant may say `in`, because which repository is exactly what a
/// reader needs to see. It may not say `env`, and E0336 refuses it at compile
/// time: a variable there would decide what git reads, which is the decision
/// this capability exists to take.
fn git_command(grant: &CapGrant, rest: &[&str]) -> Result<std::process::Command, CapError> {
    let path = PathBuf::from(&grant.constraint);
    if !path.is_absolute() {
        return Err(CapError::new(format!(
            "`{}` is not an absolute path, and `git` is not searched for on PATH",
            path.display()
        )));
    }
    check_pin(grant, &path)?;
    let mut command = std::process::Command::new(&path);
    only_this_repository(&mut command);
    if !grant.dir.is_empty() {
        command.current_dir(&grant.dir);
    }
    command.args(FIRST);
    command.args(rest);
    Ok(command)
}

/// What git said, or what it said when it failed.
fn git_output(mut command: std::process::Command) -> Result<String, CapError> {
    let out = command
        .output()
        .map_err(|e| CapError::new(format!("cannot run git: {e}")))?;
    if !out.status.success() {
        // git's own diagnosis, which is more use than the exit code: "not a
        // git repository" and "unknown revision" are different problems with
        // different fixes.
        let said = String::from_utf8_lossy(&out.stderr);
        let said = said.trim();
        return Err(CapError::new(match said.is_empty() {
            true => format!("git exited {}", out.status),
            false => format!("git: {said}"),
        }));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| CapError::new("git answered with something that is not text".to_string()))
}

/// What is modified, staged or untracked, one entry per path.
///
/// `--porcelain=v1` because it is the format git documents as stable for
/// scripts, which is what makes reading it a reasonable thing for a broker to
/// do and an unreasonable thing for a workflow to do - `sic`'s string handling
/// is thin on purpose and should stay that way rather than grow to meet `sed`.
fn git_status(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let text = git_output(git_command(grant, &["status", "--porcelain=v1"])?)?;
    let lines = text.lines().map(str::to_string).collect();
    Ok(CapOutcome::Value(CapValue::List(lines)))
}

/// What a revision resolves to.
fn git_rev_parse(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    reject_timeout(request)?;
    let rev = string_arg(request, 0, 1)?;
    // A revision is a name, and this is the only place a `git` call takes one
    // from the program. `--` and a leading `-` are how an argument becomes an
    // option, and an option is how a read becomes something else.
    if rev.starts_with('-') || rev.contains(char::is_whitespace) || rev.is_empty() {
        return Err(CapError::new(format!(
            "`{rev}` is not a revision: a revision is a name, and one that starts with `-` is \
             an option"
        )));
    }
    let text = git_output(git_command(grant, &["rev-parse", "--verify", rev])?)?;
    Ok(CapOutcome::Value(CapValue::Str(text.trim().to_string())))
}

#[cfg(test)]
mod tests;
