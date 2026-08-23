//! `sic update`: verify and swap.
//!
//! Fetching a release is somebody else's job - `curl`, a browser, a package
//! manager. This takes a file that is already on disk, checks it against a
//! digest the user brought with them, and puts it where the running binary is.
//! `docs/design/update.md` says why it stops there.

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sic_core::Sha256;

use super::{EXIT_FAILURE, EXIT_USAGE};

/// How much of a file is hashed at a time. A binary is large enough that
/// reading it whole to check it would be a waste of memory for no gain.
const HASH_CHUNK: usize = 64 * 1024;

pub struct UpdateOptions<'a> {
    /// The candidate binary. Without one, this only reports what is installed.
    pub to: Option<&'a str>,
    /// The digest the candidate has to have. Required whenever `to` is given.
    pub sha256: Option<&'a str>,
    /// Do everything except the final rename.
    pub check: bool,
}

pub fn run(opts: UpdateOptions) -> ExitCode {
    match update(opts) {
        Ok(code) => code,
        Err(Failure { message, code }) => {
            eprintln!("error: {message}");
            ExitCode::from(code)
        }
    }
}

struct Failure {
    message: String,
    code: u8,
}

fn wrong(message: impl Into<String>) -> Failure {
    Failure {
        message: message.into(),
        code: EXIT_USAGE,
    }
}

fn failed(message: impl Into<String>) -> Failure {
    Failure {
        message: message.into(),
        code: EXIT_FAILURE,
    }
}

fn update(opts: UpdateOptions) -> Result<ExitCode, Failure> {
    // Which file is being replaced is decided before anything else, because
    // every later refusal is about that file.
    let installed = current_exe()?;
    let installed_digest = hash_file(&installed)?;
    report("installed", crate::VERSION, &installed_digest, &installed);

    let Some(candidate) = opts.to else {
        if !opts.check {
            return Err(wrong(
                "`update` needs `--to <FILE> --sha256 <HEX>`, or `--check` on its own \
                 to see what is installed",
            ));
        }
        if let Some(owner) = manager_of(&installed) {
            println!("{owner} installed this, so an update goes through it");
        }
        return Ok(ExitCode::SUCCESS);
    };

    // A digest is not a precaution that can be skipped: without one there is
    // nothing to verify against, and this becomes a way to replace a binary
    // rather than a way to update one.
    let Some(expected) = opts.sha256 else {
        return Err(wrong(
            "`--to` needs `--sha256 <HEX>`: what gets installed is decided by \
             what the file is, not by where it came from",
        ));
    };
    let expected = parse_digest(expected)?;

    if let Some(owner) = manager_of(&installed) {
        return Err(failed(format!(
            "`{}` was installed by {owner}; update it the way it was installed, \
             because replacing the file leaves {owner} describing one that is no longer there",
            display(&installed)
        )));
    }

    let candidate = Path::new(candidate);
    let found = hash_file(candidate)?;
    if found != expected {
        return Err(failed(format!(
            "`{}` is sha256:{found}, but --sha256 says sha256:{expected}",
            display(candidate)
        )));
    }
    if found == installed_digest {
        // Byte-identical to what is running, so its version is known without
        // asking it.
        report("candidate", crate::VERSION, &found, candidate);
        println!("already installed, so there is nothing to do");
        return Ok(ExitCode::SUCCESS);
    }

    // From here the bytes are the verified ones. They are staged next to the
    // destination, so what identifies itself below is the same file the rename
    // then puts in place, and so the rename stays within one filesystem.
    let staged = stage(candidate, &installed)?;
    let version = match identify(&staged.path) {
        Ok(version) => version,
        Err(e) => {
            staged.discard();
            return Err(e);
        }
    };
    report("candidate", &version, &found, candidate);

    if opts.check {
        staged.discard();
        println!("would replace {}", display(&installed));
        return Ok(ExitCode::SUCCESS);
    }

    swap(staged, &installed)?;
    println!(
        "replaced {}  {} -> {version}",
        display(&installed),
        crate::VERSION
    );
    Ok(ExitCode::SUCCESS)
}

fn report(what: &str, version: &str, digest: &str, path: &Path) {
    println!(
        "  {what}  {version}  sha256:{}  {}",
        short(digest),
        display(path)
    );
}

/// The first eight characters, which is what a person compares by eye. The
/// whole digest is printed only when two of them disagree.
fn short(digest: &str) -> &str {
    &digest[..8]
}

fn current_exe() -> Result<PathBuf, Failure> {
    let path = std::env::current_exe()
        .map_err(|e| failed(format!("cannot tell where this binary is: {e}")))?;
    // Resolving symlinks matters: replacing the link rather than its target
    // would update something nobody runs.
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

fn parse_digest(given: &str) -> Result<String, Failure> {
    let hex = given.strip_prefix("sha256:").unwrap_or(given);
    let hex = hex.to_ascii_lowercase();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(wrong(format!(
            "`{given}` is not a sha256: 64 hex characters, optionally written `sha256:...`"
        )));
    }
    Ok(hex)
}

