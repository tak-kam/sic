# Reading what a program said

`docs/design/arguments.md` gave `process.exec` something to say. This is the
other half: hearing the answer.

```sic
allow {
    process.capture "/usr/bin/git" args ["rev-parse"];
}

fn head() -> Observed<String> {
    return process.capture("/usr/bin/git", ["rev-parse", "HEAD"]);
}
```

---

## 1. A second capability, not a second return value

The alternative was one capability returning a record, `{ code: Int, out:
String }`. Two reasons against it, and the first is the one that decides:

**Reading a program's output is more authority than running it.** An exit code
is one bit. Standard output is everything the program can see: `process.exec
"/bin/cat"` tells you whether a file could be read, and `process.capture
"/bin/cat"` tells you what was in it. A manifest that cannot tell those apart is
a manifest that hides the difference between checking and exfiltrating.

So they are two grants, and `sic plan` prints two different lines.

The second reason is smaller: no capability returns a record today. Records are
user-declared `type` items, so a capability returning one needs a built-in
record type - new surface, in a change that does not need it.

**§9 is what happened when something did need it.** The first reason survived
and shaped the answer: the record is a *third* grant, `process.run`, so a
manifest still tells running from reading. The second reason turned out to be
the price, and it was paid: `Exit` is the one record type the language declares
for itself.

---

## 2. A program that fails, fails the run

`process.capture` returns the output and not the exit code, because it only
returns at all when the exit code was zero. A non-zero exit is a `CapError`,
which is what `retry` counts and what stops a run.

That is the shell's `set -e` around `$(...)`, and it is what makes the missing
exit code not a loss: if you want the code, `process.exec` still returns it; if
you want the output, a program that failed did not produce one worth reading.

What this costs, plainly: a program that exits non-zero *and* prints something
worth having - a linter reporting findings, a diff that exits 1 because there
was a difference - is out of reach through this capability. Reaching it needs
the record from §1, and the record can be added the day something needs it.

That day was issue #8, and the record is §9. `process.capture` is unchanged: a
program that failed still did not produce an answer worth reading, and the type
`Observed<String>` still means what it means.

---

## 3. stderr is not a value

It is not returned, and it is not merged into the output.

**Not merged**, because the output is going to be parsed. An agent CLI answering
in JSON, with a warning interleaved into it at whatever point the operating
system chose, produces a value that fails to parse for reasons that are not
reproducible.

**Not dropped**, because the failure message is where a person finds out what
went wrong. When the exit code is non-zero, what the program wrote to stderr
goes into the error, which is the only place it is useful.

---

## 4. A limit, and no truncation

Output is read into a value, so it needs a bound: a program that prints forever
would otherwise be a way to exhaust memory through a capability that looks like
it just runs `git`.

**Exceeding the limit fails the call.** A truncated answer that looks whole is
worse than no answer - it would parse, validate, and be wrong, which is the
failure mode this project spends the most effort avoiding.

The limit is one constant in the broker. It is generous rather than tuned; the
point is that it exists and that crossing it is loud.

---

## 5. `Observed<String>`

`docs/design/trust.md` lists `Observed<T>` as one of the trust types that were
deliberately not built:

> The same goes for `Verified<T>`, `Observed<T>` and `UserProvided<T>`: each
> needs something that produces it before the type is worth anything.

This produces one. What a program printed was not verified, not approved, and
not written by whoever wrote the sic program - it is a value that was observed,
and the type says so.

**The rule is the one `LLM<T>` already has: an observed value may not be passed
to a capability that writes or executes.** One sentence covers both kinds - *a
value nobody signed off cannot decide what gets changed or run* - and the escape
is the one that already exists:

```sic
let sha = process.capture("/usr/bin/git", ["rev-parse", "HEAD"]);
let checked = approve("check out this revision?", sha);
process.exec("/usr/bin/git", ["checkout", checked]);
```

This is strict, and deliberately: the shape it refuses is the oldest injection
there is, a string a program printed deciding what the next program runs. The
cost is that ordinary plumbing - a revision, a branch name, a path - now goes
past a person or not at all. That is the trade this language exists to make, and
it is the decision here most worth revisiting if it turns out to be wrong.

**A vector of observed strings is observed.** The rule looks through a `List`,
because `["checkout", sha]` must not be a way around it.

---

## 6. No timeout

`process.capture` refuses a `timeout`, the way `fs.write` and `llm.invoke` do.

Honouring a deadline while draining a pipe needs a reader thread: a blocked read
cannot poll a clock, and a pipe nobody drains fills and stops the child. That is
a real design, not a line of code, and until it exists the honest answer is a
refusal. Accepting the syntax and ignoring it would tell a program its call was
bounded when it was not, which is the failure `reject_timeout` exists to
prevent.

---

## 7. The grant is exec's grant

Same shape, same checks, same order: an absolute path, `args [...]` pinning what
the vector must start with, and `sha256` pinning what the file is.

