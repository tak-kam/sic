# sic

A language and runtime for AI agents, workflows, and automation. See
[docs/design/v0.1.md](docs/design/v0.1.md) for the design of the current version.

## Repository language: English only

Everything committed to this repository is written in English:

- code comments and doc comments
- identifiers and test names
- `README.md` and everything under `docs/`
- commit messages and PR descriptions
- user-facing output of the `sic` binary (diagnostics, help text, error messages)
- comments inside `.sic` example programs

Conversation with the user happens in Japanese; only repository content is English.
When editing a file that still contains Japanese, translate it rather than leaving it.

The one exception is non-ASCII text used as test *data* -- multi-byte column
arithmetic, rejecting non-ASCII identifiers, and similar cases -- where the
characters are the thing under test.

## No external dependencies

`[dependencies]` stays empty in every crate. Supply chain attacks are treated as a
primary risk, so the lexer, parser, type checker, IR, bytecode compiler, verifier,
VM, JSON handling, scheduler, and journal are all written by hand.

Never add a crate because it is convenient. To propose one, first document in
`docs/design/`:

1. why it is needed
2. why `std` alone is insufficient
3. how much the dependency tree grows
4. the cost of implementing it by hand
5. the security impact

## Structure

```text
Source -> Lexer -> Parser -> AST -> Type Checker -> IR
       -> Bytecode -> Verifier -> VM -> Capability Broker
```

| crate | role |
|-------|------|
| `sic-core` | `Span`, `SourceFile`, `Diagnostic`, IDs, SHA-256, the binary reader/writer, the capability value types |
| `sic-syntax` | lexer, AST, parser (recursive descent; Pratt for expressions only) |
| `sic-types` | interned types, type checking, name resolution, trust |
| `sic-ir` | the high-level IR, where workflow semantics still exist |
| `sic-bytecode` | instruction set, the `.sicb` format, disassembler |
| `sic-compile` | HIR to bytecode |
| `sic-verify` | the bytecode verifier |
| `sic-vm` | the register VM, tasks, checkpoints |
| `sic-broker` | performs capability calls |
| `sic-journal` | the execution journal |
| `sic-json` | JSON: a parser, for what a model answers with, and the escaping every writer in the workspace uses |
| `sic-otel` | journal to OTLP traces and metrics |
| `sic-plan` | what a program may do, read from its bytecode |
| `sic-cli` | the `sic` binary |

Three boundaries must hold, and each is checked by a test rather than left as an
intention:

- **Only `sic-broker` and `sic-cli` touch the outside world.** Every other crate
  is a pure function of its input; that is what makes the capability boundary
  mean anything (`crates/sic-core/tests/workspace.rs`).
- **`sic-vm` never depends on `sic-broker`.** The VM suspends at an effect, the
  driver asks the broker. That boundary is where the two will later split into
  separate processes (`crates/sic-vm/tests/isolation.rs`).
- **`sic-core` depends on nothing else in the workspace.**

The journal records digests, never values. A checkpoint and a recorded run's
`responses.jsonl` hold values, and that difference is deliberate: see
`docs/design/runs.md`.

## Commands

```text
sic run <FILE.sic> [--journal P] [--checkpoint P] [--record] [--llm SPEC]
sic resume <CHECKPOINT> <FILE.sic> --value <V>
sic plan <FILE.sic|FILE.sicb>     what a program may do, running nothing
sic runs [--waiting] | attach <ID> [--value V] [--because WHY] [--llm SPEC]
sic explain <ID> | inspect-run <ID> | replay <ID>
sic export <JOURNAL> [--traces P] [--metrics P]
sic upgrade [--check] | --to FILE --sha256 HEX
sic compile | verify | disasm | parse | hir
sic mcp                           the capabilities a run granted, served to the
                                  agent answering for it
```

Exit code 3 means a run was suspended and checkpointed - waiting is not failing.

## Design documents

`docs/design/` holds one per phase, and each records what was deliberately left
out and why. `docs/status.md` says where each section of the specification
stands - read it before deciding what to work on. `docs/diagnostics.md` indexes
every diagnostic code, and a test fails if it drifts from the source.

## Issues

One issue is one piece of work. When a review, an audit or a survey turns up a
list of improvements, the list is not the issue: each item that could be picked
up and finished on its own gets its own issue, with the argument for it written
out. A summary issue may hold the whole picture and link to them.

The reason is the same one the design documents are written for. An issue
carrying eight loosely related items records no decision about any of them, is
never closed, and cannot be handed to anybody - so the work it describes does
not happen. An issue that argues for one change either convinces its reader or
is closed with a reason, and both of those are progress.

Keep an issue in the register of the design documents: prose that argues, a
table where a table earns its place, and a section on what is deliberately not
in it.

## One issue, one worktree

Work on an issue in a git worktree of its own, not in the checkout you happen to
be standing in:

```console
$ git worktree add ../sic-19 -b issue-19
$ cd ../sic-19
```

Two reasons, and the first is the one that decides. **More than one piece of
work runs at a time here** - a fix, a survey, a documentation pass - and they
share a filesystem. Two of them editing one checkout means `git add -A` stages
somebody else's half-finished change, `cargo fmt --all` reformats a file being
written, and a test failure belongs to whoever ran it last. None of that is
recoverable from the commit history, because it happens before the commit.

The second: a worktree makes abandoning a line of work free. Delete it. The
alternative is a stash nobody remembers the reason for.

Branch per issue, named for it (`issue-19`), merged to `main` when it is done
and the worktree removed with `git worktree remove`. `main` refuses force
pushes, so the branch is where a history is still allowed to be untidy.

## CI

`.github/workflows/ci.yml` runs formatting, clippy with warnings denied, the
tests, a check that `[dependencies]` names only workspace paths, and a check
that `Cargo.lock` agrees with the manifests. `main` refuses force pushes and
deletion, so a mistake in a commit message is fixed by another commit rather
than by rewriting history.

Commits and tags are signed with an SSH key (`gpg.format ssh`), and `main`
requires signatures, so who made a commit is a checkable fact rather than a name
typed into a field. A release tag is a signed object; the binaries attached to it
are pinned by digest instead, and `docs/design/upgrade.md` says why those are two
different claims.

That means a machine without the signing key cannot push to `main`. Set
`user.signingkey`, `gpg.format ssh` and `commit.gpgsign` there too, rather than
turning the requirement off.

## Building

`cargo test` needs a C linker (`cc`). On a machine without one, link with the
`rust-lld` shipped by rustup against the musl target:

```console
$ rustup target add x86_64-unknown-linux-musl
$ LLD="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
$ RUSTFLAGS="-Clinker=$LLD -Clinker-flavor=ld.lld" \
    cargo test --target x86_64-unknown-linux-musl
```

## Implementation priorities

Simple, small, explicit, deterministic, testable, dependency-free, auditable.
Do not build generic abstractions for features that do not exist yet.

Small is about what a function does, not how many lines it takes to do it. A
`match` arm is never extracted for length; a procedure two or more arms share
is, so that the order between the steps can be checked in one place. The six
longest functions here are exhaustive matches and are finished as they are -
see [docs/design/extraction.md](docs/design/extraction.md).
