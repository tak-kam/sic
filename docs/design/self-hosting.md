# Using sic to build sic

Issue #8 asked whether this repository's own development loop can be written as
a sic program, on the grounds that a program that cannot be written says
something no amount of design prose does.

The file in the repository is a template: every path in its `allow` block is a
placeholder, because a manifest names files on one machine and those are not
anybody else's. A copy with real paths belongs beside it and out of git -
`.gitignore` has `workflows/*.local.sic`. There is deliberately no way to write
"read this from the environment": a manifest that deferred to a variable would
hand back the decision `in` and `env` were added to take.

It can. `workflows/ci.sic` runs this repository's test suite, hands the output
to an agent, and requires the answer to fit a record type before the program can
read a field of it. It plans, it runs, it checkpoints at the model call, and
`sic explain` reads the whole thing back afterwards - including the question the
model was asked and the reason a person gave for the answer.

That is the bar #8 set, and it is met. What follows is the other half of the
exercise: the seven things that bent the program on the way, each of which is
now an issue with the argument written where the need was found.

## 1. The plan says two things about a shell, and both are true

This is the finding worth the whole exercise. `sic plan workflows/ci.sic`
prints, three lines apart:

```text
  process.capture [exec]  "/bin/sh"  args ["-c"]  (not pinned)
  llm.invoke      [invoke]  "claude-opus-4"  (not pinned)
    the agent may use  "/bin/sh"                (through the broker)
    the agent may not  run a shell              (refused by the hook)
```

Both lines are accurate about their own mechanism. The hook denies the agent its
own `Bash` tool; the broker offers `process.capture` as an MCP tool because
`reach_of` cannot translate a shell command into a permission rule. The comment
on that decision says exactly why - "a rule on a shell command is a match on a
string that can invoke anything it likes" - and then routes it, which hands the
agent the thing the rule could not express.

`docs/design/authority.md`'s claim survives in the letter: the agent's authority
is the program's manifest, and the manifest did grant `sh -c`. What does not
survive is the plan being a document somebody can approve. A reader is told, in
adjacent lines, that the agent may and may not have a shell.

→ #49, **fixed**: a `process` grant is the program's until it says
`delegable`, so the default plan no longer offers the agent a shell it was never
asked to hand over. See `docs/design/authority.md` §4a.

## 2. Output or exit code, never both, measured

`docs/design/output.md` §2 chose this deliberately and named the case it rules
out: a build that fails *and* prints why. This repository's development loop is
that case, so the workflow could not be written as intended.

`process.capture` returns output only when the program exited zero; a test suite
that fails is the entire reason to run one. `process.exec` returns the code and
throws the output away. The failure output does reach sic - it travels in the
`CapError` message - and is then unavailable to the program that asked for it.

The workaround is in the file, and it is the finding rather than the fix:

```sic
process.capture("/bin/sh", ["-c", "... 2>&1 | tail -40 || true"])
```

A grant that named one binary and pinned its first argument becomes a grant to
run anything, which is what §1 is about. The two findings are the same finding
seen from either end: **the cheapest way around "output or exit code" is a
grant that gives away everything.**

→ #50, **fixed**: `process.run` returns an `Exit` of both facts, so
`workflows/ci.sic` names `cargo` in its manifest instead of a shell. See
`docs/design/output.md` §9.

## 3. A call gets no environment, and inherits a directory

`process.exec` and `process.capture` both call `env_clear()`, which is right and
which nobody argued for anywhere a reader would find it. Neither sets a working
directory, so the child inherits whichever one `sic` was started in.

So the same bytecode, with the same manifest, does different things depending on
the shell that ran it - and `sic plan`, whose whole job is to say what a program
may do before it does it, cannot mention the directory because it is not in the
program.

The measurement is in the run this document is written from. `workflows/ci.sic`
tries to use the build workaround `CLAUDE.md` documents for a machine without a
C linker:

```sh
LLD="$(rustc --print sysroot)/..."; RUSTFLAGS="-Clinker=$LLD ..." cargo test
```

`rustc` is not found, because there is no `PATH`. `$(...)` expands to nothing,
`RUSTFLAGS` names a linker at `/lib/rustlib/...`, and the tests fail for a
reason that has nothing to do with the tests. **This repository's own documented
build command cannot be expressed as a sic capability call.**

→ #51, **fixed**: a `process` grant says `in "/abs/path"` and
`env { NAME: "value" }`, so `workflows/ci.sic` now carries this repository's own
documented build command and runs the same from any shell. `sic plan` says which
of the two a grant depends on. See `docs/design/capabilities.md`.

## 4. An agent call cannot be given a deadline

`retry` and `timeout` attach to capability calls. An agent call is a function
call, so `E0330` refuses both:

```text
error[E0330]: `retry` and `timeout` apply to capability calls only
```

The driver has a compiled-in thirty minutes instead. `cargo test --workspace`
on this repository takes about a minute, and a diagnosis that has not arrived in
five is not going to. Thirty is a number nobody chose for this program, and the
`agent` declaration - which already carries `budget`, `tools` and `memory` - is
where a chosen one would go.

→ #52, **half a finding**: an `agent` *can* be given a deadline - `deadline` on
the declaration, which reaches the driver and which `sic plan` prints. What
cannot be given one is an agent *call*, through `timeout`, and E0330 is right to
refuse it. What was true is that a program which sets none gets the driver's
thirty minutes and no plan said so; the plan says it now, and E0330's note
points at the declaration instead of claiming there is nothing to wait for.

## 5. A long string is one long line

