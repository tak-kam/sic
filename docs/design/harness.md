# The harness is a declaration

The field has a word for the scaffold around a model call. A harness is the
retry, the budget, the validation of what came back, the tool list, the person
who approves the dangerous step, and the record of what happened - everything
that is not the model. Everywhere else it is an object graph: a configuration
built by the program at run time, out of classes, decorators and callbacks, and
readable only by running it.

sic has one already, and describes it in six documents as six unrelated
features. This document says it is one thing, and makes the two claims that
separate it from a library:

- **the harness is knowable before it runs**, because it is a declaration in
  the bytecode that `sic plan` prints and a person approves; and
- **a harness can be tested**, because `--record`, `sic replay` and
  `sic recheck` turn one into a regression test made of runs it has been
  through.

Both survive being written down. The second one has a bug sitting on it that
this exercise found (§5.6), which is a different thing from the claim being
wrong, and saying which is the point of writing the program rather than the
prose.

The name is the field's rather than this project's. `agents.md` is about a
model call and a validation, `authority.md` about what the thing answering may
do, `driving.md` about who answers; none of them is about the shape all of that
makes, and a reader arriving with the word "harness" in their head has nowhere
to go. This is that document.

---

## 1. What is already here, as one idea

Nothing below is new. What is new is the column on the right.

| what a harness has | in sic | where it is enforced |
|---|---|---|
| a model call | `llm.invoke`, a capability like any other | the broker, or whoever answers a deferred call |
| the answer validated | `agent`'s `output`, lowered to `FROM_JSON` | the VM, against the type section |
| a bound on how much it may ask | `budget: N` | the VM, against a pc in the policy table |
| a bound on what it may do while answering | `tools: N` | the broker, through the agent's `PreToolUse` hook |
| a bound on how long | `deadline: N`, `timeout N` | the broker, which has the clock |
| a retry | `retry N` on a capability call | the VM, which re-issues and journals every attempt |
| what the thing answering may reach | the `allow` block, translated, routed or withheld | the agent's own permissions, the broker, the hook |
| memory between calls | `memory: task` | a tmux pane the broker owns |
| a person in the loop | `approve`, `choose`, and `HumanApproved<T>` in a signature | the type checker, before anything runs |
| what happened | the journal, `sic explain` | the runtime, with no instrumentation in the program |
| a way to run it again | `--record`, `sic replay`, `sic recheck` | nothing calls out; every answer comes from the file |

Every row of that table was built for its own reason and argued in its own
document. That is why it works, and it is also why nobody could see it: eight
good local decisions do not announce the shape they add up to.

The shape is this. **In sic the harness is the program, and the program is a
declaration.** There is no separate object holding the retry policy, no
registry of tools, no builder. `budget` is a field of a declaration the compiler
turns into a number attached to an instruction; the tool list is the `allow`
block; the retry is a word after a call; the person is a type in a function
signature. All of it is in the `.sicb` before anything runs, and `sic plan`
reads it.

---

## 2. Why declarative-and-checkable is a position rather than a detail

The usual answer to "what will this agent do" is to read the code that builds
the harness. That is not a worse version of reading a plan; it is a different
kind of act, and the difference is the whole argument.

**A run-time graph cannot be read before it runs, by construction.** The object
a reader would need does not exist until the process that builds it is running,
and the thing that builds it is the program whose behaviour was in question. To
see the harness you have to start the program, and starting the program is the
decision you were trying to make. Every mitigation for that is a promise about
a code path: a dry-run mode, a `--plan` flag, a linter. Each of them is more
code in the same program, and each can be wrong in the direction that matters,
because the thing that runs is not the thing that printed.

`sic plan` is not a mode of the program. It is a different binary path that
opens the `.sicb`, reads four sections that are already in it, and prints them.
It constructs no VM, opens no socket and starts no process (`plan.md` §1). That
is what makes it safe to run against a program you have not decided to trust
yet, which is the only moment a plan is worth anything.

What follows from that is not "it is nicer to read". It is that **the plan and
the run cannot disagree.** A budget the plan prints is the number the VM
charges against, because it is the same byte in the same file; a grant the plan
lists is the entry the broker checks. There is no second copy to drift.

The cost is real and is the honest half of this section: **a declaration is
fixed at compile time.** A harness that decides at run time which of five
agents to ask cannot be one call site with a variable in it. It is five call
sites and five lines in the plan - more verbose, and more true. A harness that
computes its own budget from a configuration file cannot be written at all.
Both of those are things a Python harness does in a line, and this language
says no to them on purpose: a bound a program computed is a bound nobody
approved.

