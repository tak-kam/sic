# A person who is present

`sic run` assumes the person who answers a run is somewhere else. When
`approve`, `choose`, or an `llm.invoke` with nothing to answer it comes up, the
run is written out, a hint is printed, and the process exits 3. The answer
arrives later, from another command:

```console
$ sic run workflows/ci.sic --record
  ...
  waiting: [merge the branch] the tests passed. merge?
  answer with: sic attach 3f2a91c4 --value <VALUE>
$ sic attach 3f2a91c4 --value true --because "tests pass"
```

That shape is correct and this document does not propose changing it. It is
what makes a run survive the terminal it started in, the machine it started on,
and the person who started it. `human.approve` exists because a person is meant
to be in the loop, and in the case sic is for, the loop is asynchronous: the
answer comes an hour later, from CI, from a queue, from somebody else's laptop.

But the person is not always somewhere else. While a workflow is being written
they are almost always sitting in front of it, and they are paying the
asynchronous price at every iteration: run, read, copy an id, type a second
command, repeat. Nothing about the design requires that. A person who is
present can be asked.

---

## 1. What this is

`--interactive`, on `run` and `attach`: when the run stops for an answer, print
the question on the terminal, read the answer from it, and carry on. A run that
stops again asks again.

Not on `resume`, and §4 says why.

A run without the flag is a **non-interactive run**, and that is the term used
throughout this document and in the code. It is a negation, and it is still the
right word: it is what `bash` calls the same distinction in its own manual, and
unlike "batch" it does not suggest that runs arrive in groups. One name, so
that the case `sic` is actually for is a thing with a name rather than three
ways of saying "not the other one".

Nothing about what an answer *is* changes. The question is the text the broker
already produced, character for character - the same text `sic attach` prints,
with `human.choose`'s alternatives already numbered. The value is parsed by
`parse_answer` in the type the capability returns. The reason is what
`--because` records.

---

## 2. The shape: the cycle that already exists, in a loop

The whole of this feature is that **`sic attach` is already the operation
"answer one question and continue"**, and nothing says it can only be reached
from a second command line.

A run today does this:

```text
run --record  ->  suspend  ->  checkpoint written  ->  exit 3
                                                        |
                          sic attach ID --value V  <-----+
                                       |
                                       ->  restore  ->  continue  ->  suspend...
```

Interactive mode closes the arrow:

```text
run --interactive --record  ->  suspend  ->  checkpoint written
                                                  |
                                            ask the terminal
                                                  |
                                     restore  ->  continue  ->  suspend...
```

No new message crosses the wire, no new state exists, and no new thing can go
wrong. The seam is the one `docs/design/processes.md` §5c already built:
`pick_up` continues a waiting run in either shape and says whether it is
waiting again. Interactive mode calls it while the answer is `Some`.

This matters more than it reads. The alternative design - branch inside the
driver loop where the broker returns `Deferred`, prompt there, and resume the
`Vm` in place - is fewer instructions and much worse. It would need a way to
produce a checkpoint mid-conversation in the two-process shape, which is a new
protocol message; it would put a terminal read inside the loop that is
otherwise a pure function of the broker's answers; and it would create a run
that was never written out, which is exactly what §4 says must not exist.

The cost of the shape chosen instead is a restore per question, and in the
isolated shape a child process per question. A restore is what `sic resume`
does; a `fork` and an `exec` is what every run already does once. For a
workflow with five approvals in it this is five of each, and neither is
measurable next to the person deciding.

---

## 3. It is never the default

The non-interactive run is the case sic is for. A run answered by a queue, by
CI, by somebody on another continent the following morning - that is why a
checkpoint is a file and why `sic runs --waiting` exists.

If `--interactive` had to be turned *off*, the non-interactive run would be the
special one, and every script that ever inherited a terminal would hang the
first time somebody added an `approve` to a workflow. So it is typed, every
time, and the default is what it is today.

---

## 4. The checkpoint is written first, and that is what makes this free

The prompt is an addition to suspending, not a replacement for it. By the time
a question is on the screen, the run is already on disk and already in `sic
runs --waiting`.

