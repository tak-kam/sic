# sic

A language and execution environment for AI agents, workflows, and automation.

The goal is not only to express *what to compute*, but to make **what may be
executed**, **on what grounds it was executed**, and **what actually happened**
first-class concerns of the language and its runtime.

- Capability-based security: every external effect must be declared
- A register-based VM running verified, purpose-built bytecode
- Observability, audit, and replay derived from a single execution journal
- Durable execution (suspend / save / resume)

The implementation is Rust with **zero external crates**, because supply chain
attacks are treated as a primary risk.

## Status: phase 8 (OpenTelemetry)

```text
Source -> Lexer -> Parser -> AST -> Type Checker -> IR
       -> Bytecode -> Verifier -> VM
```

```console
$ sic run examples/milestone.sic
30
```

Bytecode can also be written, checked and read on its own:

```console
$ sic compile examples/factorial.sic -o factorial.sicb
wrote factorial.sicb (426 bytes)

$ sic verify factorial.sicb
ok: 2 function(s) verified
required capabilities:
  (none)

$ sic disasm factorial.sicb
...
  0000  LOAD_CONST  r1, k0  ; 1  ; 5:13
  0001  LE          r2, r0, r1  ; 5:8
  0002  JUMP_IF_NOT r2, +2  ; -> 0005  ; 5:8
```

### What a program may do, before it does any of it

```console
$ sic plan examples/agent.sic
Execution plan for examples/agent.sic
bytecode sha256:5da4f16d8b7cbfc800e8fcebd029dc59888cfb76968ea65c0348eebac2ac6325

  main
    1. INVOKE llm.invoke    "claude-opus-4"  at most 2 in a run   ; 31:13
    2. VERIFY Diagnosis   ; 31:13

Capabilities:
  llm.invoke    [invoke]  "claude-opus-4"

At most 2 capability call(s).
```

`sic plan` reads bytecode and runs nothing - no socket, no process, no VM -
which is what makes it worth having on a program nobody has decided to trust
yet. Everything it needs is already in the file, because each phase put it there
for this.

It says what a program **may** do, not what it will, and it is careful about the
difference. Only a `budget` bounds a call site over a whole run; `retry` bounds
one visit, and how many visits there are depends on the path taken. So sites
without a budget are counted and named rather than folded into a number that
would be a guess dressed as a fact. See [docs/design/plan.md](docs/design/plan.md).

### Effects are capabilities

Reaching outside the program takes a grant, and the grant is part of the
program:

```text
allow {
    fs.read "./examples/greeting.txt";
}

fn main() -> String {
    return fs.read("./examples/greeting.txt");
}
```

Calling a capability the module did not declare is a compile error, so the
manifest of a compiled module is complete by construction. `sic verify` reports
it without running anything:

```console
$ sic verify read-file.sicb
ok: 1 function(s) verified
required capabilities:
  fs.read [read] "./examples/greeting.txt"
```

A grant can also pin **what** runs, not just where to look:

```text
allow {
    process.exec "/usr/bin/true"
        sha256 "7e419b0d95e2ae12993878ad13ad4b911ce5464c15cde7a6adce2fca7ee22706";
}
```

The broker hashes the file on **every** call and refuses to run it if the digest
does not match - a check that ran earlier only tells you what was true earlier.

The VM never performs the effect itself. It suspends, the driver asks the
broker, and the broker decides again - the manifest is the contract between
them, not a formality the compiler already handled:

```text
CALL_CAP -> Suspended(request) -> broker -> resume(value) -> next instruction
```

`sic-broker` is the only crate that touches the outside world. See
[docs/design/capabilities.md](docs/design/capabilities.md).

### Where a value came from decides what it may be used for

```text
fn deploy(plan: HumanApproved<Plan>) -> Int { ... }

fn main() -> Int {
    let plan = make_plan(logs);                // LLM<Plan>
    let approved = approve("deploy this?", plan); // HumanApproved<Plan>
    return deploy(approved);
}
```

Passing `plan` straight to `deploy` does not compile:

```text
error[E0301]: expected HumanApproved<Plan>, found LLM<Plan>
```

The model's answer and the answer a person signed off are different values, and
a type system is where "different" is enforced rather than remembered. A
provenance is never annotated onto a value - it is attached by whatever produced
it - and reading a field keeps it: `LLM<Diagnosis>.cause` is `LLM<String>`.

