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

/// The largest file `fs.read` will return. A capability that can exhaust the
/// host's memory is not much of a boundary.
pub const MAX_READ_BYTES: u64 = 1 << 20;

/// How often a timed-out child process is checked. Small enough that a deadline
/// is roughly honoured, large enough that waiting is not a spin.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How much of a file is hashed at a time. An executable can be large, and
/// reading one into memory to check it would be its own problem.
const HASH_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Broker {
    manifest: Vec<CapGrant>,
}

impl Broker {
    pub fn new(manifest: Vec<CapGrant>) -> Self {
        Self { manifest }
    }

    pub fn manifest(&self) -> &[CapGrant] {
        &self.manifest
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
            "llm.invoke" => llm_invoke(&grant, request),
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

fn process_exec(grant: &CapGrant, request: &CapRequest) -> Result<CapOutcome, CapError> {
    let path = allowed_path(grant, string_arg(request, 0, 1)?)?;

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

    let mut command = std::process::Command::new(&path);
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
    let prompt = string_arg(request, 0, 1)?;
    Ok(CapOutcome::Deferred {
        question: format!("[{}] {prompt}", grant.constraint),
    })
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
