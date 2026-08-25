# What is built, and what is not

The specification this project follows has 34 sections. This says where each one
stands, so that picking up the work does not start with reading everything.

Last updated at 570 tests.

That number is checked (`crates/sic-core/tests/workspace.rs`), which is the
point of it: a commit that adds a test has to come here to update the line, and
what is under the line is the thing worth re-reading. It counts test functions
in the source, so it is the same on every platform - four of them are
`#[cfg(target_os = "linux")]` and a run elsewhere reports four fewer.

---

## Built

| § | | Where |
|---|---|---|
| 2 | Rust, and nothing else | `[dependencies]` is empty in every crate |
| 3 | No external crates | lexer, parser, types, IR, bytecode, verifier, VM, JSON, SHA-256, scheduler, journal all written by hand |
| 4 | Recursive descent, Pratt for expressions | `sic-syntax` |
| 5 | Source → AST → typed → IR → bytecode → verifier → VM | the whole pipeline runs |
| 6, 7 | A register VM, 30 instructions | `sic-vm`, `sic-bytecode` |
| 8 | Capability-based security | `docs/design/capabilities.md` |
| 9 | VM and broker separated | a test fails if `sic-vm` depends on `sic-broker` |
| 10 | Absolute paths, no PATH search, binary hash pinning, argument vectors pinned by prefix, output read back - and `process.run`, which reads it whether or not the program worked | `sic-broker`, `docs/design/arguments.md`, `docs/design/output.md` |
| 11 | `import`, local files only | `docs/design/modules.md` |
| 12 | Bytecode verifier | `sic-verify`, `docs/design/v0.1.md` §9 |
| 13 | Bytecode format with a capability manifest | `.sicb`, `sic verify` reports it |
| 14 | An arena per run, no GC | `sic-vm/src/value.rs` |
| 15 | Suspend, save, resume | `docs/design/durable-execution.md` |
| 16 | Cooperative scheduling, `spawn` and `await` | `docs/design/concurrency.md` |
| 18 | Structured output: parse, validate, typed value | `docs/design/agents.md` |
| 20, 21 | The journal is the runtime's own account | `docs/design/v0.1.md` §10 |
| 22, 23, 24, 25 | OTLP traces and metrics, `sic.` and GenAI attributes | `docs/design/observability.md` |
| 27 | Secrets do not reach telemetry | the journal records digests, never values |
| 28 | A debug section maps a pc to a line | every runtime failure names one |
| 29 | The CLI | `run`, `resume`, `plan`, `runs`, `attach`, `explain`, `inspect-run`, `replay`, `recheck`, `export`, `upgrade`, `compile`, `verify`, `disasm`, `parse`, `hir` |
| 30 | `sic plan` | `docs/design/plan.md` |
| - | `sic upgrade`: fetch a release, check it against the digests it publishes, swap it in | `docs/design/upgrade.md` |
| - | `--llm tmux:claude`: a model call answered by an agent CLI in a pane, instead of deferring; an `agent` tells it the shape its answer must take, and `memory: task` keeps one conversation for as long as a task | `docs/design/driving.md` |
| - | The agent's authority is the program's manifest, and for the `process` family deliberately less: translated into its own permissions where those can hold a constraint, routed back through the broker where they cannot but only when the grant says `delegable`, and a hook that fails closed refuses every tool the manifest does not account for, which is what denies the agent the network, and puts every tool use in the journal; `budget`, `tools` and `deadline` bound it, each where it can be enforced; `sic plan` prints all of it, naming where each line is enforced | `docs/design/authority.md` |
| - | This repository's own development loop, written in sic: it plans, runs, checkpoints at the model call, and reads back with `sic explain`. Seven things bent it on the way, each now an issue | `workflows/ci.sic`, `docs/design/self-hosting.md` |
| 31 | Phases 1 to 8 | one commit each |
| 33 | The security principles | each one has a test |

---

## Partly built

**§17, agents.** An agent is a model call, a validation and a budget, and with
`memory: task` a conversation that outlives the call
(`docs/design/driving.md`). Memory is implemented by not implementing it: the
conversation holds it, the pane holds the conversation, and sic stores nothing -
which is also its cost, and why the choice is written in the declaration.

Tools are what is left, and they are where the rest of the section starts to
cost something: an agent with tools runs its own loop inside one call, so the
driver counts 1 where the machine did 200, and the grant that let the program
ask says nothing about what the agent did while answering. `sic plan` prints
that as a warning rather than leaving it out, and `docs/design/authority.md` is
the design that makes the warning untrue.

**§19, trust.** `LLM<T>`, `HumanApproved<T>`, `Observed<T>` and
`HumanChosen<T>` exist and are enforced. `Secret<T>`, `Verified<T>` and `UserProvided<T>` do not, because
nothing produces one yet - see `docs/design/trust.md`.

**§22, OpenTelemetry.** The journal converts to OTLP documents. Nothing sends
them: sending is an external effect, and an external effect is a capability.

**§25, metrics.** Counts are there. Durations and token costs are not: the
broker does not report either, so any number would be invented.

---

## Not built

**§26, structured logging.** `log info "..." { ... }` does not parse. The IR has
the instruction, the journal has nowhere for it to go yet, and the exporter says
so.

**§9, as separate processes.** The VM and the broker are separate crates with no
dependency between them, and the values that cross between them are already
serializable. They still run in one process.

---

## Deliberately not built

Each is recorded where the decision was made, with the reason:

- No optimization passes, no register reuse (`docs/design/v0.1.md`)
- No `parallel { }` block - it would be sugar over `spawn` and `await`
- No cancellation, no backoff, no preemption (`docs/design/concurrency.md`)
- No option or nullable types, so every field of a record is required
- No iteration: v0.1 has no loops, and recursion is how a program repeats
- No package registry, no dynamic loading (§11, §33)
- No pruning or retention for recorded runs (`docs/design/runs.md`)