That is not a precaution, it is the whole safety argument. Ctrl-C at the
question, a terminal that closes, an ssh session that drops, a laptop that
sleeps and never comes back: in every one of those the run is exactly where a
non-interactive run would have left it, and can be answered tomorrow the old
way. **The worst case of an interactive run is a non-interactive run.** A
feature that can only make things better is one that needs no argument about
when to use it.

Getting this for nothing is the reason §2 chose the shape it did: the
checkpoint is written by suspending, and suspending is what happens before the
prompt, so there is no ordering to get right and no code that could get it
wrong.

### It follows that the run needs somewhere to be saved

A run with no `--record` and no `--checkpoint` has nowhere to put its state and
already says so:

```text
error: the run is waiting for `...` and has nowhere to be saved
       pass --checkpoint PATH to write its state out
```

`--interactive` cannot make that case work, because the thing it would have to
give up is the safety net that justifies it. So it refuses:

```text
error: `--interactive` keeps the run it is asking about, so it needs --record
```

Implying `--record` would be the friendlier design and the wrong one. `sic`
does not turn flags on for people - a manifest names an absolute path, a grant
writes out an environment, and `workflows/ci.sic` has no way to read a path out
of the environment. A command line that quietly started keeping a run in a
store the person did not mention would be the same kind of surprise.

### And that `resume` is not one of the commands that take it

`--record` rather than "either `--record` or `--checkpoint`", because a loose
checkpoint is the wrong thing to answer a person's questions from. It has no
run behind it, and three things follow from that, none of them fixable here:

| | a recorded run | a loose checkpoint |
|---|---|---|
| a reason for the answer | recorded beside it | nowhere to put it; `resume` refuses `--because` for exactly this reason |
| a conversation with an agent | found by the run's id | a checkpoint does not say which run it came from, and `resume` already refuses `--llm` when a program keeps one |
| where the next stop is written | the store, by construction | wherever `--checkpoint` says, which has to be typed again |

An interactive `resume` would ask a question it could not record the answer
to, offer a reason it would throw away, and need its own next destination
named on the command line - three special cases, to reach the same place
`--record` reaches with none. So `sic run --record --interactive` is the
interactive path, and `sic resume` stays what it is: the way to pick up a
checkpoint from wherever it ended up, with the answer supplied.

---

## 5. The journal cannot tell the difference

An approval typed at a terminal and one that arrived through `sic attach` are
the same fact about the same run: a person was asked this, and answered that,
for this reason. The journal records the digest of the answer and the store
keeps the value beside the question, and neither gains a field saying which
keyboard it came from.

If it did, `HumanApproved<T>` would mean two things, and `sic recheck` of a
recording made one way could not answer a program that was going to be run the
other. The trust type is about *who* answered, not about *where they were
standing*.

This is free too, and for the same reason as §4: interactive mode answers
through `attach`'s own recording path, so there is nothing to keep consistent.

---

## 6. No terminal, no prompt

`--interactive` without a terminal on stdin is an error, not a wait:

```text
error: `--interactive` needs a terminal, and stdin is not one
```

A CI job that inherits the flag by accident has to fail in a second rather than
hang until somebody notices a queue backing up. `std::io::IsTerminal` has been
stable since 1.70; the manifest declares 1.85, so this is `std`.

And it is checked *before* the program runs, not at the first question. A run
that performed three effects and then discovered it could not ask anybody is a
run that has to be picked up by hand, which is the situation the flag was for.

### Why a flag rather than looking for a terminal

Checking for a terminal and switching behaviour is the more convenient design,
and it is the wrong one here. `sic` has refused ambient configuration
everywhere it came up:

| the ambient thing | what replaced it |
|---|---|
| `PATH` | a grant names an absolute path |
| the inherited environment | `env_clear`, then `env { }` in the manifest |
| the working directory | `in "/abs"` on the grant |
| a variable in the environment | nothing; the path is written out |

"The same command line does different things depending on how it was invoked"
is what every row of that table exists to prevent. A tty is ambient. So
`--interactive` is typed, and it *additionally* requires a terminal - the flag
says what is wanted, the check says whether it is possible, and neither one
guesses.