---

## 3. And a harness can be tested

The second claim is the one nothing in the field does well, and it is worth
stating as a mechanism rather than as a virtue.

A recorded run keeps three files: the bytecode that ran, the journal of what
happened, and the answers the broker gave (`runs.md` §1). `sic replay` re-runs
the stored bytecode against the stored answers and compares journals, which is
a claim about determinism. `sic recheck` compiles a *different* source file and
runs that against the same answers, which is a claim about the program: every
recorded answer is still being given to the same question.

For a harness that second claim is worth more than it is for an ordinary
program, because the thing most likely to change in a harness is exactly what
`recheck` compares. A threshold, a retry count, an order of questions - all of
them move which call comes next, and none of them is visible in a diff as a
behaviour change. Lowering one number in the harness of §4 from 70 to 30:

```console
$ sic recheck dcfe0395 lax.sic
rechecking dcfe0395e79606005edd5e283a2cfffc (main) against lax.sic
  2 of 5 calls matched
  call 3: the recording answered `llm.invoke`, this program asks `human.approve`
```

The edit made the harness stop retrying, and the recording said so, from an
answer given days earlier, without calling a model. Raising the same number to
95 is caught from the other side: `call 4: the recording answered
human.approve, this program asks llm.invoke`. Nothing had to be written to get
either. Somebody answered a run once, and it became a test of the retry policy.

That is the shape of the claim: **a harness whose behaviour is a regression
test is a different kind of object from one that is a Python object graph.** It
is also, today, a claim with a bug on it, and §5.6 is that bug.

---

## 4. The exercise: a harness written and run

`workflows/harness.sic` is the program. It is deliberately the shape real agent
scaffolding has rather than a demonstration of a feature: a build that fails and
prints why, a model asked to diagnose it, an answer that has to fit a declared
type, a retry when the answer is not good enough, a budget over the whole thing,
a person at the point that changes something, and a record.

It is a template, as `workflows/ci.sic` is and for the same reason - a manifest
names files on one machine. The transcripts below are of a local copy with real
paths in it.

The plan is the harness:

```console
$ sic plan workflows/harness.sic
Execution plan for workflows/harness.sic
bytecode sha256:...

  build
    1. RUN      process.run     "/PATH/TO/build"   ; 48:12

  propose_until_confident
    1. INVOKE   llm.invoke      "claude-opus-4"  in one conversation per task  at most 3 in a run, shared by 2 sites  at most 8 tool use(s)  120000ms per answer   ; 61:13
    2. VERIFY   Fix   ; 61:13

  retry_proposal
    1. INVOKE   llm.invoke      "claude-opus-4"  in one conversation per task  at most 3 in a run, shared by 2 sites  at most 8 tool use(s)  120000ms per answer   ; 69:13
    2. VERIFY   Fix   ; 69:13

  apply
    1. EXEC     process.exec    "/PATH/TO/apply"   ; 85:12

  main
    1. APPROVE  human.approve   "applying the fix"   ; 97:20

Capabilities:
  process.run     [exec]  "/PATH/TO/build"  (not pinned)  (no declared shape)  in "/PATH/TO/project"  with no environment
  llm.invoke      [invoke]  "claude-opus-4"  (not pinned)
    the agent may not  use "/PATH/TO/build"     (the grant does not say `delegable`)
    the agent may use  "applying the fix"       (through the broker)
    the agent may not  use "/PATH/TO/apply"     (the grant does not say `delegable`)
    the agent may not  reach the network        (no tool it has can)
    the agent may not  run a shell of its own   (refused by the hook)
    the agent may not  use any other tool       (refused by the hook)
  human.approve   [invoke]  "applying the fix"  (not pinned)
  process.exec    [exec]  "/PATH/TO/apply"  (not pinned)  in the directory `sic` is started in  with no environment

Budgets:
  at most 3 llm.invoke calls in a run, from 2 sites: propose_until_confident 61:13, retry_proposal 69:13

At most 3 call(s) from budgeted sites, plus 3 site(s) with no budget.
```

Everything a harness is, before it runs: which model, how many times, how long
per answer, how many tools, whether it remembers, what it may reach, what the
answer must fit, and which effect is behind a person. `--graph` draws the retry
as what it is - `retry_proposal --> retry_proposal`, a cycle printed before the
program runs.

