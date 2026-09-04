//! `sic upgrade`: fetch, verify, swap.
//!
//! sic does not speak HTTP. It runs `curl`, at an absolute path, only when this
//! command is the one that was typed - the same arrangement `process.exec`
//! makes for a program: the effect is performed by something outside, and only
//! because somebody asked for it. Nothing here runs on a timer, and a sic
//! program still has no way to reach a network.
//!
//! What it then does is check: the archive against the digest the release
//! publishes, the binary inside it against the same list, and the file it is
//! about to install against what that file says it is.
//! `docs/design/upgrade.md` says what those checks do and do not prove.

use crate::out::sayln;

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sic_core::{Digest, Sha256};

use super::{EXIT_FAILURE, EXIT_USAGE};
use crate::path::display;

/// The flags every fetch here makes, so that no call site can be missing one.
///
/// `--proto =https` binds the URL on the command line. It does **not** bind a
/// redirect - curl's own manual says "by default curl only allows HTTP, HTTPS,
/// FTP and FTPS on redirect" - so without `--proto-redir` a response that sends
/// this to `http://` is followed, and a download this is about to check a digest
/// against arrives in clear. The digest check still holds; what is lost is that
/// nobody on the path read or rewrote the bytes, which is a claim the `https` in
/// the URL looks like it is already making.
///
/// One constant rather than three copies of two flags, for the reason
/// `recorded_message` is one function: two of them would agree until one was
/// edited.
const HTTPS: &[&str] = &["-fsSL", "--proto", "=https", "--proto-redir", "=https"];

/// How much of a file is hashed at a time. A binary is large enough that
/// reading it whole to check it would be a waste of memory for no gain.
const HASH_CHUNK: usize = 64 * 1024;

/// Where a release comes from. Compiled in rather than configurable: an updater
/// that can be pointed somewhere else is a way to install something else.
const REPO: &str = "tak-kam/sic";

/// Absolute paths only. Resolving a downloader through PATH would let the
/// environment decide what performs the fetch.
#[cfg(windows)]
const CURL: &[&str] = &["C:/Windows/System32/curl.exe"];
#[cfg(not(windows))]
const CURL: &[&str] = &["/usr/bin/curl", "/bin/curl", "/opt/homebrew/bin/curl"];
#[cfg(windows)]
const TAR: &[&str] = &["C:/Windows/System32/tar.exe"];
#[cfg(not(windows))]
const TAR: &[&str] = &["/usr/bin/tar", "/bin/tar", "/opt/homebrew/bin/tar"];

/// Variables a downloader needs to work in the environment it was run in. The
/// rest is cleared: a fetch should not inherit whatever else is set.
const KEEP_ENV: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "HOME",
    "SystemRoot",
    "USERPROFILE",
];

pub struct UpgradeOptions<'a> {
    /// A candidate already on disk. Without one, the latest release is fetched.
    pub to: Option<&'a str>,
    /// The digest the candidate has to have. Required with `to`, and refused
    /// without it: a fetched release brings its own.
    pub sha256: Option<&'a str>,
    /// Do everything except the final rename.
    pub check: bool,
}

pub fn run(opts: UpgradeOptions) -> ExitCode {
    match upgrade(opts) {
        Ok(code) => code,
        Err(Failure { message, code }) => {
            eprintln!("error: {message}");
            ExitCode::from(code)
        }
    }
}

#[derive(Debug)]
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