fn hash_file(path: &Path) -> Result<String, Failure> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| wrong(format!("cannot read `{}`: {e}", display(path))))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| wrong(format!("cannot read `{}`: {e}", display(path))))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finish().hex())
}

/// A file written next to the destination, removed if it is never installed.
struct Staged {
    path: PathBuf,
}

impl Staged {
    fn discard(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn stage(candidate: &Path, installed: &Path) -> Result<Staged, Failure> {
    let dir = installed.parent().unwrap_or(Path::new("."));
    // Windows decides what may be executed partly by extension, and the staged
    // file is run before it is installed.
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let path = dir.join(format!(".sic-update-{}{suffix}", std::process::id()));

    let bytes = std::fs::read(candidate)
        .map_err(|e| wrong(format!("cannot read `{}`: {e}", display(candidate))))?;
    let staged = Staged { path };
    if let Err(e) = write_all(&staged.path, &bytes) {
        staged.discard();
        return Err(failed(format!(
            "cannot write to `{}`: {e}; sic does not ask for privileges it was not given",
            display(dir)
        )));
    }
    Ok(staged)
}

fn write_all(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    // Executable, and only writable by its owner. A staged binary that is
    // group-writable would be a way in during the window before the rename.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    file.sync_all()
}

/// Asks the candidate what it is, by running it.
///
/// This happens only after the digest matched, so what runs is the file the
/// user vouched for.
fn identify(staged: &Path) -> Result<String, Failure> {
    let out = Command::new(staged)
        .arg("version")
        .env_clear()
        .output()
        .map_err(|e| failed(format!("cannot run the candidate: {e}")))?;
    let said = String::from_utf8_lossy(&out.stdout);
    let said = said.trim();
    let version = said
        .strip_prefix("sic ")
        .filter(|v| !v.is_empty() && !v.contains(char::is_whitespace))
        .ok_or_else(|| {
            failed(format!(
                "the candidate does not identify itself as sic: `sic version` said `{said}`"
            ))
        })?;
    Ok(version.to_string())
}

/// Puts the staged file where the running binary is.
///
/// On Unix a running executable cannot be written to, but its directory entry
/// can be replaced, so the rename is both allowed and atomic.
#[cfg(unix)]
fn swap(staged: Staged, installed: &Path) -> Result<(), Failure> {
    std::fs::rename(&staged.path, installed).map_err(|e| {
        let path = staged.path.clone();
        staged.discard();
        failed(format!(
            "cannot replace `{}` with `{}`: {e}",
            display(installed),
            display(&path)
        ))
    })
}

/// Windows will not replace or delete a running image, but it will rename one,
/// so the old binary is moved aside first. Deleting it afterwards fails while
/// this process is still running from it, and saying so is better than leaving
/// a file nobody expected.
#[cfg(windows)]
fn swap(staged: Staged, installed: &Path) -> Result<(), Failure> {
    let path = staged.path.clone();
    let aside = installed.with_extension(format!("old-{}", std::process::id()));
    if let Err(e) = std::fs::rename(installed, &aside) {
        staged.discard();
        return Err(failed(format!(
            "cannot move `{}` aside: {e}",
            display(installed)
        )));
    }
    if let Err(e) = std::fs::rename(&path, installed) {
        // Put back what was there, so a failed update leaves a working binary.
        let _ = std::fs::rename(&aside, installed);
        staged.discard();
        return Err(failed(format!(
            "cannot install `{}` as `{}`: {e}",
            display(&path),
            display(installed)
        )));
    }
    if std::fs::remove_file(&aside).is_err() {
        println!(
            "the old binary is still running, so it is at {} until it is not",
            display(&aside)
        );
    }
    Ok(())
}

/// Who else already manages this file, if anyone.
///
/// Replacing a file a package manager installed leaves its record describing
/// one that is no longer there, and its next upgrade would undo the update
/// without saying so.
fn manager_of(path: &Path) -> Option<&'static str> {
    let path = display(path);
    let owned: &[(&str, &str)] = &[
        ("/nix/store/", "Nix"),
        ("/opt/homebrew/", "Homebrew"),
        ("/home/linuxbrew/", "Homebrew"),
        ("/snap/", "snap"),
        ("/var/lib/flatpak/", "Flatpak"),
        ("/usr/bin/", "the system package manager"),
        ("/bin/", "the system package manager"),
    ];
    for (prefix, owner) in owned {
        if path.starts_with(prefix) {
            return Some(owner);
        }
    }
    // Not anchored: a cargo or Homebrew root can be anywhere.
    if path.contains("/.cargo/bin/") {
        return Some("cargo");
    }
    if path.contains("/Cellar/") {
        return Some("Homebrew");
    }
    None
}

fn display(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}