`approve` is the only way to produce a `HumanApproved<T>`, and it needs the
`human.approve` capability like any other path to an effect. A model's answer
also cannot reach a `write` or `exec` capability at all.

**Trust is erased before the bytecode.** The rule being enforced is "this
program may not be written", which is a claim about the program rather than
about a run, so the VM has never heard of it. See
[docs/design/trust.md](docs/design/trust.md).

### Runs account for themselves

Observability is not an SDK bolted on afterwards: the runtime produces the
events, so a program needs no instrumentation to be observable.

```console
$ sic run examples/read-file.sic --journal run.jsonl
run 64ddb0176b1919f74b4b8812783de41b -> run.jsonl
"hello from a file\n"

$ cat run.jsonl
{"ts":...,"seq":0,...,"span":0,"parent":null,"event":"run_started","workflow":"main","args":"sha256:af5570f5..."}
{"ts":...,"seq":1,...,"span":1,"parent":0,"event":"function_entered","func":"main"}
{"ts":...,"seq":2,...,"span":2,"parent":1,"event":"capability_requested","cap":"fs.read","args":"sha256:88d243d0..."}
{"ts":...,"seq":3,...,"span":2,"parent":1,"event":"capability_completed","cap":"fs.read","result":"sha256:31ddb35c..."}
{"ts":...,"seq":4,...,"span":1,"parent":0,"event":"function_exited","func":"main"}
{"ts":...,"seq":5,...,"span":0,"parent":null,"event":"run_completed","result":"sha256:31ddb35c..."}
```

Events carry **digests, not values**: neither the path read nor the contents
that came back appear anywhere in that file. Telemetry is an exfiltration path
like any other, and a default that copies values into it is a default that leaks
secrets.

`seq` is the order. The timestamp is added by the sink as it writes, so the
journal itself reads no clock and a run stays reproducible - checked by a test
that fails if `sic-journal` ever mentions `std::time`.

This one stream is meant to be the single source for durability, tracing,
metrics, audit and replay, rather than separate mechanisms that have to agree.

### A run can outlive its process

Some effects cannot answer within the call - a person has to approve something.
The run stops, its state is written out, and it continues when the answer
arrives:

```console
$ sic run examples/approval.sic --checkpoint deploy.sicc --journal deploy.jsonl
waiting: [deploy to production] deploy build 42?
saved 274 bytes to deploy.sicc
$ echo $?
3

$ sic resume deploy.sicc examples/approval.sic --value true --journal deploy.jsonl
0
```

Nothing had to be added to the VM for this. Because it suspends rather than
calling the broker, everything needed to continue was already its state; a
checkpoint is that state written down. The journal carries on across the two
processes as one sequence, because a resumed run is the same run.

The checkpoint records the digest of the bytecode it came from, so a run cannot
be continued inside a program that has changed since. See
[docs/design/durable-execution.md](docs/design/durable-execution.md).

### Waiting concurrently

```text
allow { process.exec "/usr/bin/true"; }

fn check() -> Int {
    return process.exec("/usr/bin/true") retry 2;
}

fn main() -> Int {
    let a = spawn check();
    let b = spawn check();
    return await a + await b;
}
```

What is made concurrent is **waiting**, not computing. A workflow spends its
time on capability calls, and the point of two tasks is that one can proceed
while the other waits. There are no OS threads and no async runtime; the
scheduler is cooperative and a task yields only where it is already waiting, at
`CALL_CAP` and at `await`.

```text
seq= 7 task=1 capability_requested   cap=process.exec
seq= 8 task=2 capability_requested   cap=process.exec
seq= 9 task=1 capability_completed   cap=process.exec
seq=12 task=2 capability_completed   cap=process.exec
```

`retry` and `timeout` attach to a capability call, and only to one - retrying a
pure function computes the same answer again. They are enforced in different
places on purpose: **retry belongs to the VM**, which records every attempt, so
an audit shows what happened rather than only what worked; **timeout belongs to
the broker**, the only side with a clock. See
[docs/design/concurrency.md](docs/design/concurrency.md).

### An agent is not a function that returns a string

