# `sic upgrade`

```console
$ sic upgrade
  installed  0.1.1  sha256:975d6bcb...  /home/me/.local/bin/sic
fetching v0.1.2 for x86_64-unknown-linux-musl
  candidate  0.1.2  sha256:88eb87ac...  sic-v0.1.2-x86_64-unknown-linux-musl/sic
replaced /home/me/.local/bin/sic  0.1.1 -> 0.1.2
```

One command, and every step of it says what it did.

---

## 1. sic does not speak HTTP

Fetching a release means HTTPS, which means TLS, which is not something to write
by hand for this - the same wall that made `llm.invoke` defer rather than call
out (`docs/design/agents.md`).

So `sic` does not. It runs `curl`, found at an absolute path, with the
environment cleared down to the few variables a fetch needs. That is the
arrangement `process.exec` already makes for a sic program: the effect is
performed by something outside, and only because somebody asked for it.

This is worth being exact about, because §33 says **no implicit network
access** and that is still true:

- **The runtime has no network.** A sic program cannot open a socket. There is
  no capability for it, so nothing a program does can acquire one.
- **The tool reaches the network only when this command is the one typed.**
  Nothing runs on a timer, nothing checks for updates in the background, and
  nothing reports anything anywhere. `sic run` fetches nothing.
- **Where it fetches from is compiled in.** An updater that can be pointed at
  another host is a way to install something else.

`sic upgrade --to FILE --sha256 HEX` never touches the network at all: the file
is already on disk, and the digest came from wherever the person got it. That
path came first, and it is still the one to use on a machine that should not be
reaching out.

---

## 2. The order the checks happen in

Each step is refused before the next one can matter.

1. **Where the installed binary is.** `std::env::current_exe`, canonicalized.
2. **Whether anything else owns it** (§5). A refusal here happens before
   anything is downloaded.
3. **Which release, and whether it is newer.** The tag comes from the GitHub
   API. The same tag means there is nothing to do; an *older* tag is refused
   rather than installed, because a binary built from a branch is ahead of every
   release and moving it backwards under the word "upgrade" would be the
   surprising kind of success.
4. **The archive, against `SHA256SUMS`.** Checked before it is unpacked:
   handing an unverified archive to `tar` trusts it with more than a digest
   comparison needs.
5. **The binary inside it, against the same list.** A name `SHA256SUMS` does not
   mention is a failure, not a reason to install something unchecked. Neither is
   a name it mentions twice with two different digests: a list that gives one
   file two answers has not said which, and picking one would be sic deciding
   on its behalf.

Steps 4 and 5 are one function, `checked`, taking the name, the list and the
file. It is separate from the fetch around it so that it can be run without a
network - welded in after two downloads, the comparison the whole feature rests
on was the one part of it no test could reach.
6. **What the candidate says it is.** The verified bytes are written next to the
   destination, made executable, and run as `sic version`. Output that does not
   read as `sic <version>` stops the upgrade.
7. **The swap** (§4).

Step 6 is where "running an unverified binary to ask what it is" would have been
exactly backwards. It runs *after* the digests matched, so what executes is what
was published, and it is the same file the rename then puts in place - not a
second copy that might differ.

The staging file is written into the destination directory, because a rename
across filesystems is not one. `--check` does everything except the final
rename, and removes the staging file afterwards, so a `--check` that cannot
write to `/usr/local/bin` has told you something true about the upgrade.

---

## 3. What the digest proves, and what it does not

`SHA256SUMS` comes from the release it describes. Checking a download against it
therefore proves that **the bytes arrived intact**: a truncated transfer, a
corrupted mirror, a proxy that mangled the file. It does not prove who made
them. Somebody who could replace the binary on the release page could replace
the digest list next to it.

What stands between that and this machine is TLS, and GitHub's control of its
own release storage. That is the same amount of trust `cargo install`, `brew
upgrade` and every `curl | sh` rely on. It is worth saying out loud rather than
letting a digest imply more than it carries.

The honest fix is a signature, which says *who* published the bytes rather than
*which* bytes they are. Commits and tags in this repository are already signed,
so a release tag says who cut it - but the binaries are built by GitHub's
runners from that tag, and nothing here rebuilds them to check, so the tag's
signature does not reach them. Closing that gap needs a key the release build
can use, a public half published where an old binary can already have it, and a
decision about rotation. The bytecode format has an empty signature section
waiting for the same design.

Until then, `--to` with a digest from somewhere else is the stronger path: two
sources have to agree instead of one.

---

## 4. Replacing a file that is running

On Unix, a running executable cannot be **written to** - the kernel answers
`ETXTBSY`, as the broker's tests found the hard way - but the directory entry
can be replaced. Writing a staging file and renaming it over the destination is
therefore both allowed and atomic: a reader either sees the whole old file or
the whole new one, and a process already running the old inode keeps running it
until it exits.

Windows will not let a running image be replaced or deleted, but it does allow
it to be **renamed**. So there the old binary is moved aside first, the new one
renamed into place, and the leftover deleted if the operating system will part
with it - it will not while the process doing the upgrade is still running from
it, and that leftover is reported rather than hidden.

That path is written and is not exercised by the tests here: CI runs on Linux,
and a test that replaces a running binary only means something on the platform
it runs on.

---

## 5. What it will not touch

If the installed path is under a package manager's control, `sic upgrade`
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
no longer there, and its next upgrade would silently undo this one. There is no
flag to override it: a person who wants that file replaced can copy it, which
makes it their decision rather than a decision `sic` made about their system.

Privileges work the same way. `sic` never asks for any, so an upgrade into a
directory it cannot write to fails with the reason. Failing clearly beats
succeeding surprisingly.

---

## 6. Not here

- **No automatic check.** Nothing runs on a timer, and nothing reports a version
  anywhere. `sic upgrade` runs when a person runs it.
- **No signature** (§3).
- **No mirror, no proxy setting, no channel.** One place to fetch from, chosen
  at compile time.
- **No rollback.** The previous binary is not kept on Unix, because a copy of a
  release is a thing the release already provides. On Windows the moved-aside
  file remains only because the operating system would not delete it yet.
- **No timeout on the identify step.** A candidate that hangs on `sic version`
  hangs the upgrade. The bytes were verified first, so this is a bad release,
  not an attack a digest missed.
- **No `update` alias.** It was called `sic update` in 0.1.1 and is called
  `sic upgrade` now. Carrying both names forever to save one release's worth of
  friction is how a CLI gets a surface nobody can describe.