fn upgrade(opts: UpgradeOptions) -> Result<ExitCode, Failure> {
    // Which file is being replaced is decided before anything else, because
    // every later refusal is about that file.
    let installed = current_exe()?;
    let installed_digest = hash_file(&installed)?;
    report(
        "installed",
        crate::VERSION,
        &installed_digest,
        &display(&installed),
    );

    if let Some(owner) = manager_of(&installed) {
        return Err(failed(format!(
            "`{}` was installed by {owner}; upgrade it the way it was installed, \
             because replacing the file leaves {owner} describing one that is no longer there",
            display(&installed)
        )));
    }

    // The download has to outlive the borrow of what is in it: dropping it
    // removes the directory it was unpacked into.
    let download;
    let (candidate, expected, label) = match opts.to {
        Some(to) => {
            // A digest is not a precaution that can be skipped: without one
            // there is nothing to verify against, and this becomes a way to
            // replace a binary rather than a way to upgrade one.
            let Some(expected) = opts.sha256 else {
                return Err(wrong(
                    "`--to` needs `--sha256 <HEX>`: what gets installed is decided by \
                     what the file is, not by where it came from",
                ));
            };
            let path = PathBuf::from(to);
            let label = display(&path);
            (path, parse_digest(expected)?, label)
        }
        None => {
            if opts.sha256.is_some() {
                return Err(wrong(
                    "`--sha256` belongs with `--to`; a fetched release brings the \
                     digests it published",
                ));
            }
            match fetch_latest()? {
                Fetched::AlreadyLatest(tag) => {
                    // The version matches; the bytes may not, for a binary
                    // built locally. Saying "nothing newer" claims only what
                    // was actually compared.
                    sayln!("{tag} is the latest release, so there is nothing newer to install");
                    return Ok(ExitCode::SUCCESS);
                }
                Fetched::New(new) => {
                    download = new;
                    (
                        download.binary.clone(),
                        download.digest.clone(),
                        download.name.clone(),
                    )
                }
            }
        }
    };

    let found = hash_file(&candidate)?;
    if found != expected {
        return Err(failed(format!(
            "`{label}` is sha256:{found}, but the digest it should have is sha256:{expected}"
        )));
    }
    if found == installed_digest {
        // Byte-identical to what is running, so its version is known without
        // asking it.
        report("candidate", crate::VERSION, &found, &label);
        sayln!("already installed, so there is nothing to do");
        return Ok(ExitCode::SUCCESS);
    }

    // From here the bytes are the verified ones. They are staged next to the
    // destination, so what identifies itself below is the same file the rename
    // then puts in place, and so the rename stays within one filesystem.
    let staged = stage(&candidate, &installed)?;
    let version = match identify(&staged.path) {
        Ok(version) => version,
        Err(e) => {
            staged.discard();
            return Err(e);
        }
    };
    report("candidate", &version, &found, &label);

    if opts.check {
        staged.discard();
        sayln!("would replace {}", display(&installed));
        return Ok(ExitCode::SUCCESS);
    }

    swap(staged, &installed)?;
    sayln!(
        "replaced {}  {} -> {version}",
        display(&installed),
        crate::VERSION
    );
    Ok(ExitCode::SUCCESS)
}

/// What asking for the latest release found.
enum Fetched {
    /// The release that is published is the one that is running.
    AlreadyLatest(String),
    New(Download),
}

/// A release, unpacked into a directory that goes away when this does.
struct Download {
    binary: PathBuf,
    /// The digest SHA256SUMS gives for that binary.
    digest: String,
    /// What to call it in a line a person reads: the name SHA256SUMS uses.
    name: String,
    _dir: TempDir,
}

/// Asks GitHub what the latest release is, and unpacks it.
///
/// Every download is checked against SHA256SUMS from the same release, which
/// catches a truncated or corrupted transfer. It is not a signature, and
/// `docs/design/upgrade.md` §4 says plainly what that does and does not mean.
fn fetch_latest() -> Result<Fetched, Failure> {
    // Both tools are found before anything is downloaded: failing afterwards
    // would leave a fetch that had no way to finish.
    let curl = tool("curl", CURL)?;
    let tar = tool("tar", TAR)?;
    let target = target_triple()?;

    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = capture(&curl, &[HTTPS, &[api.as_str()]].concat())?;
    let json = sic_json::parse(&body)
        .map_err(|e| failed(format!("the release list is not JSON this can read: {e}")))?;
    let Some(sic_json::Json::Str(tag)) = json.member("tag_name") else {
        return Err(failed("the release list has no `tag_name`"));
    };
    let tag = tag.clone();
    // A release tag is `v` and a version, and this one is about to become a
    // URL and a path under the temporary directory. Nothing here runs a shell,
    // so there is no injection to have - what there is, is a string that
    // arrived over a network deciding where a file is written. `sic-plan`'s
    // `options_at` has the instinct: read what is there, and refuse rather than
    // guess when it is not what was expected.
    if !is_a_release_tag(&tag) {
        return Err(failed(format!(
            "the latest release is tagged `{tag}`, which is not `v` and a version"
        )));
    }
    if tag == format!("v{}", crate::VERSION) {
        return Ok(Fetched::AlreadyLatest(tag));
    }
    // "The latest release" and "newer than this" are not the same claim. A
    // binary built from a branch is ahead of every release, and quietly moving
    // it backwards under the word "upgrade" would be the surprising kind of
    // success. Going back is still possible, by naming the file.
    let backwards = match (
        version_of(tag.trim_start_matches('v')),
        version_of(crate::VERSION),
    ) {
        (Some(there), Some(here)) => there < here,
        _ => false,
    };
    if backwards {
        return Err(failed(format!(
            "{tag} is the latest release, which is older than the {} that is running; \
             use `--to` with `--sha256` to install it anyway",
            crate::VERSION
        )));
    }

    sayln!("fetching {tag} for {target}");
    let dir = TempDir::new()?;
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let stem = format!("sic-{tag}-{target}");
    let archive = if target.contains("windows") {
        format!("{stem}.zip")
    } else {
        format!("{stem}.tar.gz")
    };
    let inner = if target.contains("windows") {
        format!("{stem}/sic.exe")
    } else {
        format!("{stem}/sic")
    };

    let sums = capture(
        &curl,
        &[HTTPS, &[format!("{base}/SHA256SUMS").as_str()]].concat(),
    )?;
    let archive_path = dir.path.join(&archive);
    let url = format!("{base}/{archive}");
    let out = display(&archive_path);
    capture(
        &curl,
        &[HTTPS, &["-o", out.as_str(), url.as_str()]].concat(),
    )?;

    // The archive is checked before it is unpacked: handing an unverified one
    // to `tar` would be trusting it with more than a digest comparison needs.
    checked(&archive, &sums, &archive_path)?;

    let into = display(&dir.path);
    let status = Command::new(&tar)
        .args(["xf", &out, "-C", &into])
        .status()
        .map_err(|e| failed(format!("cannot run `{}`: {e}", display(&tar))))?;
    if !status.success() {
        return Err(failed(format!("`tar` could not unpack `{archive}`")));
    }

    Ok(Fetched::New(Download {
        binary: dir.path.join(&inner),
        digest: digest_line(&sums, &inner)?,
        name: inner,
        _dir: dir,
    }))
}

