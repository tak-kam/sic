# What a program has to say about itself

```sic
fn main() -> Int {
    log info "running the tests";
    let r = tests();
    if r.code == 0 {
        log info "they passed";
        return 0;
    }
    log warn "they failed, asking";
    let d = diagnose(r.output);
    log error d.cause;
    return r.code;
}
```

§26 of the specification has been unbuilt since phase 4, and not for want of
time: the HIR has had the instruction the whole while. What was missing was a
decision, and this document is it.

---

## 1. Why it was not just wiring

**The journal records digests, never values.** That is the sentence the whole
observability design rests on: telemetry is an exfiltration path like any other,
so a run's account is made of things that cannot leak.

A log message is a value the program wrote. Putting it in the journal breaks the
rule; putting its digest in is a record nobody can read. So `log` was not a
feature waiting to be written, it was a question waiting to be answered - and
answering it wrong would have cost more than not having logs.

Issue #8 is where the absence became concrete. Writing this repository's own
development loop in sic, the diagnosis an agent produced had nowhere to go:
`workflows/ci.sic` bound it to a name it never used, because the only channel a
workflow had was its return value and that has one type.

---

## 2. The split already existed

`docs/design/runs.md` §2 is called "`responses.jsonl` holds values, and the
journal still does not". A recorded run keeps what a capability answered in a
file *beside* the journal, and the journal keeps the digest of it. That is the
same problem, already answered once, with the reasons already argued.

So a log line goes where values already go:

| | holds | when |
|---|---|---|
| `journal.jsonl` | the level, and the digest of the message | always |
| `logs.jsonl` | the text | only with `--record` |
| stderr | the text | always |

One sentence rather than two decisions: **a log line goes where a person can see
it, and is kept where the run is kept.**

### stderr, not stdout

stdout is the value the program returned, and `sic run` prints it there. A line
saying what happened must not be mistakable for what came out.

### A run nobody recorded keeps nothing

Which is the promise `responses.jsonl` already makes, said the same way: if the
text must not be kept, do not pass `--record`. `sic explain` on such a run shows
that a line was logged, at what level, and says the text was not kept rather
than printing a digest where a sentence goes.

---

## 3. Where each part of it happens

The VM does no I/O. That is what makes the capability boundary mean anything, so
`LOG` cannot print - it emits an event, and that is its whole effect. It is the
only instruction of which that is true.

A `Sink` is code the CLI owns. `LogSink` wraps whatever sink a run has -
including the one that writes nothing - prints the line to stderr as it happens,
and appends the text to `logs.jsonl` when the run is being recorded. Wrapping
always is what makes `sic run p.sic` show a line without `--journal`: a person
watching should not have to have asked for a journal to see what the program is
doing.

Because it is a sink, the line appears **when it happens**. Collecting lines and
handing them to the driver at the next suspension - which is how the agent's
tool uses reach the journal - would print a run's first hour of output at the
end of its first hour.

---

## 4. Trust

Any value may be logged, and its provenance is erased on the way in.

The rule trust enforces is that a value nobody signed off must not decide what
gets changed or run. Logging changes nothing outside the run's own account of
itself, so there is nothing for the rule to protect. `log error d.cause` puts a
model's answer in `logs.jsonl`, which is a file that already holds model answers
- `responses.jsonl` is full of them.

**`Secret<T>` is where this changes**, and the rule is already written down for
its sibling: `observability.md` §5 says a `Secret<T>` never reaches an attribute
at all. A logged one would be the same rule at the same boundary, and it is not
enforceable today because nothing produces a `Secret<T>` - see `trust.md` §5.

---

## 5. What the instruction is

`LOG`, opcode 30, ABC form: `a` is the level and `b` is the register holding the
message. The verifier checks that `b` holds a string and that `a` names one of
four levels, because a file that decodes says nothing about what is in it.

The level is a number in the bytecode and a word everywhere else. The numbers
are part of the file format like an opcode's: a reader that took `2` for `warn`
would report the wrong thing about a run.

`log` and the four levels are ordinary identifiers rather than keywords, the way
`args`, `sha256`, `repeatable` and `delegable` are. `log info "x"` is two
identifiers in a row, which no expression can be, so the parser recognises the
statement by its shape and a program may still have a function called `log`.

That shape is also what makes a mistyped level a good error: `log shout "x"` is
`E0218` naming the four, rather than a parser guessing at an expression.

---

## 6. Not here

- **Fields.** `log info "done" { cause: d.cause }` is not syntax, and the HIR no
  longer carries a `fields` vector nothing fills. A message can be any
  expression, so a program can say what happened rather than only that
  something did; structured fields are a second shape with their own questions
  - which types may be a field, what a field's provenance means - and they come
  back with the syntax that needs them.
- **Levels doing anything.** All four are recorded and shown. Filtering is a
  policy, and a policy needs somewhere to be written; nothing has wanted one.
- **OTLP log records.** A log line is a log record, and this converts a journal
  into traces and metrics. OTLP's third signal is its own document with its own
  shape, and putting the text on a span instead would be the wrong signal chosen
  because it was nearer. `observability.md` §5 keeps its entry, with the reason
  changed from "there is no log event" to "there is a third signal and this
  emits two".
- **Sending anything anywhere.** Same reason every other signal has: sending is
  an external effect and an external effect is a capability.