The run is four commands, because a model call with no driver defers and a
person answers it:

```text
$ sic run harness.local.sic --record
warn: the build failed, asking for a fix
waiting: [claude-opus-4] error[E0308]: mismatched types ...

$ sic attach dcfe0395 --value '{"file":"src/lib.rs","change":"add a cast","confidence":40}'
waiting: [claude-opus-4] that answer was not confident enough. Try again.

$ sic attach dcfe0395 --value '{"file":"src/lib.rs","change":"cast the index to usize","confidence":90}'
info: proposed: cast the index to usize
waiting: [applying the fix] apply this fix?
  approving: {"file":"src/lib.rs","change":"cast the index to usize","confidence":90}

$ sic attach dcfe0395 --value true --because "the cast is the right fix"
applied
```

The first answer is rejected by the program's own gate and the harness asks
again; the second passes; the person is shown the value they are approving and
their reason is recorded beside it. Read back afterwards:

```text
    task main
          call process.run
            process.run answered sha256:f33282aa
        warn: the build failed, asking for a fix
          call llm.invoke  (budget: 2 left)
            llm.invoke answered sha256:ed76765e
            call llm.invoke  (budget: 2 left)
              llm.invoke answered sha256:4571bfe2
        info: proposed: cast the index to usize
        call human.approve
          human.approve answered sha256:7cc15f3c
          call process.exec
            process.exec answered sha256:35966776
```

That is the harness's whole run, produced by a runtime rather than by
instrumentation in the program, with the budget charge printed against the call
that spent it and the retry visible as the deeper frame. Both `budget: 2 left`
lines are §5.3.

---

## 5. What could not be written

Every gap below was found by writing that program and running it. They are
ordered by how much they cost a harness, and the first one is the whole
argument for doing the exercise.

### 5.1 Retry and validation cannot be put in the same place

The single most characteristic thing a harness does is ask again because the
answer did not fit. sic has a retry, and it has a validation, and there is no
way to make the retry be about the validation.

| | validates the answer | may be retried |
|---|---|---|
| `agent propose { output: Fix }` | yes, `FROM_JSON` against the type | no - E0330 |
| `llm.invoke("...")` | no; there is no `from_json` a program can write | yes, `retry 3`, if the grant says `repeatable` |

```text
error[E0330]: `retry` and `timeout` apply to capability calls only
  = note: an agent is bounded in its declaration: `budget` for model calls,
          `tools` for tool uses, `deadline` for wall clock
```

E0330 is right that an agent call is a function call. Its note is right about
bounds and answers a different question: `budget`, `tools` and `deadline` are
three ways to say *how much*, and none of them is a way to say *again*.

And `retry` on the raw capability would not close it even if the validation
could be written next to it, because retrying is attached to the `CALL_CAP`
site and the validation is the `FROM_JSON` after it. A `retry 3` would ask
again when the broker failed, which is a transport problem, and not when the
answer was prose, which is the problem harnesses actually have.

Underneath both is the thing that decides it: **a validation failure is not a
value.** It ends the run.

```console
$ sic attach 3f8025cb --value '{"file":"src/lib.rs","change":"add a cast","confidence":"high"}'
error: the document does not fit the type: confidence: expected Int, found a string
 --> harness.local.sic:61:13
```

The message is excellent and the run is over, with a `budget: 3` that was never
spent. Nothing in the language catches it, and nothing should be bolted on for
this alone: exceptions, `Result`, a fallible `from_json` and a `try` block are
four different large decisions and this is one program's evidence for them. But
the position of this document has to be stated honestly. **The harness sic can
declare is one whose model answers in the right shape.** A harness for the
world as it is retries the shape failure, and that is a gap rather than a
choice.

The exercise's own workaround is worth reading as the shape of what is missing:
the harness validates against `Fix`, which always fits, and then gates on a
field of it - `confidence > 70` - which is a *semantic* retry the program can
write because the value exists. Semantic retries are useful and this one is
real. It is not the one the field means.

→ separable, and the argument for it is above. Not filed here.

### 5.2 A `Float` is a value nothing can be done with

The first version of the harness declared `confidence: Float`, because that is
what a model answers with and what every `agent` in this repository already
declares:

```text
error[E0303]: `>` cannot be applied to Float
   = note: v0.1 supports arithmetic and comparison on Int only
```