/// Whether a tag from the release list is one this may build a URL and a path
/// out of.
///
/// `v` and a version, and nothing else. Nothing here runs a shell, so there is
/// no injection to have; what there is, is a string that arrived over a network
/// deciding where a file is written. `sic-plan`'s `options_at` has the instinct:
/// read what is there, and refuse rather than guess when it is not what was
/// expected.
fn is_a_release_tag(tag: &str) -> bool {
    match tag.strip_prefix('v') {
        Some(rest) => version_of(rest).is_some(),
        None => false,
    }
}

/// `0.1.2` as something that can be compared.
///
/// Anything that is not three numbers returns `None`, and the caller treats
/// that as "no opinion" rather than as an error: refusing to upgrade because a
/// version string was unfamiliar would be worse than not checking.
fn version_of(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// The release built for the machine this is running on.
fn target_triple() -> Result<&'static str, Failure> {
    // Linux gets the musl build whatever its libc is: it is static, so it runs
    // where a glibc build might not.
    Ok(match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-musl",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        (arch, os) => {
            return Err(failed(format!(
                "no release is built for {arch} {os}; build from source, or use \
                     `--to` with a binary you have"
            )));
        }
    })
}

/// The first of these paths that exists.
fn tool(name: &str, candidates: &[&str]) -> Result<PathBuf, Failure> {
    for path in candidates {
        let path = Path::new(path);
        if path.exists() {
            return Ok(path.to_path_buf());
        }
    }
    Err(failed(format!(
        "`{name}` is not at any of {}; install it, or use `--to` with a binary \
         you downloaded yourself",
        candidates.join(", ")
    )))
}

