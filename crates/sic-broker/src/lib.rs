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

use sic_core::{CapError, CapGrant, CapOutcome, CapRequest, CapValue, Sha256};

pub mod agent;
pub mod tmux;

pub use agent::{AgentDriver, Ask, DriverInfo, Thread};
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
        let grant = self.authorize(request)?.clone();
        match grant.name.as_str() {
            "fs.read" => fs_read(&grant, request),
            "fs.write" => fs_write(&grant, request),
            "process.exec" => process_exec(&grant, request),
            "human.approve" => human_approve(&grant, request),
            "process.capture" => process_capture(&grant, request),
            "human.choose" => human_choose(&grant, request),
            "llm.invoke" => match self.driver.as_mut() {
                Some(driver) => llm_driven(&grant, request, driver.as_mut()),
                None => llm_invoke(&grant, request),
            },
            other => Err(CapError::new(format!(
                "`{other}` is in the manifest but this broker cannot perform it"
            ))),
        }
    }

    /// Checks that the request names an entry that exists and agrees with it.
    ///
    /// Both the index and the name are checked. A request whose index and name
    /// disagree comes from a broken or hostile caller, and is not something to
    /// reconcile.
    fn authorize(&self, request: &CapRequest) -> Result<&CapGrant, CapError> {
        let grant = self.manifest.get(request.index as usize).ok_or_else(|| {
            CapError::new(format!("no capability {} in the manifest", request.index))
        })?;
        if grant.name != request.name {
            return Err(CapError::new(format!(
                "capability {} is `{}`, but the request says `{}`",
                request.index, grant.name, request.name
            )));
        }
        Ok(grant)
    }
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

    let out = std::process::Command::new(&path)
        .args(&args)
        .env_clear()
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

    // A path says where to look, not what is there. When the grant pins the
    // contents, they are checked on every call: a check that ran earlier tells
    // you what was true earlier.
    if !grant.pin.is_empty() {
        let found = hash_file(&path)?;
        if found != grant.pin.to_ascii_lowercase() {
            return Err(CapError::new(format!(
                "`{}` is sha256:{found}, but the grant pins sha256:{}",
                path.display(),
                grant.pin
            )));
        }
    }

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
    command.env_clear();
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

#[cfg(test)]
mod tests;