A string literal cannot be broken across lines: the escapes are `\"`, `\\`,
`\n`, `\t`, `\r`, `\0` and `\u{...}`, and a backslash before a newline is
`E0105`. Two strings cannot be joined either - `+` is `Int` only (`E0303`).

So the shell command in `workflows/ci.sic` is one physical line of 286
characters, in a repository whose own Rust is wrapped at 80. There is no way to
write it otherwise.

#8 listed "no string concatenation" as a known obstacle, and predicted it would
bite when a program built a prompt from parts. It bit somewhere else and harder:
the problem is not composing a value at runtime, it is writing a literal down.

→ #53, **fixed**: a backslash at the end of a line joins it to the next and
eats the indentation, so `workflows/ci.sic` no longer has a 171-character line.
Joining two strings at run time is the other half and is still not here; §5
argues that the literal was the half that mattered.

## 6. `sic explain` puts a budget charge above the call that spent it

Reading the run back:

```text
          call process.capture
            process.capture answered sha256:8a2a14c9
          budget: 0 left
        call llm.invoke
```

The budget belongs to `llm.invoke`, which is the only budgeted site in the
program. The journal is right - the charge and the call share span 5 - and the
rendering is not, because the charge is emitted before the call it pays for and
`explain` prints events in order with nothing tying the two together.

This is the same ordering #28 dealt with in the OTLP exporter, where the fix was
to hold the charge until its span opens. `explain` was not part of that change.

→ #54, **fixed**: `explain` holds a charge until its call arrives and prints
the two together, the same way the OTLP exporter does since #28. A charge whose
call never arrives - a journal cut between the two - is said rather than
dropped.

## 7. Passing no arguments needs a type annotation

```sic
process.capture("/bin/pwd", [])
```

is `E0342`, "an empty list needs a type annotation", so a call that passes no
arguments is:

```sic
let none: List<String> = [];
process.capture("/bin/pwd", none);
```

The capability's parameter type is known - it is `List<String>` and cannot be
anything else - so this is a place the checker has the answer and asks anyway.

→ #55, **fixed**: an empty list literal takes the type its position already
names - a `let` annotation, a parameter, a return type. It is still `E0342`
where nothing says, because guessing there would move the error to wherever the
list is used.

## What did not stop it

Worth recording, because #8 predicted some of these and the point of writing the
program was to find out rather than to guess.

**Recursion instead of loops** reads fine. "Run the tests up to three times" is a
function taking a count and calling itself, and `sic plan` says honestly that
"how often they run depends on the path taken" rather than pretending to know.

**`budget` counting calls** did not bite here, because the workflow makes one
model call. #16 already replaced the unit for the case that would have.

**Building a prompt from parts** did not come up. The agent's input is the
captured output, which is one value. §5 is the real version of this problem.

**Trust did not get in the way**, and it is worth asking whether it should have.
`Observed<String>` - text a program printed - was accepted as an agent's input
without complaint. That is defensible: the agent's authority is bounded by the
manifest whatever it reads, which is what `authority.md` is for. It is not
written down anywhere as a decision, and a reader who has just been told that an
`Observed` value may not decide what runs may be surprised that it may decide
what an agent is asked.

## What it says about the run, since #62

The workflow now asks what it is testing before it tests anything:

```sic
fn what_is_being_tested() -> Int {
    log info git.rev_parse("HEAD");
    return len(git.status());
}
```

That is worth more here than the two lines suggest. A test result that is not
tied to a state of the repository is a fact about nothing, and until there was
a `git` capability the workflow had no way to say which state - the honest
alternatives were a shell, or a `process.run "/usr/bin/git"` grant whose
manifest could not say that the repository's own hooks and config would not
run. `sic explain` on a recorded run now says which commit it was about and how
much was not in it.

It reports rather than refuses. A tree with uncommitted work in it is the
normal case while somebody is working, and a workflow that stopped there would
be refusing to test the thing it was written to test.

## What the language gained, and what the workflow does with it

Three of the seven things that bent this program are now in the language, and
the workflow uses each where it earns its place rather than to demonstrate it:

**`for` over a list** (#66) names the uncommitted files instead of counting
them. A count answers "is this a test of HEAD"; the names answer "of what,
then", which is the question somebody reading the record a week later has.

**`contains`** (#68) tells a build that did not compile apart from a test that
failed. Until it existed, this program held everything cargo printed and could
answer exactly one question about it - how long it was - so it asked the agent
"why did these tests fail" about output that was sometimes a compiler error,
spending the one model call it is budgeted for on a question whose premise was
false.

That is also the first place this workflow takes on a dependency the manifest
cannot express: it matches on `test result: FAILED`, which is cargo's wording
and not a promise to anybody. If that changes, the branch goes the other way
and the diagnosis says so - visibly wrong rather than quietly wrong - and the
comment in the source says which it is.

**`+` on `String`** (#69) puts the commit and the file name in the log line
rather than beside it, so `sic explain` reads as sentences.

What is still missing is what #66 found: a `for` loop cannot fold, because
nothing in the language assigns. Every use above performs an effect per
element, which is the half a loop can do.

## What this is not

**Not a replacement for `.github/workflows/ci.yml`.** CI runs on a machine with
no agent and no tmux, and should stay a thing that works without either. #8 said
so and it is still right.

**Not a recommendation to write workflows this way yet.** §1 and §2 together
mean the honest form of this program grants a shell. Until a program can read
what a failing program said, the workflow that motivated this exercise is one
whose manifest a careful reader should refuse.

**Not a general answer about the language.** One program was written. It is the
program this repository's own work has the shape of, which is why it is worth
more than a synthetic one, and it is still one.
