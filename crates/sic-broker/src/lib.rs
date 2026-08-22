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

use sic_core::{CapError, CapGrant, CapRequest, CapValue};

/// The largest file `fs.read` will return. A capability that can exhaust the
/// host's memory is not much of a boundary.
pub const MAX_READ_BYTES: u64 = 1 << 20;

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
    pub fn call(&mut self, request: &CapRequest) -> Result<CapValue, CapError> {
        // Cloned so the borrow of the manifest ends here: performing a call
        // must not be able to change what it was authorized against.
        let grant = self.authorize(request)?.clone();
        match grant.name.as_str() {
            "fs.read" => fs_read(&grant, request),
            "fs.write" => fs_write(&grant, request),
            "process.exec" => process_exec(&grant, request),
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

fn fs_read(grant: &CapGrant, request: &CapRequest) -> Result<CapValue, CapError> {
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
    Ok(CapValue::Str(text))
}

fn fs_write(grant: &CapGrant, request: &CapRequest) -> Result<CapValue, CapError> {
    let path = allowed_path(grant, string_arg(request, 0, 2)?)?;
    let data = string_arg(request, 1, 2)?.to_string();
    std::fs::write(&path, data)
        .map_err(|e| CapError::new(format!("cannot write `{}`: {e}", path.display())))?;
    Ok(CapValue::Unit)
}

fn process_exec(grant: &CapGrant, request: &CapRequest) -> Result<CapValue, CapError> {
    let path = allowed_path(grant, string_arg(request, 0, 1)?)?;

    // An executable is never resolved through PATH: what runs is decided by the
    // grant, not by the environment the run happens to have.
    if !path.is_absolute() {
        return Err(CapError::new(format!(
            "`{}` is not an absolute path, and process.exec does not search PATH",
            path.display()
        )));
    }

    let status = std::process::Command::new(&path)
        .env_clear()
        .status()
        .map_err(|e| CapError::new(format!("cannot run `{}`: {e}", path.display())))?;

    // A process killed by a signal has no exit code, and reporting one would
    // say it finished when it did not.
    match status.code() {
        Some(code) => Ok(CapValue::I64(code as i64)),
        None => Err(CapError::new(format!(
            "`{}` was terminated by a signal",
            path.display()
        ))),
    }
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
