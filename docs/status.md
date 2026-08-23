# What is built, and what is not

The specification this project follows has 34 sections. This says where each one
stands, so that picking up the work does not start with reading everything.

Last updated at 353 tests.

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
| 10 | Absolute paths, no PATH search, binary hash pinning | `sic-broker` |
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
| 29 | The CLI | `run`, `resume`, `plan`, `runs`, `explain`, `inspect-run`, `replay`, `export`, `compile`, `verify`, `disasm`, `parse`, `hir` |
| 30 | `sic plan` | `docs/design/plan.md` |
| 31 | Phases 1 to 8 | one commit each |
| 33 | The security principles | each one has a test |

---

## Partly built

**§17, agents.** An agent is a model call and a validation, with a budget. Tools,
memory and execution history are not there: an agent that can call tools is an
agent that can loop, and a loop whose stopping condition is a model's output
needs a budget that counts more than calls.

**§19, trust.** `LLM<T>` and `HumanApproved<T>` exist and are enforced.
`Secret<T>`, `Verified<T>`, `Observed<T>` and `UserProvided<T>` do not, because
nothing produces one yet - see `docs/design/trust.md`.

**§22, OpenTelemetry.** The journal converts to OTLP documents. Nothing sends
them: sending is an external effect, and an external effect is a capability.

**§25, metrics.** Counts are there. Durations and token costs are not: the
broker does not report either, so any number would be invented.

---

## Not built

**§11, imports.** There is no `import` at all, so a program is one file. The
specification's rule - local files only, no network resolution - is not violated
by this, but it is not implemented either. This is the largest gap for anyone
writing a program longer than an example.

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