```sic
allow {
    process.capture "/usr/bin/tmux" args ["capture-pane", "-p", "-t", "sic:0"];
}
```

A grant of `process.exec` does not cover `process.capture`, and neither covers
`process.run`. They are three different authorities (§1, §9), so they are three
different grants, and a program that wants two declares two.

---

## 8. Not here

- **A record return from `process.capture`.** It has one (§9), and it is a
  different capability.
- **Streaming.** The value arrives when the program exits. A program that
  produces output over time, read as it comes, is a different capability with a
  different shape - and it is what driving an agent in a pane will need.
- **stdin.** Nothing writes to the child.
- **A timeout** (§6).
- **stderr as a value.** It reaches a person through the failure, not the
  program through a return.

---

## 9. `process.run`: both facts, and a third grant

Two capabilities each answered half of one question, and the half neither
answered is the one this repository's own work is made of: **a program that
fails and prints why.** Writing this repository's development loop in sic
(#8, `docs/design/self-hosting.md`) could not be done, and the workaround -
`sh -c '... || true'` - replaced a grant naming one binary with a grant to run
anything.

So there is a third:

```sic
allow {
    process.run "/usr/bin/cargo" args ["test"];
}

fn tests() -> Exit {
    return process.run("/usr/bin/cargo", ["test", "--workspace"]);
}

fn main() -> Observed<String> {
    let r = tests();
    if r.code == 0 {
        return r.output;
    }
    return r.output;
}
```

### Three grants, because there are three authorities

§1's decisive reason was that reading a program's output is more authority than
running it, and a manifest that cannot tell them apart hides the difference
between checking and exfiltrating. That reason is why `process.run` is a third
grant rather than a flag on either of the other two:

| | what it answers | what it hides |
|---|---|---|
| `process.exec` | did it work | everything it said |
| `process.capture` | what it said, when it worked | that it failed, and what it said then |
| `process.run` | both, always | nothing |

`process.run` is strictly more than either, so it is the one a reader should
have to see named. `sic plan` gives it its own verb, `RUN`, and its own line in
the capability list. A grant of one does not cover another, the way a grant of
`process.exec` has never covered `process.capture`.

### `Exit`, and why it is not wrapped

```sic
Exit { code: Int, output: Observed<String> }
```

The provenance is on the field that has one. Wrapping the record instead -
`Observed<Exit>` - is wrong on its own terms: an exit code is produced by the
operating system rather than written by the program, so it has no provenance to
carry, and a label on it would be a claim nothing supports.

The argument first written here was a different one - that `if r.code == 0`
would not compile, because a labelled value could not be an operand (`E0371`) -
and #73 removed it. E0371 was narrowed to the operators that hand back a value
of their operands' own kind, so a comparison takes a label and answers a plain
`Bool`; `Observed<Exit>` would compare fine now. The shape of the type did not
change, because the sentence above was always the reason and the operand rule
was a second one that happened to agree. `trust.md` §2a has the narrowing.

`r.output` is exactly what `process.capture` returns, and the rule that stops it
from deciding what runs is unchanged: passing it to `process.exec` is still
`E0372`, and `approve` is still the way through.

`Exit` is the only record type the language declares for itself. A module may
not redefine it - that is `E0345`, the same diagnostic that refuses redefining
`Int`.

### On the wire, and why it is not a general record

`CapValue` grows one variant, `Exit { code, output }`, appended so that every
checkpoint written before it still reads.

The objection recorded on `CapValue::List` - that nesting buys a depth limit, a
recursive encoder and a decoder that has to refuse a hostile depth - is an
objection to a *general* record and does not reach this one. Two fields of known
type, no nesting, one more tag. Nothing here can be made to recurse.

The VM builds the object in the order `Exit` declares its fields, because
bytecode addresses a field by position. That is a coupling between two crates
and it is checked rather than commented.

### What did not change

`process.capture` still refuses a non-zero exit, and its type still says what it
said. A program that wants "the output, and a failure is a failure" is asking
for exactly that, and `retry` still counts the failures.

stderr is still not a value (§3), the output is still capped and never truncated
(§4), and a timeout is still refused (§6) - for the same reason in all three
cases, and the reasons did not become weaker by there being a third capability.

### Not here

- **`process.run` returning stderr.** §3's argument is unchanged: the output is
  going to be parsed, and interleaving is not reproducible. What a failing
  program wrote to stderr still reaches a person through the error - and now, if
  the program wants it, `2>&1` is something the program can ask its own shell
  for while still naming a real binary in its manifest.
- **A general record on the capability boundary.** `Exit` is one shape, and the
  day a second one is wanted is the day to ask whether the boundary should carry
  records at all.
- **Making `process.capture` or `process.exec` do this.** Both say something
  precise. A flag that changed what a capability returns would make a manifest
  line mean two things.
