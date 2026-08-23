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
was a difference - is out of reach. Reaching it needs the record from §1, and
the record can be added the day something needs it.

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

A grant of `process.exec` does not cover `process.capture`. They are different
authorities (§1), so they are different grants, and a program that wants both
declares both.

---

## 8. Not here

- **The record return** (§1, §2).
- **Streaming.** The value arrives when the program exits. A program that
  produces output over time, read as it comes, is a different capability with a
  different shape - and it is what driving an agent in a pane will need.
- **stdin.** Nothing writes to the child.
- **A timeout** (§6).
- **stderr as a value.** It reaches a person through the failure, not the
  program through a return.