`==` is refused too. A `Float` can be written as a literal, declared as a
field, parsed out of a model's answer, validated, carried, approved and shown
to a person, and there is no operation in the language that can ask a question
about it. `examples/agent.sic` and `workflows/ci.sic` both declare a
`confidence: Float`, and neither of them ever reads it - which is not a
coincidence, because neither of them could.

The cost is not "no arithmetic". It is that **the most common validation gate
in the field cannot be written against the field it is about.** The harness
here declares `confidence: Int` and asks the model for a percentage, so the
workaround changes the question put to the model in order to work around the
type system. `contains` and `starts_with` were added on exactly this argument -
a workflow could not otherwise ask a question it needed - and a comparison on
`Float` is the same argument with a stronger case, since the value is one a
model produced and a program has to act on.

→ separable: comparison on `Float`, which is the narrow version and probably
the whole of it.

### 5.3 `budget: N` bounded a call site, and `agents.md` said both things

Written in the present tense of the exercise, and settled since - the note at
the end of this section says how, and §4's transcript is already the plan
afterwards.

The harness declares one agent with `budget: 3`. Its plan ended:

```text
At most 6 call(s) from budgeted sites, plus 3 site(s) with no budget.
```

Because the retry is written as two functions - one asks with the logs, one
asks again - the agent has two call sites, and **each got its own allowance of
three.** `sic explain` showed it plainly: two model calls, and both printed
`budget: 2 left`.

The plan's total was honest, which matters; the plan's per-line wording was
`at most 3 in a run`, which is what a reader takes as the bound, and the bound
was three *at that site*. And `agents.md` §6 stated both readings in one
paragraph without noticing:

> `budget` is a count of capability calls the agent may make in a whole run.
> [...] It is enforced by the VM, which keeps a count per call site.

Those are the same sentence only for an agent called from one place. Per-site
is the right answer for a recursion, which is how the first version of this
harness was written and where the count behaved exactly as declared. It is the
wrong answer for a refactor: **splitting one retry function into two doubles a
harness's model-call budget, and nothing in the source, the declaration or the
plan's per-line text says so.**

The declaration is the thing this whole document says a person approves. A
number in it that means something other than what it reads as is worth a
correction whichever way it is settled - the document to match the enforcement,
or the enforcement to match the document.

→ separable, and it is one issue with two candidate answers rather than two
issues. **#84 settled it per agent**: the budget can only be written on the
declaration, so a per-site bound is one the language gives no way to declare,
and a number a person approves must not depend on how many places call the
thing it is written on. The transcript in §4 above is the plan after that -
one allowance of three, both sites named under it, and a total that agrees
with the lines rather than doubling them. `agents.md` §6 carries the argument
and what the other reading cost.

### 5.4 A retry cannot say why, and `memory: task` is the answer sic already had

The natural retry prompt is "your last answer was rejected because X". It does
not compile:

```text
error[E0375]: `+` cannot join Observed<String> with LLM<String>
   = the result would have come from two places, and a value comes from one
```

The logs came from a program, the rejected answer came from a model, and a
prompt built out of both is a value with two origins. `trust.md` is right, the
refusal is the rule working, and the harness still needs to say "again".

What closes it is a feature that was designed for something else. With
`memory: task` the second call is asked in a conversation that already holds
the first answer, so the retry prompt is a literal - `"that answer was not
confident enough. Try again."` - and a literal has no provenance to conflict
with anything. The harness does not have to quote the model back to itself,
because the model still has it.

That is worth recording as a positive rather than only as a gap avoided.
Prompt composition is where every other harness leaks provenance: the failed
answer, the tool output and the system prompt end up concatenated into one
string, after which nothing can say where any of it came from. sic cannot write
that string, and the thing it offers instead keeps the accumulated context
where it can be pointed at - in a pane, named after the run, listed in the
run's directory.

The limit is stated where it belongs: this works because the conversation
remembers. A one-shot agent retried with a literal is asking a stranger the
same question in different words, and the program cannot tell it what went
wrong. That is the honest cost of §5.4 and it is bounded by §5.1 anyway - the
failure a retry would most want to explain is the one that ended the run.

### 5.5 `tools: 0` cannot be written

```text
error[E0208]: `tools` needs a positive number
```