---

## 7. What the prompt asks

Two lines per question, the second skippable:

```console
waiting: [merge the branch] the tests passed. merge?
answer (Bool): true
why (optional): the flake in journal::rotate was fixed in 9f31465
```

The type is shown because `parse_answer` is going to insist on it, and being
told after the fact that `yes` is not a `Bool` is worse than being told before.
An answer that does not parse is re-asked rather than fatal: the run is saved,
the person is present, and there is no reason to make them start the command
again.

The reason is asked for every capability rather than only for the two where it
reads as a decision. One rule is easier to hold than a table of exceptions, and
`sic attach --because` is already accepted for all of them.

End of input - Ctrl-D - is not an answer. It leaves the run waiting, exits 3,
and prints the same hint a non-interactive run would have printed, because at
that point that is exactly what the run is.

---

## 8. What it buys past the typing

`--interactive --record` turns a session of human judgement into a test case.

Answer the questions once as they come, and the run is recorded with its
answers in `responses.jsonl`. `sic recheck <ID> <FILE.sic>` then runs an edited
program against those answers and says whether it still asks what was answered
(`docs/design/runs.md`).

There is no way to produce such a recording today except by answering each
question from a separate command line, which is precisely the friction that
means nobody produces one. A regression test for a workflow's decisions is
worth more than the keystrokes it costs, and this makes it cost none.

---

## 9. Deliberately not here

- **A REPL.** Evaluating expressions one at a time contradicts what `sic plan`
  promises: that what a program may do is readable from the whole of its
  bytecode before any of it runs. A manifest is a property of a program, not of
  a line, and a language with no loops and a fuel budget is not one anybody
  wants to type into a prompt.
- **A stepper or a debugger.** Stopping on an instruction and reading a
  register is a different feature for a different audience - people working on
  sic rather than people writing sic - and `sic disasm` with the journal
  already covers most of it.
- **Editing the program from the prompt.** The bytecode the run started with is
  what the checkpoint's digest is tied to, and `resume` checks it.
- **Prompting in front of effects the broker performs.** `process.run` and
  `fs.read` are answered inside the call. Asking a person to confirm one would
  be a proposal about consent, not about where the person is sitting, and it
  would need its own argument about what a grant already decided.
- **Readline, history, completion, colour.** A line of text off the terminal,
  and no dependency.
- **A prompt for `--llm`-answered calls.** If something is driving the agent,
  it is answering; interactive mode is for the calls that defer.

---

## 10. What is tested, and the one thing that is not

1. ~~Reading an answer from a terminal: the prompt, the type, the re-ask, and
   Ctrl-D.~~ **Done**, in `cmd::ask`, against a slice rather than a terminal -
   which is the point: what counts as an answer is a decision, and what a tty
   is is not.
2. ~~`--interactive` on `attach`.~~ **Done.** `attach` became one round plus a
   loop, and the round is `sic attach` unchanged.
3. ~~`--interactive` on `run`.~~ **Done**, and it is four lines: the run
   returns the id of a recorded run that stopped, and `attach`'s loop takes it
   from there. Both shapes had to learn to say whether the run was waiting,
   which is the same `(code, waiting)` pair `pick_up` already returned.
4. ~~The refusals.~~ **Done**, and the order between them turned out to matter:
   `--record` is checked before the terminal is. What was typed is wrong or
   right on its own; whether there is a terminal is a fact about the machine.
   Being told to add `--record` only after settling the terminal question would
   be two round trips for one command line - and, not by accident, it is also
   what makes the `--record` refusal testable on a machine with no terminal.

**The loop itself has no end-to-end test.** Driving it needs a pseudoterminal,
and allocating one needs `ioctl`, which needs a dependency. So the reading is
unit-tested against a slice, the refusals are tested end to end, and the loop
is covered only by the two of those meeting in the middle.

That is the same trade the `--llm tmux:` tests make - they check every refusal
and never drive an agent - and it is worth naming rather than leaving as a gap
somebody finds later. What would close it is a dependency, and
`docs/design/` is where that argument would have to be made first.
