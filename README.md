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

```console
$ tar xzf sic-v0.1.0-x86_64-unknown-linux-musl.tar.gz
$ ./sic-v0.1.0-x86_64-unknown-linux-musl/sic version
```

Or with cargo, which needs Rust 1.85 or newer:

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
99
```

It reads its own source and returns its length.

Remove the `allow` block and it stops compiling, with the fix in the message.

## What it does

**Capabilities.** Every external effect is declared, typed and constrained.
Calling one the module did not grant is a compile error, so the manifest of a
compiled module is complete by construction. `process.exec` takes an absolute
path, never searches `PATH`, and can pin the binary's sha256.
→ [capabilities.md](docs/design/capabilities.md)

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
and a run fails at the model boundary rather than three steps later.
→ [agents.md](docs/design/agents.md)

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
→ [runs.md](docs/design/runs.md)

## Commands

```text
sic run <FILE.sic> [--journal P] [--checkpoint P] [--record]
sic plan <FILE.sic|FILE.sicb>      what a program may do, running nothing
sic runs [--waiting]               what has been recorded, or what is waiting
sic attach <RUN-ID> [--value V]    see what a waiting run needs, or answer it
sic resume <CHECKPOINT> <FILE.sic> --value <V>
sic explain <RUN-ID> | inspect-run <RUN-ID> | replay <RUN-ID>
sic export <JOURNAL> [--traces P] [--metrics P]
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
| [trust.md](docs/design/trust.md) | provenance in the type system |
| [observability.md](docs/design/observability.md) | the journal and OpenTelemetry |
| [runs.md](docs/design/runs.md) | recorded runs, attach, replay |
| [plan.md](docs/design/plan.md) | `sic plan` |
| [diagnostics.md](docs/diagnostics.md) | every diagnostic code |

Each design document records what was deliberately left out, and why. That is
usually the more useful half.

## How it is built

```text
Source → Lexer → Parser → AST → Type Checker → IR
       → Bytecode → Verifier → VM → Capability Broker
```

Fourteen crates, no external dependencies, 30 instructions in a register VM that
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