/// Runs a tool and returns what it printed, with the environment cleared down
/// to what a fetch needs.
fn capture(tool: &Path, args: &[&str]) -> Result<String, Failure> {
    let mut command = Command::new(tool);
    command.args(args).env_clear();
    for name in KEEP_ENV {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    let out = command
        .output()
        .map_err(|e| failed(format!("cannot run `{}`: {e}", display(tool))))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(failed(format!(
            "`{}` failed: {}",
            display(tool),
            said.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Refuses a file that is not the one `SHA256SUMS` names.
///
/// This is what the whole of the fetch path rests on -
/// `docs/design/upgrade.md` §4 says the binaries a release publishes are
/// pinned by digest rather than by the signature on the tag, and this is that
/// pin being applied.
///
/// It is a function of its arguments rather than a step inside `fetch_latest`
/// so that a test can reach it. Welded in after two downloads, the one
/// comparison the feature depends on was the one thing no test could run.
fn checked(name: &str, sums: &str, path: &Path) -> Result<(), Failure> {
    let want = digest_line(sums, name)?;
    let found = hash_file(path)?;
    if found != want {
        return Err(failed(format!(
            "`{name}` is sha256:{found}, but SHA256SUMS says sha256:{want}"
        )));
    }
    Ok(())
}

/// The digest SHA256SUMS gives for one name.
///
/// A missing line is a failure rather than a reason to install unchecked, and
/// so are two lines for one name: a document that gives a file two digests
/// cannot decide anything, and picking one of them would be this program
/// deciding instead.
fn digest_line(sums: &str, name: &str) -> Result<String, Failure> {
    let mut found_hex: Option<&str> = None;
    for line in sums.lines() {
        let mut fields = line.split_whitespace();
        let (Some(hex), Some(named)) = (fields.next(), fields.next()) else {
            continue;
        };
        if named.trim_start_matches('*') != name {
            continue;
        }
        if found_hex.is_some_and(|first| first != hex) {
            return Err(failed(format!(
                "SHA256SUMS gives `{name}` two different digests"
            )));
        }
        found_hex = Some(hex);
    }
    match found_hex {
        Some(hex) => parse_digest(hex),
        None => Err(failed(format!("SHA256SUMS has no line for `{name}`"))),
    }
}

/// A directory that is removed when it goes out of scope.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<TempDir, Failure> {
        let path = std::env::temp_dir().join(format!("sic-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .map_err(|e| failed(format!("cannot make `{}`: {e}", display(&path))))?;
        Ok(TempDir { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn report(what: &str, version: &str, digest: &str, where_: &str) {
    sayln!("  {what}  {version}  sha256:{}  {where_}", short(digest));
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

/// A digest a person typed, or one read out of a `SHA256SUMS` line.
///
/// The `sha256:` prefix is optional here and required nowhere else. This is the
/// one place a digest is typed by hand rather than read back from something sic
/// wrote, and both spellings appear in the wild - the tag page prints one, the
/// journal prints the other. Accepting either costs nothing a command line can
/// misread.
///
/// What comes back is the lowercase hex, because that is what the rest of this
/// file compares against `hash_file`.
fn parse_digest(given: &str) -> Result<String, Failure> {
    let hex = given.strip_prefix("sha256:").unwrap_or(given);
    match Digest::from_hex(hex) {
        Some(digest) => Ok(digest.hex()),
        None => Err(wrong(format!(
            "`{given}` is not a sha256: 64 hex characters, optionally written `sha256:...`"
        ))),
    }
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
    let path = dir.join(format!(".sic-upgrade-{}{suffix}", std::process::id()));

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
        sayln!(
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

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &str = "\
9e37867d347e2fb7d8b5b833b0086cfaa9bbe14c6eecd5a4a5ef69f7cf9e318b  sic-v0.1.1-x86_64-unknown-linux-musl.tar.gz
975d6bcb612ea728a43c8fe1ca44dc09213e5c1892f0a399fabe2ff8bf27f0c6  sic-v0.1.1-x86_64-unknown-linux-musl/sic
";

    /// A file in a directory of this test's own, and the `SHA256SUMS` line
    /// that describes it.
    fn archive(name: &str, contents: &[u8]) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("sic-checked-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a writable temporary directory");
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("writable");
        let sums = format!("{}  {name}\n", Digest::of(contents).hex());
        (path, sums)
    }

    #[test]
    fn an_archive_that_matches_its_line_is_accepted() {
        let (path, sums) = archive("match.tar.gz", b"the release");
        assert!(checked("match.tar.gz", &sums, &path).is_ok());
    }

    /// The one comparison the fetch path rests on, failing. Both digests are in
    /// the message, because "it did not match" without them says nothing about
    /// which end is wrong.
    #[test]
    fn an_archive_that_does_not_match_is_refused_and_the_message_names_both() {
        let (path, sums) = archive("swapped.tar.gz", b"the release");
        std::fs::write(&path, b"something else").expect("writable");
        let err = checked("swapped.tar.gz", &sums, &path).expect_err("the digests differ");
        assert!(
            err.message.contains(&Digest::of(b"the release").hex()),
            "{}",
            err.message
        );
        assert!(
            err.message.contains(&Digest::of(b"something else").hex()),
            "{}",
            err.message
        );
    }

    /// A file `SHA256SUMS` says nothing about is not installed unchecked.
    #[test]
    fn an_archive_that_is_not_listed_is_refused() {
        let (path, _) = archive("unlisted.tar.gz", b"whatever");
        let err = checked("unlisted.tar.gz", SUMS, &path).expect_err("it is not in SUMS");
        assert!(err.message.contains("no line for"), "{}", err.message);
    }

    /// Reaching the comparison at all requires reading the file, and a file
    /// that cannot be read is not a file that matched.
    #[test]
    fn an_archive_that_cannot_be_read_is_refused() {
        let (path, sums) = archive("gone.tar.gz", b"the release");
        std::fs::remove_file(&path).expect("it was just written");
        let err = checked("gone.tar.gz", &sums, &path).expect_err("the file is gone");
        assert!(err.message.contains("cannot read"), "{}", err.message);
    }

    /// A document that gives one name two digests has not said which one, and
    /// choosing would be this program deciding on its behalf.
    #[test]
    fn two_lines_for_one_name_decide_nothing() {
        let (path, sums) = archive("twice.tar.gz", b"the release");
        let doubled = format!("{sums}{}  twice.tar.gz\n", Digest::of(b"another").hex());
        let err = checked("twice.tar.gz", &doubled, &path).expect_err("SUMS disagrees with itself");
        assert!(
            err.message.contains("two different digests"),
            "{}",
            err.message
        );

        // The same line twice is not a disagreement.
        let repeated = format!("{sums}{sums}");
        assert!(checked("twice.tar.gz", &repeated, &path).is_ok());
    }

    #[test]
    fn a_digest_is_read_by_the_name_it_belongs_to() {
        assert_eq!(
            digest_line(SUMS, "sic-v0.1.1-x86_64-unknown-linux-musl/sic").unwrap(),
            "975d6bcb612ea728a43c8fe1ca44dc09213e5c1892f0a399fabe2ff8bf27f0c6"
        );
    }

    /// Installing something SHA256SUMS says nothing about is the case this
    /// whole command exists to refuse.
    #[test]
    fn a_name_that_is_not_listed_is_a_failure() {
        assert!(digest_line(SUMS, "sic-v9.9.9-x86_64-unknown-linux-musl/sic").is_err());
    }

    /// `sha256sum -b` writes `*name`, and the star is not part of the name.
    #[test]
    fn a_binary_mode_line_is_read_the_same_way() {
        let sums = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff *sic\n";
        assert!(digest_line(sums, "sic").is_ok());
    }

    #[test]
    fn a_line_that_is_not_a_digest_is_refused() {
        let sums = "not-a-digest  sic\n";
        assert!(digest_line(sums, "sic").is_err());
    }

    #[test]
    fn this_machine_has_a_release_or_is_told_so() {
        match target_triple() {
            Ok(target) => assert!(target.contains('-'), "{target}"),
            Err(e) => assert!(e.message.contains("no release is built"), "{}", e.message),
        }
    }

    #[test]
    fn a_version_is_compared_as_numbers() {
        assert!(version_of("0.1.10") > version_of("0.1.9"));
        assert_eq!(version_of("0.1.2"), Some((0, 1, 2)));
        assert_eq!(version_of("0.1"), None);
        assert_eq!(version_of("0.1.2-dev"), None);
    }

    /// What the release list says is a tag becomes a URL and a path under the
    /// temporary directory, so it is checked before it is either.
    #[test]
    fn only_a_version_tag_is_followed() {
        assert!(is_a_release_tag("v0.8.0"));
        assert!(is_a_release_tag("v10.0.123"));

        assert!(!is_a_release_tag("0.8.0"), "a version is not a tag");
        assert!(!is_a_release_tag("v0.8"), "three numbers or none");
        assert!(!is_a_release_tag("v0.8.0-rc1"));
        assert!(!is_a_release_tag(""));
        // The shapes that would decide where a file is written. None of them
        // reaches a shell, and none of them should reach `Path::join` either.
        assert!(!is_a_release_tag("v../../../../etc/x"));
        assert!(!is_a_release_tag("v0.8.0/../.."));
        assert!(!is_a_release_tag("v0.8.0$(id)"));
    }

    /// Every fetch uses one set of flags, and `--proto-redir` is the half that
    /// is easy to leave out: `--proto` binds the URL on the command line and
    /// not the redirect that follows it.
    #[test]
    fn a_redirect_may_not_leave_https() {
        assert!(HTTPS.contains(&"--proto"));
        assert!(HTTPS.contains(&"--proto-redir"));
        assert_eq!(HTTPS.iter().filter(|f| **f == "=https").count(), 2);
    }

    #[test]
    fn a_digest_may_be_written_either_way() {
        let hex = "9e37867d347e2fb7d8b5b833b0086cfaa9bbe14c6eecd5a4a5ef69f7cf9e318b";
        assert_eq!(parse_digest(hex).unwrap(), hex);
        assert_eq!(parse_digest(&format!("sha256:{hex}")).unwrap(), hex);
        assert_eq!(parse_digest(&hex.to_ascii_uppercase()).unwrap(), hex);
        assert!(parse_digest("beef").is_err());
    }
}