An absent `tools` means no limit at all, which the plan prints as `any number
of tool uses`. So the range a program may declare is one to four billion, plus
unbounded, and the one value missing is the strongest claim a harness can make
about an agent: **this one answers a question and does not act.** That claim is
the whole of what most harness sites want - a classifier, a triage step, a
summariser - and it is the one the declaration cannot carry.

It is a small change with a plan line behind it, which is why it is worth
filing rather than mentioning: `tools: 0` should print as `no tools`, and the
hook already refuses everything the manifest does not account for, so the
enforcement is there and only the declaration is missing.

→ separable, and small.

### 5.6 `sic replay` fails on any run that logs

This is a bug, it is on the second of this document's two claims, and it
reproduces in three lines:

```text
fn main() -> Int {
    log info "hello";
    return 1;
}
```

```console
$ sic replay ddcacb8a
replaying ddcacb8ae318144249047cc6907234a4 (main)
  seq 3: recorded logged, replayed logged
```

Exit 1, on a run that is perfectly deterministic, with a difference report
whose two sides are the same word. The cause is the split `runs.md` §2 makes
and `logging.md` uses: a `Logged` event holds the text in memory and its digest
in `journal.jsonl`, and the text lives in the run's values file. So the
recorded event read back off disk holds a digest where the replayed event holds
the text that digest is of, `EventKind` is compared with `==`, and `describe`
has no arm for `Logged` - so both sides print the event's name and the report
says nothing.

Three things follow. The comparison should compare digest with digest, as it
already does for what a capability answered. `describe` should say something
when it reports a difference, because a report that prints the same text twice
is not a report. And the tests that replay a recorded run should replay one
that logged - none of them does, which is why a bug this size has been sitting
in the command that exists to establish determinism.

It matters here beyond being a bug. `logging.md` exists because a workflow has
things to say about itself, `workflows/ci.sic` says four, and the harness in §4
says three. **A harness that says anything at all cannot currently be
replayed**, so the first half of "a harness can be tested" is unavailable to
exactly the programs it was written for. `recheck` is unaffected - it compares
capability calls by name and argument digest and passes on all three of the
edits in §3 - so the claim's more useful half is intact.

→ separable, and the smallest, most clearly-argued issue this exercise
produced.

### 5.7 What the failing program said, once more from this end

`process.run` answers with the exit code and standard output, and stderr is not
a value (`output.md` §3). The harness's fixture prints to stdout so the program
works; a real toolchain writes its diagnosis to stderr, and then the thing the
harness most wants to show a model is the thing it cannot hold. The way round
is `2>&1`, which needs a shell, which is `self-hosting.md` §1 and §2 - the
cheapest way around a limit being a grant that gives away everything.

Nothing new is being claimed here. §3 of `output.md` argues the interleaving
case and is right about it. What the harness adds is that the case it rules out
is not rare in this shape of program: it is the input to the model call, every
time.

---

## 6. The loop, and the question #80 asked this exercise

Issue #80 says an agent loop is "keep going until done", that nothing in the
language assigns, and that a `for` body therefore carries nothing to the next
element. It asks whether a narrow accumulator - `for x in xs into total` -
would be enough, and names this exercise as what should answer.

**The premise needs a correction first, and it is the useful part of the
answer.** A bounded retry with an early exit can be written today, twice over.
As a recursion, which is what `workflows/harness.sic` does. And as a `for`
loop, because **a `return` inside a `for` body compiles and ends the loop**:

```text
fn attempts() -> LLM<Fix> {
    for a in [1, 2, 3] {
        let f = propose("try again");
        if f.confidence > 70 {
            return f;
        }
    }
    return propose("last try");
}
```

That runs, and answering the first call well enough returns immediately without
spending the rest of the budget. So the loop a harness needs most - try up to N
times, stop when one is good enough - is not missing. What is missing is
narrower than #80 states, and it is worth stating precisely: a loop cannot
carry a value to the *next iteration*. It can carry one to the *caller*, which
for a retry is where the value was going anyway.

So: **would `into` have been enough?** For this harness, it was not needed. For
the harness shape that does want it - best-of-N, where three answers are asked
for and the best is kept - `into` is the right shape and it is **not enough on
its own**, for a reason that lands squarely on #80's third decision.

The accumulator has to hold something before the first model call, and there is
nothing to hold. A `Fix` the program invented is not an `LLM<Fix>`, and the
type system says so in both directions:

```text
error[E0301]: expected LLM<Fix>, found Fix
error[E0301]: expected Observed<String>, found String
```

