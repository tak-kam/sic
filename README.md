# sic

[![CI](https://github.com/tak-kam/sic/actions/workflows/ci.yml/badge.svg)](https://github.com/tak-kam/sic/actions/workflows/ci.yml)

A language and runtime for AI agents, workflows and automation, where **what a
program may do** is part of the program.

```text
allow {
    llm.invoke "claude-opus-4";
    human.approve "deploying";
    process.exec "/usr/bin/deploy";
}

agent make_plan {
    input: String,
    output: Plan,
    budget: 2,
}

fn deploy(plan: HumanApproved<Plan>) -> Int {
    return process.exec("/usr/bin/deploy");
}

fn main() -> Int {
    let plan = make_plan("what should we deploy?");
    return deploy(approve("deploy this?", plan));
}
```

Passing `plan` to `deploy` without `approve` does not compile. Calling anything
the `allow` block does not name does not compile. What the program may reach is
readable from the compiled bytecode, before it runs, without running it.

---

## Why

Most of what a workflow does is reach outside itself: a file, a process, an API,
a model, a person. The interesting questions are not about the arithmetic.

- What is this program allowed to touch?
- On what grounds did it do that?
- What actually happened?

sic answers those in the language and the runtime rather than in convention. An
effect has to be declared before it can be called; a value carries where it came
from; a run keeps its own account of itself and can be stopped, moved and
resumed.

The implementation has **zero external crates**. Supply chain attacks are
treated as a primary risk, so the lexer, parser, type checker, IR, bytecode
compiler, verifier, VM, JSON parser, SHA-256, scheduler and journal are all
written by hand.

## Install

From a release, which publishes a static Linux binary, both macOS
architectures, and Windows, with a `SHA256SUMS` beside them:

Driving an agent needs a unix socket and tmux, so `--llm`, `sic mcp` and
`sic hook` are unix-only and say so on Windows rather than being missing.
Everything else - compiling, running, planning, recording, replaying - is the
same everywhere. CI compiles for all four targets, which is how that stays
true.

```console
$ tar xzf sic-v0.5.0-x86_64-unknown-linux-musl.tar.gz
$ ./sic-v0.5.0-x86_64-unknown-linux-musl/sic version
```

Or with cargo, which needs Rust 1.85 or newer - CI compiles against exactly
that version, so it is the real minimum rather than the one somebody guessed:

```console
$ cargo install --git https://github.com/tak-kam/sic sic-cli
```

Or from source:

```console
$ git clone https://github.com/tak-kam/sic
$ cd sic
$ cargo build --release
$ ./target/release/sic run examples/milestone.sic
30
```

Without a C linker, link with the `rust-lld` that rustup already ships:

```console
$ rustup target add x86_64-unknown-linux-musl
$ LLD="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
$ RUSTFLAGS="-Clinker=$LLD -Clinker-flavor=ld.lld" \
    cargo build --release --target x86_64-unknown-linux-musl
```

### Upgrading

```console
$ sic upgrade
  installed  0.1.1  sha256:975d6bcb...  /home/me/.local/bin/sic
fetching v0.5.0 for x86_64-unknown-linux-musl
  candidate  0.5.0  sha256:88eb87ac...  sic-v0.5.0-x86_64-unknown-linux-musl/sic
replaced /home/me/.local/bin/sic  0.4.0 -> 0.5.0
```

`sic` does not speak HTTP: it runs `curl` at an absolute path, only when this
command is the one that was typed. The runtime still has no network capability,
nothing checks for updates on a timer, and a program you run reaches nothing.
Every download is checked against the digests the release publishes, before the
archive is unpacked and again before the binary is installed.

`--check` says what would happen and changes nothing. `--to FILE --sha256 HEX`
does the same from a file already on disk, touching no network at all. A binary
that cargo or a package manager installed is left to the thing that installed
it.
→ [upgrade.md](docs/design/upgrade.md)

## A first program

```text
// hello.sic
allow {
    fs.read "./hello.sic";
}

fn main() -> Int {
    return len(fs.read("./hello.sic"));
}
```

```console
$ sic plan hello.sic          # what it may do, running nothing
Execution plan for hello.sic
bytecode sha256:...

  main
    1. READ   fs.read  "./hello.sic"

$ sic run hello.sic
112
```

It reads its own source and returns its length.

Remove the `allow` block and it stops compiling, with the fix in the message.

## What it does

**Capabilities.** Every external effect is declared, typed and constrained.
Calling one the module did not grant is a compile error, so the manifest of a
compiled module is complete by construction. `process.exec` takes an absolute
path, never searches `PATH`, can pin the binary's sha256, and can pin what its
arguments must start with - a grant on `tmux` that cannot say which pane is a
grant to drive every pane on the machine. A grant that reads something back can
also say what form the answer takes - `answers json`, `answers jsonl` - and the
broker holds the program to it, so a workflow that parses another program's
output says so in a plan instead of finding out on the day the wording changes.
→ [capabilities.md](docs/design/capabilities.md),
[arguments.md](docs/design/arguments.md),
[answers.md](docs/design/answers.md)

**Granting git is closer to granting arbitrary execution than it looks.**
`core.pager`, `diff.external` and an alias are command lines in a config file;
`.git/hooks` holds executables that arrived with the repository;
`credential.helper` and `protocol.ext` name programs too. A manifest can pin
the binary and clear the environment and reach none of them. So `git.status`
and `git.rev_parse` are capabilities of their own: the broker builds the
command line, and every call turns all of that off. That is the test a program
has to pass to get a capability rather than a `process.run` grant - the broker
must be able to enforce something the manifest cannot say - and `cargo` and
`npm` do not pass it.
→ [git.md](docs/design/git.md)

**Reading what a program said is its own grant.** `process.capture` returns
standard output, and only when the program succeeded. An exit code is one bit;
standard output is everything the program can see, so the two are different
authorities and `sic plan` prints them differently. What comes back is
`Observed<String>`, which cannot decide what the next program runs without a
person on the record.

`process.run` returns both - `Exit { code: Int, output: Observed<String> }` -
because a build that fails and prints why is the case neither of the other two
could reach. It is a third grant, not a flag, because it is strictly more
authority than either.
→ [output.md](docs/design/output.md)

**Modules that cannot grant themselves anything.** `import "./lib/deploy.sic";`
brings a local file in. A library declares what it needs with `requires`, and
the program that is run is the only file with an `allow` block, so the manifest
stays one list in one place and `sic plan` says which file spends each grant.
There is no registry, no version and no network resolution.
→ [modules.md](docs/design/modules.md)

**The VM cannot reach outside.** It suspends at an effect and something else
performs it. That boundary is checked by a test, not by convention, and it is
where the VM and the broker will later split into separate processes.

**Durable execution.** A run that cannot finish now is written to a checkpoint
and continues later, in another process, on another day. Nothing had to be added
to the VM for this: suspending was already how it worked.
→ [durable-execution.md](docs/design/durable-execution.md)

**Tasks.** `spawn` and `await` over a cooperative scheduler. What is made
concurrent is waiting, not computing: while one task is stopped at a capability
call, another runs. No OS threads, no async runtime.
→ [concurrency.md](docs/design/concurrency.md)

**Agents with typed output.** An agent is a model call and a validation. What
comes back is text; what the program gets is a value that fit a declared type,
and a run fails at the model boundary rather than three steps later. A type may
also say it describes *part* of a document - `type Line { reason: String, .. }`
- because an extra field in a model's answer means the model answered a
different question, and an extra field in a machine protocol means the protocol
has more in it than this program asked about. Those are opposite conventions
and one validator now serves both. A field may also say it is sometimes not
there - `executable: String?` - which is the other half of the same
disagreement: `a.executable?` asks whether the document carried it, and
`a.executable` fails the run at a named line rather than inventing a value,
which is the decision `xs[i]` already made.
→ [agents.md](docs/design/agents.md)

**A list is walked, and a string can be asked what it holds.** `for x in xs`
has no frame per element, so a list longer than the call stack can be walked -
which recursion could not do. `==`, `contains`, `starts_with` and `+` are the
whole of what a program may do to a string, and each was added because a
workflow could not otherwise ask a question it needed: is this the branch we
deploy from, did the build print a warning, is this path inside that directory.
Joining is charged a fuel per byte, so the instruction budget bounds the arena.
→ [v0.1.md](docs/design/v0.1.md), [alternatives.md](docs/design/alternatives.md)

**A model call answered by an agent CLI.** `sic run p.sic --llm tmux:claude`
puts the prompt in front of a real coding agent in a tmux pane and reads the
answer back, instead of stopping so a person can paste one in. The multiplexer
lives in the broker rather than in the language: a program granted
`process.exec "/usr/bin/tmux"` could reach every pane on the machine. Nothing
answers unless it was asked for by name.
→ [driving.md](docs/design/driving.md)

**Trust and provenance.** `LLM<T>` is attached by an agent, `HumanApproved<T>`
by `approve`, and a model's answer cannot reach a capability that changes
something. Reading a field keeps the label. It is all erased before the bytecode:
the rule is about the program, not about a run.
→ [trust.md](docs/design/trust.md)

**A journal, not instrumentation.** The runtime produces the events, so a
program needs none. It records digests, never values, because telemetry is an
exfiltration path like any other. Traces and metrics are a view of it.
→ [observability.md](docs/design/observability.md)

**Runs you can come back to.** `sic runs --waiting` says what is waiting and for
what; `sic attach <id>` answers it. Reading the question is a separate step from
answering it, which is what makes it usable by something other than a person.
`sic replay <id>` re-runs the stored bytecode against the stored answers and
compares - which is a check on determinism, and calls nothing.

**A recorded run is a test case.** `sic recheck <id> <file.sic>` runs an edited
program against the same recorded answers and says where it stops asking what
the recording answered. A directory of recorded runs is then a regression suite
for the program, made of cases it has actually been through.
→ [runs.md](docs/design/runs.md)

**What a program may do, drawn.** `sic plan --graph` writes the same plan as a
Mermaid flowchart, which says the one thing a list of functions side by side
cannot: which of them reach which, and so which effect is only reachable from
behind an approval. It renders in GitHub and most editors with nothing
installed, and where nothing renders it is still readable. The first node in
the diagram says "may, not will" - an arrow is much harder to qualify than a
sentence, and a plan that over-claims is as useless as one that under-reports.
→ [plan.md](docs/design/plan.md)

**And a person who is present can just answer.** `sic run p.sic --record
--interactive` asks this terminal when the run stops, and keeps asking for as
long as it keeps stopping. The checkpoint is written before the question
appears, so Ctrl-C, a dropped connection or a laptop that sleeps leaves exactly
what a non-interactive run would have left - the worst case of an interactive
run is a non-interactive one. Answer a workflow's questions once this way and
`sic recheck` has a regression test that cost nothing to make.
→ [interactive.md](docs/design/interactive.md)

**And all of that is one thing: a harness that can be read before it runs.**
The field's word for the scaffold around a model call covers the retry, the
budget, the validation, the tool list, the person who approves the dangerous
step and the record of what happened - everywhere else an object graph the
program assembles at run time, and readable only by running it. Here it is a
declaration in the bytecode, so `sic plan` prints it without constructing a VM
and the plan and the run cannot disagree: they are the same bytes. And because
`--record` keeps what a run was answered, `sic recheck` turns a harness into a
regression test - change a retry threshold and a run from last week says which
call it stopped asking. `workflows/harness.sic` is one written and run, and the
document reports what could not be written as carefully as what could.
→ [harness.md](docs/design/harness.md)

## Commands

```text
sic run <FILE.sic> [--journal P] [--checkpoint P] [--record] [--llm SPEC] [--no-isolate]
                                   [--interactive]  ask this terminal, and keep asking
sic plan <FILE.sic|FILE.sicb> [--graph]
                                   what a program may do, running nothing;
                                   --graph draws which functions reach which
sic runs [--waiting]               what has been recorded, or what is waiting
sic attach <RUN-ID> [--value V] [--because WHY] [--llm SPEC] [--interactive]
                                   see what a waiting run needs, or answer it
sic resume <CHECKPOINT> <FILE.sic> --value <V> [--no-isolate]
sic explain <RUN-ID> | inspect-run <RUN-ID> | replay <RUN-ID>
sic recheck <RUN-ID> <FILE.sic>     does this edit still ask what the run answered
sic vm --socket P                  the interpreter, started by a run rather than a person
sic export <JOURNAL> [--traces P] [--metrics P]
sic upgrade [--check] | --to FILE --sha256 HEX
sic compile | verify | disasm | parse | hir
```

Exit code 3 means a run was suspended and checkpointed. Waiting is not failing.

## Documentation

| | |
|---|---|
| [status.md](docs/status.md) | where each part of the design stands |
| [v0.1.md](docs/design/v0.1.md) | the language, the bytecode, the VM, the verifier |
| [capabilities.md](docs/design/capabilities.md) | how effects are declared and performed |
| [durable-execution.md](docs/design/durable-execution.md) | suspend, checkpoint, resume |
| [concurrency.md](docs/design/concurrency.md) | tasks, retry, timeout |
| [agents.md](docs/design/agents.md) | structured output and agents |
| [harness.md](docs/design/harness.md) | the scaffold around a model call, as a declaration rather than an object graph - and one written, run, and reported on |
| [trust.md](docs/design/trust.md) | provenance in the type system |
| [observability.md](docs/design/observability.md) | the journal and OpenTelemetry |
| [runs.md](docs/design/runs.md) | recorded runs, attach, replay |
| [interactive.md](docs/design/interactive.md) | answering a run from the terminal it is running in |
| [logging.md](docs/design/logging.md) | what a program has to say about itself, and where it goes |
| [processes.md](docs/design/processes.md) | what splitting the VM from the broker buys, and what is already true without it |
| [plan.md](docs/design/plan.md) | `sic plan`, and `--graph` |
| [driving.md](docs/design/driving.md) | answering a model call with an agent CLI in a pane |
| [authority.md](docs/design/authority.md) | what the agent answering may do, and who decides |
| [arguments.md](docs/design/arguments.md) | what a program may be told, and what a grant pins about it |
| [output.md](docs/design/output.md) | reading what a program said, and what that makes the value |
| [answers.md](docs/design/answers.md) | what shape a program answers in, and what a grant may claim about it |
| [git.md](docs/design/git.md) | `git`, and when a program deserves a capability |
| [decisions.md](docs/design/decisions.md) | `choose`, and recording what was not chosen |
| [checking.md](docs/design/checking.md) | whether a label may be discharged by evidence instead of a person |
| [alternatives.md](docs/design/alternatives.md) | a value that is one of several shapes, and how a program takes one apart without pattern matching |
| [upgrade.md](docs/design/upgrade.md) | `sic upgrade`: fetch, verify, swap |
| [extraction.md](docs/design/extraction.md) | why the longest functions are the right length |
| [self-hosting.md](docs/design/self-hosting.md) | writing this repository's own development loop in sic, and the seven things that bent it |
| [diagnostics.md](docs/diagnostics.md) | every diagnostic code |

Each design document records what was deliberately left out, and why. That is
usually the more useful half.

## How it is built

```text
Source → Lexer → Parser → AST → Type Checker → IR
       → Bytecode → Verifier → VM → Capability Broker
```

Fourteen crates, no external dependencies, 37 instructions in a register VM that
only runs bytecode a verifier has accepted. Three boundaries are enforced by
tests rather than left as intentions:

- only the broker and the CLI touch the outside world
- the VM never depends on the broker
- `sic-core` depends on nothing else in the workspace

## Principles

Simple, small, explicit, deterministic, testable, dependency-free, auditable.

- No implicit network access, no implicit credentials, no runtime dependency
  resolution, no dynamic plugin loading, no `PATH`-based executable resolution
- No capability without an explicit declaration
- No bytecode execution without verification
- No secrets in telemetry by default
- No abstraction built for a feature that does not exist yet

Adding a dependency means first documenting, in `docs/design/`: why it is needed,
why `std` is insufficient, how much the dependency tree grows, the cost of
writing it by hand, and the security impact. "It is convenient" is not an
argument.

## Status

Early, and honest about it. Phases 1 to 8 of the design are implemented and
`docs/status.md` says exactly what is not.

Not a stable language. Not benchmarked. Not something to run untrusted code with
yet, though most of the machinery for that is the point of the design.

## License

Licensed under either of

- Apache License, Version 2.0
  ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license
  ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
