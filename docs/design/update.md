# `sic update`

Updating a tool is ordinary. Doing it the ordinary way is not available here.

Fetching a release means HTTPS, which means TLS, which is not something to write
by hand for this - the same wall that made `llm.invoke` defer rather than call
out (`docs/design/agents.md`). And fetching is an external effect, which is a
capability. A `sic` that quietly reached the network to replace itself would be
the one thing every other part of this design is arranged to prevent.

So downloading is somebody else's job. `sic` verifies and swaps.

```console
$ tar xzf sic-v0.1.1-x86_64-unknown-linux-musl.tar.gz
$ sic update --check --to sic-v0.1.1-x86_64-unknown-linux-musl/sic --sha256 88eb87ac...
  installed  0.1.0  sha256:04c4682a...  /home/me/.local/bin/sic
  candidate  0.1.1  sha256:88eb87ac...  sic-v0.1.1-x86_64-unknown-linux-musl/sic
would replace /home/me/.local/bin/sic

$ sic update --to sic-v0.1.1-x86_64-unknown-linux-musl/sic --sha256 88eb87ac...
  installed  0.1.0  sha256:04c4682a...  /home/me/.local/bin/sic
  candidate  0.1.1  sha256:88eb87ac...  sic-v0.1.1-x86_64-unknown-linux-musl/sic
replaced /home/me/.local/bin/sic  0.1.0 -> 0.1.1
```

This is `process.exec`'s pinning applied to `sic` itself: what runs is decided
by what the file **is**, not by where it came from. `--check` shows what would
happen before it happens, which is what `sic plan` does for programs.

---

## 1. The digest is not optional

`--to` without `--sha256` is a usage error, not a prompt to continue anyway.

Without it there is nothing to verify against, and an update mechanism that
installs whatever it was handed is a way to replace a binary, not a way to
update one. The digest has to come from somewhere the user already trusts:
every release publishes `SHA256SUMS`, covering both each archive and the binary
inside it, so the digest is a published fact rather than something the updater
computed and then vouched for itself. Publishing only the archives' digests
would have left this check with nothing to compare against, which is why the
release workflow unpacks them before hashing.

`sic` does not read `SHA256SUMS` for you. Parsing the file that ships next to
the download, to check the download, would only be checking the download against
itself. Copying one line out of a release page is the point where a person looks
at what they are about to install.

---

## 2. The order the checks happen in

Each step is refused before the next one can matter.

1. **Where the installed binary is.** `std::env::current_exe`, canonicalized.
2. **Whether anything else owns it** (§4). A refusal here happens before the
   candidate is read at all.
3. **What the candidate hashes to.** Read in chunks, compared with `--sha256`.
   A mismatch prints both digests and stops.
4. **Whether it is already installed.** Equal digests mean there is nothing to
   do, which is a success, not an error.
5. **What the candidate says it is.** The verified bytes are written next to the
   destination, made executable, and run as `sic version`. Output that does not
   read as `sic <version>` stops the update.
6. **The swap** (§3).

Step 5 is where "running an unverified binary to ask what it is" would have been
exactly backwards. It runs *after* the digest matched, so what executes is the
bytes the user vouched for, and it is the same file that the rename then puts in
place - not a second copy that might differ.

The staging file is written into the destination directory, because a rename
across filesystems is not one. `--check` does everything except the final
rename, and removes the staging file afterwards, so a `--check` that cannot
write to `/usr/local/bin` has told you something true about the update.

---

## 3. Replacing a file that is running

On Unix, a running executable cannot be **written to** - the kernel answers
`ETXTBSY`, as the broker's tests found the hard way - but the directory entry
can be replaced. Writing a staging file and renaming it over the destination is
therefore both allowed and atomic: a reader either sees the whole old file or
the whole new one, and a process already running the old inode keeps running it
until it exits.

Windows will not let a running image be replaced or deleted, but it does allow
it to be **renamed**. So there the old binary is moved aside first, the new one
renamed into place, and the leftover deleted if the operating system will part
with it - it will not while the process that is doing the update is still the
one running from it, and that leftover is reported rather than hidden.

That path is written and is not exercised by the tests here: CI runs on Linux,
and a test that replaces a running binary only means something on the platform
it runs on.

---

## 4. What it will not touch

If the installed path is under a package manager's control, `sic update`
refuses and names the manager:

| Path | Owner |
|---|---|
| `/nix/store/...` | Nix |
| `.../.cargo/bin/...` | `cargo install` |
| `/opt/homebrew/...`, `.../Cellar/...`, `/home/linuxbrew/...` | Homebrew |
| `/snap/...` | snap |
| `/var/lib/flatpak/...` | Flatpak |
| `/usr/bin/...`, `/bin/...` | the system package manager |

Swapping the file would leave the manager's record describing something that is
no longer there, and the next upgrade would silently undo the update. There is
no flag to override this: a person who wants that file replaced can copy it,
which makes it their decision rather than a decision `sic` made about their
system.

Privileges work the same way. `sic` never asks for any, so an update into a
directory it cannot write to fails with the reason. Failing clearly beats
succeeding surprisingly.

---

## 5. Not here

- **No fetching.** Nothing in `sic` opens a socket, and this does not either.
- **No automatic check.** Nothing runs on a timer, and nothing reports a version
  anywhere. `sic update --check` runs when a person runs it.
- **No signature on the artifacts.** Commits and tags are signed, so a release
  says who cut it. That is not the same as saying who built what is attached to
  it: those bytes come from GitHub's runners, and nothing here rebuilds them to
  check. Within the release, a digest says the file is the file that was
  published, which is what `--sha256` verifies. Signing the artifact itself
  would need a key the build can reach, somewhere to publish the public half,
  and a decision about rotation - a design of its own, and the bytecode format
  already has an empty signature section waiting for the same one.
- **No reading `SHA256SUMS`** (§1).
- **No rollback.** The previous binary is not kept on Unix, because a copy of a
  release is a thing the release already provides. On Windows the moved-aside
  file remains only because the operating system would not delete it yet.
- **No timeout on the identify step.** A candidate that hangs on `sic version`
  hangs the update. The bytes were verified first, so this is a bad release,
  not an attack the digest missed.
