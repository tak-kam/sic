# What is built, and what is not

The specification this project follows has 34 sections. This says where each one
stands, so that picking up the work does not start with reading everything.

Last updated at 809 tests.

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
| 6, 7 | A register VM, 37 instructions | `sic-vm`, `sic-bytecode` |
| 8 | Capability-based security | `docs/design/capabilities.md` |
| 9 | VM and broker separated | a test fails if `sic-vm` depends on `sic-broker` |
| 10 | Absolute paths, no PATH search, binary hash pinning, argument vectors pinned by prefix, output read back - and `process.run`, which reads it whether or not the program worked | `sic-broker`, `docs/design/arguments.md`, `docs/design/output.md` |
| 11 | `import`, local files only | `docs/design/modules.md` |
| 12 | Bytecode verifier | `sic-verify`, `docs/design/v0.1.md` §9 |
| 13 | Bytecode format with a capability manifest | `.sicb`, `sic verify` reports it |
| 14 | An arena per run, no GC | `sic-vm/src/value.rs` |
| 15 | Suspend, save, resume | `docs/design/durable-execution.md` |
| 16 | Cooperative scheduling, `spawn` and `await` | `docs/design/concurrency.md` |
| 18 | Structured output: parse, validate, typed value - and `type Line { reason: String, .. }`, a type that says it describes part of a document, because one validator serves a model's answer and a machine protocol and those disagree about a field nobody declared; a field may also say it is sometimes not there, `executable: String?`, which is the other half of the same disagreement - `a.executable?` asks and `a.executable` fails the run rather than inventing a value | `docs/design/agents.md` |
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
| - | The scaffold around a model call - retry, budget, validation, tool list, the person who approves, the record - named as one thing, and argued as a declaration a plan can print rather than an object graph a program assembles at run time. A real one is written and run, and what could not be written is reported - and then written: a retry could not be about a validation (#83 gave the declaration a `retry`), a `Float` had no operators (#85 gave it four), and `budget` bounded a call site rather than an agent (#84 settled it the other way) | `workflows/harness.sic`, `docs/design/harness.md` |
| - | `retry: N` on an agent: the answer that does not fit `output` is asked again, up to N times, with what was wrong with the last one appended to the prompt by the runtime - which the program could not write, because a rejected `LLM<String>` and a reason from the type section are two provenances. Every attempt is charged to the budget, so a bound that counted only successes is not what a person approved | `docs/design/agents.md` §6a |
| 26 | `log <level> <expr>;` - the journal keeps the level and the digest, the run's values file keeps the text, and stderr shows it as it happens | `docs/design/logging.md` |
| - | `--interactive`: a run that stops for an answer asks the terminal instead of leaving it for whoever comes along later, and keeps asking for as long as it keeps stopping - the checkpoint is written first either way, so the worst case of an interactive run is a non-interactive one | `docs/design/interactive.md` |
| - | `for x in xs { ... }`: the only loop, over a list and nothing else - no assignment, so no induction variable and no way to write one that does not end, and no frame per element, which is what a list longer than the 1024-frame call stack needed. It lowers to a counter, `GET_INDEX` and the backward `JUMP` the bytecode already encoded, so no instruction was added and the verifier's fixed point already handled the edge | `docs/design/v0.1.md` §2 |
| 31 | Phases 1 to 8 | one commit each |
| 33 | The security principles | each one has a test |
| - | A person approving a value is shown it, serialised by an instruction no syntax can reach - and shown only the fields the type declares, so `sic plan` says `(declared fields only)` where a type is open | `docs/design/trust.md` §3, `docs/design/agents.md` §8 |
| - | Both spellings of asking a model carry the label: `llm.invoke` is typed `LLM<String>` in the capability table rather than at the one call shape that declares an answer, and `from_json` carries a document's label onto the record it reads | `docs/design/trust.md` §2 |
| - | What a trusted value may **decide**, as against what it may reach: a branch is not an effect, because the manifest is the unit of approval - and `len` takes the label off, which is a channel from a model to a branch and is accepted with reasons rather than by accident | `docs/design/trust.md` §2a |
| - | E0371 refuses an operator that hands back a value of its operands' own kind, and nothing else. A comparison answers a `Bool` *about* a labelled value and is allowed, which is the same criterion `+` on two strings answers the other way by carrying the label; arithmetic still cannot launder one, and `x == true` is refused because it is the `Bool` again rather than a question about it. The rule stopped being about which syntax a program used to ask, which is what a builtin taking the argument an operator refused had made it | `docs/design/trust.md` §2a |
| - | What a grant on each capability may say - `in`, `env`, `delegable`, and how an agent reaches it - is a table with a test that it is complete, so a capability added without those four decisions fails rather than being found by reading the output | `crates/sic-cli/tests/cli.rs` |
| - | `==` and `!=` on `String`: byte equality of the interned string, so `"main" == "Main"` is false. Every layer below the checker was already generic - `EQ` is three registers, the verifier asks only that both operands have the same type, and the VM's `values_equal` had the arm - which made one row of the checker's operator table the whole of the refusal. Ordering stayed out, because `<` needs a collation decision nobody has asked for | `docs/design/v0.1.md` §4 |
| - | `contains(haystack, needle)` and `starts_with(string, prefix)`: the two questions a program may ask about a string it holds, answering `Bool` and allocating nothing. Two rather than one because a grant is about a prefix, and a match in the middle answers a different question with the same word. A labelled string may be asked either, and the answer is plain - the reason `len` gives, and the width that adds to it, is argued rather than assumed | `docs/design/trust.md` §2a, `docs/design/v0.1.md` §6 |
| - | `"a" + "b"`: the first thing a program can do that makes a value bigger than the ones it was given, and so the first that allocates without a capability being called. `CONCAT` is charged a fuel per byte of its result before the string is built, which makes the instruction budget a bound on the arena - at most `fuel` bytes joined in a whole run - and leaves `sic plan` saying exactly what it said before. A label is contagious across it, on either side, because `"" + tainted` is laundering with an extra character; two different labels are refused, because a value comes from one place | `docs/design/v0.1.md` §6, `docs/design/trust.md` §2a |
| - | `approve` shows the person the value. It renders it with `TO_JSON` - the inverse of `FROM_JSON`, an instruction no syntax can name, so the language still has no way to get a plain `String` out of a labelled value - and passes it to `human.approve` beside the question. The whole document crosses rather than a digest or a first screenful, and the bound is the run's own budget, because rendering is charged by the byte the way `CONCAT` is. `sic explain` needed no change: the question a person was asked is already recorded beside their answer, and the value is in the question | `docs/design/trust.md` §3 |
| - | And every field of a grant survives the journey from bytecode to a plan, checked with a value per field that could not have come from any other - the transcriptions between the three structs a grant is declared in are hand-written, and several of its fields are `String`, so the copy that takes the wrong one compiles | `crates/sic-cli/tests/cli.rs` |
| - | `answers json` and `answers jsonl` on a grant: what form a program's output takes, checked by the broker on the bytes and printed by `sic plan` - so that a workflow reading another program's answer says which kind of dependency it has taken. Output that does not parse fails the call naming the line, and carries what the program said on stderr, because a false manifest is not data. There is no typed rung: measuring cargo's own stream shows the lines are a sum of shapes agreeing on one field, which would need sum types, optional fields and open records - and the check would have to cross the broker boundary into a crate with no type system. A grant that says nothing is printed as `(no declared shape)` rather than left looking checked | `docs/design/answers.md` |

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
`HumanChosen<T>` exist and are enforced, and each of them says what it means:
`HumanApproved<T>` is a person who was shown this value and said yes, which it
was not until the value crossed to them. `Secret<T>`, `Verified<T>` and `UserProvided<T>` do not, because
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
- No `while`, no `break`, no `continue`, no ranges and no iterators. A `for` over a
  list needs none of them, and a `while` needs something to change between two visits
  to its condition, which nothing in this language does (issue #66)
- No assignment, so a loop body performs effects rather than accumulating a value: a
  fold is still a recursion, and still costs a frame per element - measured, it stops
  at about 1020, and the shape somebody writes instead is refused rather than run,
  because a `let` that hides a binding its own initializer reads is E0313 (issue #81,
  `docs/design/v0.1.md` §2). `docs/design/loops.md` is the design that would change both, and
  it argues for `mut` and `=` rather than a loop-bound accumulator: the narrow form
  is assignment with a restriction bolted on, and it cannot write an agent loop
- No package registry, no dynamic loading (§11, §33)
- No pruning or retention for recorded runs (`docs/design/runs.md`)
- No typed shape on a capability grant. A grant may come to say `answers json` or
  `answers jsonl`, which the broker can check because parsing needs no type system;
  `answers jsonl of T` would need sum types on top of the optional fields and open
  records that have since landed, and cargo's own JSONL - the case that motivated
  it - needs all three (`docs/design/answers.md`)
- No second discharge for a trust label. A person is the only thing that turns a
  model's answer into one a capability may write or run, and a discharge whose
  argument is evidence waits on a capability that can look at a labelled value
  and answer a fact about it - which no capability in the table does
  (`docs/design/checking.md`)
- No sum types. A value that is one of several shapes is designed rather than
  built: the discriminating field a protocol already carries is the runtime tag,
  so `l.reason` is a field read at position 0 and only the extraction, `l as
  Finished`, needs an instruction - no change to the arena, the checkpoint or
  the verifier's lattice. It waits on open records, because measuring cargo's
  own stream one level down shows that two of its three arms cannot be written
  as closed records at all, and because optional fields did not dissolve into it:
  two messages with the same discriminant are not two arms, so they were built
  on their own argument (`docs/design/alternatives.md`, `docs/design/agents.md` §8)