```text
type Diagnosis { cause: String, confidence: Float }

allow { llm.invoke "claude-opus-4"; }

agent diagnose {
    input: String,
    output: Diagnosis,
    budget: 2,
}

fn main() -> String {
    let d = diagnose("disk usage is at 100%");
    return d.cause;
}
```

What comes back from a model is text; what a workflow needs is a value it can
branch on. An `agent` declaration is a **function the compiler writes**: a
capability call and a validation.

```console
$ sic run examples/agent.sic --checkpoint ask.sicc
waiting: [claude-opus-4] disk usage is at 100%

$ sic resume ask.sicc examples/agent.sic \
    --value '{"cause": "disk full", "confidence": 0.9}'
"disk full"
```

Nothing in the VM knows what an agent is - it sees `CALL_CAP` and `FROM_JSON` -
and nothing reaches a model without a grant naming it. An answer that does not
fit fails **at the boundary**, with the path that failed:

```text
error: the document does not fit the type: evidence[0].weight: expected Int, found a string
```

`sic-json` is a parser written by hand, accepting RFC 8259 and nothing more, with
caps on document size and nesting because its input is untrusted text from a
model. See [docs/design/agents.md](docs/design/agents.md).

### Telemetry is a view of the journal

```console
$ sic run app.sic --journal run.jsonl
$ sic export run.jsonl --traces traces.json --metrics metrics.json
```

The journal is the canonical record; OpenTelemetry is an external interface. The
arrow points one way, and none of the OTel vocabulary reaches back into the
event model.

The exporter **converts and does not send**. Sending telemetry is an external
effect, and an external effect is a capability - a VM that could quietly post
spans somewhere would be the exfiltration path the journal was careful not to
build. It is also a pure function of the journal, so it can run long after the
run finished, on a machine that never saw it.

Spans come from events that already carry a span and a parent, so a trace is
pairing starts with ends rather than reconstructing a tree. A span that never
closed - a run that was killed - is exported with an error status rather than
dropped, since those are the runs worth looking at. **Digests stay digests**:
converting to another format is not a reason to start including values. See
[docs/design/observability.md](docs/design/observability.md).

Every phase is verified: `sic run` compiles, verifies, and only then executes.
The VM never runs bytecode that has not passed the verifier, including bytecode
this process just produced.

Other commands: `sic parse` (AST), `sic hir` (high-level IR).

See [docs/design/v0.1.md](docs/design/v0.1.md) for the design.

## Building

```console
$ cargo test
$ cargo run -p sic-cli -- parse examples/milestone.sic
```

Linking needs a C linker (`cc`). If you see `linker 'cc' not found`, install one
(`sudo apt install build-essential` on Debian/Ubuntu). If you cannot, link with
the `rust-lld` that rustup already ships, against the musl target:

```console
$ rustup target add x86_64-unknown-linux-musl
$ LLD="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
$ RUSTFLAGS="-Clinker=$LLD -Clinker-flavor=ld.lld" \
    cargo test --target x86_64-unknown-linux-musl
```

## Layout

| crate | role |
|-------|------|
| `sic-core` | `Span`, `SourceFile`, `Diagnostic`, shared ID newtypes |
| `sic-syntax` | lexer, AST, parser (recursive descent; Pratt for expressions) |
| `sic-types` | interned types, type checker, name resolution |
| `sic-ir` | high-level IR, where workflow semantics still exist |
| `sic-bytecode` | instruction set, `.sicb` format, disassembler |
| `sic-compile` | HIR to bytecode |
| `sic-verify` | the bytecode verifier |
| `sic-vm` | the register VM |
| `sic-journal` | the execution journal: events, digests, JSONL |
| `sic-json` | a JSON parser, for what a model answers with |
| `sic-plan` | what a program may do, read from its bytecode |
| `sic-otel` | turns a journal into OTLP traces and metrics |
| `sic-broker` | performs capability calls; the only crate with external effects |
| `sic-cli` | the `sic` command |

`sic-vm` performs no external effects and does not depend on `sic-broker`; that boundary is where the VM and the capability
broker will later split into separate processes, and it is checked by a test
rather than left as an intention.

## Adding a dependency

`[dependencies]` stays empty. To propose a crate, document the following in
`docs/design/` first:

1. why it is needed
2. why `std` alone is insufficient
3. how much the dependency tree grows
4. the cost of implementing it by hand
5. the security impact
