# What is built, and what is not

The specification this project follows has 34 sections. This says where each one
stands, so that picking up the work does not start with reading everything.

Last updated at 633 tests.

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
| 6, 7 | A register VM, 31 instructions | `sic-vm`, `sic-bytecode` |
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
| 30 | `sic plan`, and `--graph`: the same plan as a Mermaid flowchart, which says which functions reach which - the one thing a list of blocks side by side cannot, and the caption says "may, not will" so an arrow does not claim more than the sentence it replaces | `docs/design/plan.md` |
| - | `sic upgrade`: fetch a release, check it against the digests it publishes, swap it in | `docs/design/upgrade.md` |
| - | `--llm tmux:claude`: a model call answered by an agent CLI in a pane, instead of deferring; an `agent` tells it the shape its answer must take, and `memory: task` keeps one conversation for as long as a task | `docs/design/driving.md` |
| - | The agent's authority is the program's manifest, and for the `process` family deliberately less: translated into its own permissions where those can hold a constraint, routed back through the broker where they cannot but only when the grant says `delegable`, and a hook that fails closed refuses every tool the manifest does not account for, which is what denies the agent the network, and puts every tool use in the journal; `budget`, `tools` and `deadline` bound it, each where it can be enforced; `sic plan` prints all of it, naming where each line is enforced | `docs/design/authority.md` |
| - | `git.status` and `git.rev_parse`: a repository read through the broker, with hooks, pagers, aliases, credential helpers and every config file it did not put there turned off - which is what a `process.run "/usr/bin/git"` grant cannot say and is the whole test a capability has to pass to exist | `docs/design/git.md` |
| - | This repository's own development loop, written in sic: it plans, runs, checkpoints at the model call, and reads back with `sic explain`. Seven things bent it on the way, each now an issue | `workflows/ci.sic`, `docs/design/self-hosting.md` |
| 26 | `log <level> <expr>;` - the journal keeps the level and the digest, the run's values file keeps the text, and stderr shows it as it happens | `docs/design/logging.md` |
| - | `--interactive`: a run that stops for an answer asks the terminal instead of leaving it for whoever comes along later, and keeps asking for as long as it keeps stopping - the checkpoint is written first either way, so the worst case of an interactive run is a non-interactive one | `docs/design/interactive.md` |
| 31 | Phases 1 to 8 | one commit each |
| 33 | The security principles | each one has a test |

**§9, as separate processes.** On unix `sic run` starts a child, `sic vm`, and
that child is the interpreter: it opens no file and starts no program, and every
effect crosses back to the parent's broker. Under a memory limit a run that
would have aborted the whole thing aborts the child, and the parent still has
the journal and says what happened. A run that stops to wait is saved and picked
up again - the child produces the checkpoint and the parent writes it, and the
bytes are the ones one process would have written. A child that dies is told
apart from one that failed and from one that stopped quietly, and a child left
behind by a parent that died notices; there is no timeout, because a sic program
cannot run forever and the only thing left to bound would be sic's own bugs.
`resume` and `attach` split the same way, a checkpoint does not remember which
shape wrote it, and `--no-isolate` is how a run says one process instead.

Windows has no unix socket and runs one process. That is stated rather than
arranged around: this is defence in depth and a resource bound, not the
capability boundary. The boundary is the crate graph, it holds everywhere, and
`crates/sic-vm/tests/isolation.rs` is what checks it.

`docs/design/processes.md` is the design, and it starts by measuring what the
split buys rather than assuming: "the VM cannot reach the outside world" was
already true. What the split adds is the resource bound - a run that grew its
arena to 230 MB took the process that was also holding the run store and the
terminal - and the possibility of giving the side that runs the bytecode fewer
privileges than the side that performs effects, which is the one thing a crate
boundary cannot do.

---

## Partly built

**§17, agents.** An agent is a model call, a validation and a budget, and with
`memory: task` a conversation that outlives the call
(`docs/design/driving.md`). Memory is implemented by not implementing it: the
conversation holds it, the pane holds the conversation, and sic stores nothing -
which is also its cost, and why the choice is written in the declaration.

Tools are what is left, and they were where the rest of the section started to
cost something: an agent with tools runs its own loop inside one call, so the
driver counted 1 where the machine did 200, and the grant that let the program
ask said nothing about what the agent did while answering. `sic plan` printed
that as a warning.

`docs/design/authority.md` removed the warning by making it untrue. The agent's
authority is the manifest - translated, routed or withheld - the hook puts every
tool use in the journal, and `tools` bounds the count that `budget` cannot. What
is actually left of §17 is smaller than it was: the agent's own loop is still
one capability call in the journal, so a run's *shape* is the program's calls
rather than the agent's steps, and that is a deliberate line rather than a gap -
see `authority.md` §7.

**§19, trust.** `LLM<T>`, `HumanApproved<T>`, `Observed<T>` and
`HumanChosen<T>` exist and are enforced. `Secret<T>`, `Verified<T>` and `UserProvided<T>` do not, because
nothing produces one yet - see `docs/design/trust.md`.

**§22, OpenTelemetry.** The journal converts to OTLP documents. Nothing sends
them: sending is an external effect, and an external effect is a capability.

**§25, metrics.** Counts and durations are there - two histograms, because a
call answered within the call and one answered after a suspension are times four
orders of magnitude apart and each is useless as an answer to the other's
question. Token costs are not: nothing reports them, so a number would be
invented.

---

## Not built

Nothing from the specification is outstanding. What is left is in
`Deliberately not built`, or in an issue.

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