A label does not widen. So `for f in attempts into best` needs `best` to be an
`LLM<Fix>` from the start, and the only honest values of that type come from a
model that has not been asked yet. Every way out is a decision nobody has
taken: an accumulator whose label changes between iterations, which is #80
decision 3's "third thing nobody has thought about", now with a program behind
it; an optional accumulator, which is `Option<T>` and is refused
(`agents.md` §8); or seeding with a real first call before the loop, which is
the recursion again wearing a loop's syntax.

The other half of the answer is smaller and still worth having. A bounded loop
has to write its bound as a list literal - `[1, 2, 3]` - because there are no
ranges, so the retry count appears twice in a harness that also has a `budget`,
in two places that can disagree. That is not an argument for ranges; it is an
argument for noticing that a harness's two bounds are written in two
vocabularies.

**The answer, then:** `into` is the right narrow shape and would not have
unblocked this program, because the retry needed an early exit rather than an
accumulator and already has one. It would unblock counting and summing, where
the accumulator starts at `0` and no label is involved. Whether it unblocks
best-of-N depends entirely on decision 3, and that decision should be taken
before the syntax rather than discovered by it.

---

## 7. What sic will not do

### A graph assembled at run time

This is the thing being argued against, so it gets an argument rather than a
preference.

A harness built at run time out of nodes, edges and handlers is more flexible
than a declaration, and the flexibility is real: branch on a value, add a tool
because a flag was set, compose two harnesses. What it costs is the property
§2 is about, and it costs it completely rather than partially. A graph that
exists only while the process runs has no state in which it can be read and not
be running, so there is no document to approve, and every substitute for one is
a claim made by the same program about itself.

sic will not grow that, and the reason is not taste. **The manifest is the unit
of approval in this project**, and every other decision has been arranged to
keep it complete by construction: a call the `allow` block does not name is a
compile error, a grant is checked again by the broker, a plan is produced by a
reader that runs nothing. A harness assembled at run time re-opens every one of
those, because the answer to "what may this reach" becomes "it depends what it
builds".

The narrower forms of the same request get the same answer for the same reason:
a budget read from a configuration file is a bound nobody approved, a tool list
computed from a value is a manifest that is not in the manifest, and a model
allowed to choose which agent runs next is `authority.md` §11's
model-written-program with a smaller vocabulary.

What replaces it is being verbose. Five agents means five declarations and five
plan lines. A conditional harness is an `if` with two branches, both of which
the plan lists, neither of which it claims will happen. That is worse to write
and better to read, and this project has taken that trade every time it has
come up.

### A state machine, and the evidence for not building one

`CLAUDE.md` says not to build for a feature that does not exist, and the issue
that produced this document asks the exercise to say whether the harness wanted
a state-machine feature - a graph type, nodes, transitions, a scheduler over
them.

It did not, and the shape of what it wanted instead is the finding. The harness
in §4 is four functions, one `if`, one recursion and one approval. Its control
flow is control flow. The things it lacked - a retry that can be about a
validation, a comparison on a `Float`, a bound that means what it says - are
none of them a graph, and a graph would have supplied none of them. A
state-machine feature would have been an abstraction over the part that already
worked.

Recorded so the next person does not have to guess: one real harness was
written, and it wanted a conditional, a bounded repeat, and a way to say that
an answer was unusable. Two of those exist.

---

## 8. Not in this

- **Any language feature.** Six findings are above; each is separable and each
  is an issue with its argument already written. This document builds none of
  them, including the one it is most sure of (§5.6, which is a bug rather than
  a feature and is still not this document's to fix).
- **A second harness.** One program was written. `self-hosting.md` ends the
  same way and for the same reason: it is the program this repository's work
  has the shape of, and it is still one.
- **Tool use, measured.** `tools: 8` is declared in §4 and nothing exercised
  it, because that needs a driver, a pane and an agent that uses tools.
  Everything this document says about `tools` is about the declaration and the
  plan, which is what it could check.
- **A comparison with a named framework.** The claims in §2 and §3 are about
  what a mechanism can and cannot do, and they stand or fall on that. A table
  of what four libraries do this month would date faster than the argument and
  would be weaker than it.
- **A recommendation to write harnesses this way yet.** §5.1 is the reason. A
  harness that cannot retry a malformed answer is a harness for a model that
  does not produce one, and until that gap is closed the honest thing to say is
  that this is a design that has been demonstrated rather than a tool to reach
  for.
