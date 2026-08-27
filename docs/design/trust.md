# Trust and provenance

Where a value came from decides what it may be used for.

```text
agent plan_deploy { input: String, output: Plan }

fn main() -> Int {
    let plan = plan_deploy(logs);          // LLM<Plan>
    let approved = approve("deploy?", plan); // HumanApproved<Plan>
    return deploy(approved);                 // takes HumanApproved<Plan>
}
```

Passing `plan` straight to `deploy` does not compile. That is the whole point:
the model's answer and the answer a person signed off are different values, and
a type system is where "different" is enforced rather than remembered.

---

## 1. Provenance is inferred, not annotated

A trust type is never written on a value that is being produced. It is attached
by whatever produced it:

| Type | Attached by |
|---|---|
| `LLM<T>` | an agent's output |
| `HumanApproved<T>` | `approve(question, value)` |

It can be written in a signature, which is the useful direction: a function says
what it will accept, and callers have to produce it.

```text
fn deploy(plan: HumanApproved<Plan>) -> Int { ... }
```

Annotating a value into existence - writing `let x: HumanApproved<Plan> = ...`
over a plain one - is not possible, because there would be no point to any of
this if it were.

---

## 2. What it forbids

**A `LLM<T>` cannot reach a capability that changes something.** Concretely: no
argument of a `write` or `exec` capability may carry `LLM`. The label is
attached by `llm.invoke` itself, in the capability table, so it does not matter
whether a program declared an `agent` or called the capability directly - #72
was that it did matter, and the lower-level spelling was exempt from this
sentence. Reading and invoking
are fine - asking a model about a model's answer is ordinary, and so is reading
a file whose name it suggested - which is why the rule is about the
capability's kind rather than about the value.

**A trust type is not its inner type.** `LLM<Int> + 1` does not compile, and
neither does passing `LLM<Plan>` where `Plan` is expected. Arithmetic on a value
whose provenance matters is exactly where provenance gets lost.

**Reading a field keeps the provenance.** `LLM<Diagnosis>.cause` is
`LLM<String>`, not `String`. A field of a model's answer is still the model's
answer, and the alternative - losing the label at the first field access - would
make the whole thing decorative.

---

## 2a. What a trusted value may decide

§2 says what a label forbids. It does not say whether a label reaches control
flow, and the answer has been assembled out of three unrelated decisions rather
than taken once. This section takes it.

### A branch is not an effect

A model choosing which way a program goes is not, on its own, something this
language protects against. **The manifest is the unit of approval, not the
path.** A reader who approves

```sic
allow {
    process.exec "/usr/bin/deploy";
    fs.write "./rollback.log";
}
```

has approved a program that may deploy *or* may write that file. Which one
happens on a given run already depends on a file's contents, on an exit code,
on what a person answered - and now on what a model said. None of those widens
what the run may do, because none of them can reach past the manifest, and
`sic plan` prints the manifest.

So there is no rule that a branch condition must be untrusted, and this
document should not be read as implying one.

### E0371 is about laundering, not about branching

This is the part that is easy to get backwards, and was:

```text
error[E0371]: LLM<Int> cannot be used as an operand
```

That looks like a rule against a model deciding a branch. It is not. §2 gives
its actual reason - "Arithmetic on a value whose provenance matters is exactly
where provenance gets lost" - and the shape it protects is this:

```sic
let clean: Int = d.severity + 0;   // if this compiled, the label is gone
```

An operator takes a labelled value and answers an unlabelled one. The rule
refuses that, everywhere, so that:

> **Once attached, a label never comes off.**

Not through `approve` either, and this is worth being precise about because the
first draft of this section got it wrong: `approve` turns `LLM<T>` into
`HumanApproved<T>`, which is still a label. E0371 refuses it as an operand just
the same - `if approved.severity > 5` does not compile - and what changed is
only which capabilities the value may now reach (§2, §3). A labelled value is
**opaque to computation, whoever vouched for it**: capabilities may take it,
fields and elements keep its label, and nothing hands the program back a plain
value to compute with.

That sentence is the whole of the trust system's guarantee, and the three rules
in §2 are each a consequence of it. What a person's approval buys is reach, not
arithmetic - and that is right, because the person approved *using* the value,
not every conclusion a program might derive from it.

### `len` is the exception, deliberately

The one place the program gets a computable value out of a labelled one:

```sic
let d = diagnose("why did it fail?");   // LLM<Diagnosis>
if len(d.cause) > 5 { … }               // compiles, today
```

`len` takes a trusted value and answers a plain `Int`. The comment in
`check_len` gives the reason - "How long something is says nothing about where
it came from" - and that is true about the *number*: a length is a fact about
the value rather than the value.

It is not true about what the number lets somebody decide. A branch needs one
bit, and a model asked to "answer yes or no" controls the length of its own
answer. **So `len` is a channel from a model to a branch, and calling it
anything else would be pretending.**

It stays, and the reason is the section above rather than the comment in the
checker: a branch is not an effect, so a channel to a branch is not a leak. The
label has not been laundered onto a *value* - nobody can get the model's answer
back out of its length - and no capability call has become reachable that the
manifest did not already list.

What `len` must not become is a precedent for stripping labels off values. The
test of whether the next builtin may do it is: **can the result be used where
the labelled value could not?** A length cannot be written to a file, passed to
`exec`, or turned back into the answer. A "first line of", or a "trimmed", or a
"parsed as an integer" could all be argued as facts about a value, and every
one of them hands back something the label was protecting.

### `contains` and `starts_with` take it off too, and widen it

Two builtins ask a question about a string and answer a `Bool`:

```sic
let out = process.capture(...);          // Observed<String>
if contains(out, "warning:") { … }       // compiles
if starts_with(path, "/safe/dir") { … }  // compiles
```

Either argument may be labelled, and the answer is a plain `Bool`. Put through
the test above - can the result be used where the labelled value could not? -
they pass for `len`'s reason and no other: a `Bool` reaches no capability, is
written to no file, and nobody can get the string back out of it.

That is the whole of the argument for them, and it is worth being clear about
what it does not cover. **`len` gives a program one number about a value it may
not read; these give it a question of its own choosing, and a question may be
asked again.** The consequence is concrete rather than theoretical:

```sic
if starts_with(head, "deadbeef") && len(head) == 8 { … }
```

That is byte equality of a labelled string, spelled out of two things this
document allows, and `head == "deadbeef"` is the same test written with an
operator - which E0371 refuses. So after this change E0371's refusal is about
what an expression **hands back**, not about what a program can find out. It
was already only that: `len(x) == 1` finds out a great deal. This makes it
plain, and the honest summary of the trust system's guarantee is the sentence
above - *a labelled value is opaque to computation* - with "except that a
program may ask yes-or-no questions about it" attached.

The alternative was to propagate the label, and it was not close. Every
question these builtins were added for is about what a capability reported -
what a command printed, what a repository said - and that is `Observed<T>`. A
`contains` that answered `Observed<Bool>` would answer something that is not a
`Bool`, cannot be an `if` condition, and cannot be `!`-ed either. The choice was
between a channel to a branch, which the first subsection here accepts on
purpose, and two builtins that refuse every program they were written for.

### `starts_with` is not a security check, and will look like one

This is the shape somebody will write:

```sic
let path = plan_path(request);            // LLM<String>
if starts_with(path, "/safe/dir/") {
    fs.write(path, contents);             // error[E0372]
}
```

It does not compile, and that is the point worth recording: the guard proves
nothing about the value, so the value keeps its label and E0372 still refuses
it. Nothing here is a door out of §2 - a `Bool` came out, and the string went
nowhere.

The second half is about the programs where it does compile, because the path
came from somewhere unlabelled. **A prefix of bytes is not containment in a
filesystem**: `/safe/dir/../../etc/passwd` starts with `/safe/dir/`, and so does
`/safe/dir-of-somebody-else`. `starts_with` answers exactly what it says and
nothing about where a path leads. What actually holds a run to its grants is the
broker checking the manifest before it performs anything, which is a check
`sic plan` prints and a program cannot skip; a program's own prefix test is a
question it asks about text, and this document is not going to let it be read as
more than that.

### Joining two strings keeps the label

`+` on two strings is the one operator E0371 does not refuse, and the reason is
the sentence above rather than an exception to it.

Read what E0371 is for again: *an operator takes a labelled value and answers an
unlabelled one*, and the rule refuses that so that once attached, a label never
comes off. Joining does not do that. It answers a **labelled** value:

```sic
let sentence = "the agent says: " + d.cause;   // LLM<String>
```

so nothing is handed back plain, and `process.exec` refuses `sentence` for
exactly the reason it refuses `d.cause`. That puts joining beside reading a
field and indexing a list - the operations that carry a label onward - rather
than beside arithmetic, which is where it would sit if it produced a `String`.

The alternative was not "refuse it": it was `"" + tainted` being a plain
`String`, which is laundering with an extra character. The label is contagious
because the bytes are, and it does not matter which side the literal is on.

**Two operands with different labels are refused** (E0375):

```text
error[E0375]: `+` cannot join LLM<String> with Observed<String>
```

Not because joining them is dangerous - the honest answer is that either label
would be safe, since both carry the same restriction. Because there is no name
for the result. `Types::trust` already says a value has one origin, and picking
a winner between "a model said it" and "a program printed it" needs an order
between the labels that nothing else in this document has, invented here for a
program nobody has written. `HumanApproved` makes that plainer: a person
approved one of the two values, and calling the join approved would be claiming
they approved a sentence they never saw. Refusing costs a workaround, and the
program that finds the workaround intolerable is the issue that argues for the
order.

What this does **not** buy, and the issue that asked for it says it does:
`approve(question, value)` takes a plain `String` question, so
`approve("commit the fix for: " + d.cause, d)` is still refused - now with
E0301 rather than E0303. Which builtins erase a label on an argument they only
show to a person is a separate decision from what an operator does, and it is
not taken here.

### The asymmetry, said out loud

If a branch is not an effect, then `if d.severity > 5` could be allowed too,
and E0371 could be narrowed to the operators that produce a *value* the program
keeps. It is not narrowed, and this document is not narrowing it: refusing more
than is strictly necessary costs a workaround, and allowing more than is
necessary costs an argument every time somebody asks why. If the refusal turns
out to be in the way of a real program, that is the issue to write, and this
section is what it has to answer.

### What is checked

| claim | how |
|---|---|
| a labelled value is not an operand, whoever vouched for it | E0371, with tests for `LLM<T>`, `HumanChosen<T>` and `HumanApproved<T>` |
| both spellings of asking a model are labelled | `llm.invoke` carries the label in the capability table, so a direct call and an `agent` are checked alike |
| a labelled value does not reach a changing capability | E0372 |
| joining two strings keeps the label, on either side | tested in both operand positions, because a rule about `a + b` that only holds for `a` is not a rule |
| joining two different labels is refused | E0375 |
| reading a field keeps the label | §2, tested |
| `len` strips it, and that is on purpose | a test that says so, so that changing it is a decision rather than a regression |
| `contains` and `starts_with` strip it too | a test that a labelled string may be asked, beside one that the answer buys it nothing: the same value still cannot reach `fs.write` |

The last row matters more than it looks. Before this section, `len`'s behaviour
was one sentence in a checker and nothing in a test: an edit that made it
propagate would have looked like a fix.

### What is deliberately not here

- **A rule about branch conditions.** The manifest is the unit of approval.
- **`Observed<T>`.** Text a program printed carries a different argument
  (`docs/design/output.md`), and it reaches `len` the same way for the same
  reason - `len(git.status()) > 0` is the question that capability exists to
  answer.
- **Narrowing E0371.** Above. `+` on two strings is not a narrowing of it: the
  rule is about an operator answering an unlabelled value, and that one does
  not.
- **An order between the labels.** E0375 refuses a join of two rather than
  ranking them.
- **A rule about what a builtin does to a label on its arguments.** `approve`
  and `choose` want a plain `String` question; `len` takes any label and
  answers a plain `Int`. Those are three separate decisions and none of them is
  taken by the operator table.
- **A channel budget.** Counting how many bits a model can push through `len`
  or a run of `contains` calls is information-flow analysis, and this language
  is not going to have one.
- **A rule about which argument may carry a label.** Both may. A model that
  chooses the needle is choosing which question gets asked, and choosing a
  question is choosing a branch.
---

## 3. `approve`

```text
approve(question: String, value: LLM<T>) -> HumanApproved<T>
```

It asks `human.approve` with the question **and with the value**, and if the
answer is no, the run fails. There is no third outcome to return: without an
option type, "approved or not" would have to be a `Bool` beside the value, and
nothing would stop the program from ignoring it.

`approve` needs the `human.approve` capability granted, like any other path to
an effect. It is the only way to produce a `HumanApproved<T>`, which is what
makes the type mean anything.

### What the type means

> **`HumanApproved<T>` means: a person was asked about this value, at this point
> in this run, was shown it, and said yes - and what they were shown, what they
> answered, and why, are in the run's record.**

That sentence did not exist until #74, and while it did not, the type meant less
than a reader of this document would have taken it to mean. `approve` lowered to
`CALL_CAP human.approve(question)` - the question only. The value was moved into
the destination register and never crossed to the broker, so the person at the
terminal read the words the program's author had written and nothing about the
thing they were signing off:

```console
$ sic attach 089a1f45 --value '{"action": "rm -rf /"}'
waiting: [deploying] deploy this?
answer with:  sic attach 089a1f45 --value <VALUE>
```

`docs/design/checking.md` §2a found that by running the program rather than by
reading this document, and named what was left: **accountability rather than
verification**. Somebody was answerable; nobody had looked. The value crosses
now, so the second half is there too:

```console
$ sic attach 089a1f45 --value '{"action": "rm -rf /"}'
waiting: [deploying] deploy this?
  approving: {"action":"rm -rf /"}
answer with:  sic attach 089a1f45 --value <VALUE>
```

Three things the sentence still does not claim, and each of them matters more
than the one it does:

- **Not that they read it.** Nothing can establish that, and a type that implied
  it would be back to claiming more than its mechanism. What is established is
  that it was in front of them, in the same output as the question, at the
  moment they answered.
- **Not that the value is safe.** A person can approve a bad plan. The label
  says who is answerable for it, not that they were right - which is the whole
  reason `sic explain` prints the reason they gave.
- **Not that what they saw *is* the value.** It is the document the value came
  from and would go back to, which is the closest thing to identity a rendering
  can be, and the next section is about why it is that and not something more
  comfortable to read.

### Why `choose` settled this and `approve` did not

`decisions.md` §1 sends `choose`'s alternatives to whoever answers, numbered,
because "whoever answers has to be able to read them without the source in front
of them". The same document never argued about `approve` either way, and the
asymmetry survived on a mechanical fact rather than on a decision: **a
capability's signature is a fixed list of types.** `human.choose` could take a
`List<String>`, because its alternatives are strings by construction.
`human.approve` could not take a `T`, because there is no such parameter type -
`CapValue` is flat, and `sic-core` refuses a general nested value twice, with
reasons, in the comments on `List` and `Exit`. A record cannot cross as a
record.

So it crosses as text, and the rest of this section is the three decisions that
follow: how the text is made, how much of it there is, and what it looks like.

### The instruction

`approve` lowers to three instructions and a branch rather than two:

```text
approve(q, v)  ->  TO_JSON  text, t<T>, v
                   CALL_CAP human.approve(q, text) -> Bool
                   MOVE     the value through
                   BRANCH   on the answer; FAIL if it is no
```

`TO_JSON` is opcode 34, and it is the inverse of `FROM_JSON`: it reads the type
section for the field names and writes out the document the value came from.
Two things about it are deliberate.

**No syntax produces it.** There is no `to_json` builtin, and adding one is not
a small extension of this - it is the thing §2a's test was written to refuse.
*Can the result be used where the labelled value could not?* A rendered
`LLM<Plan>` is a plain `String`: it can be written to a file, passed to `exec`,
and read back into a `Plan`. That is not a fact about a value in the way a
length is; it is the value, laundered. So the instruction exists in the
bytecode and is reachable only from `approve`'s lowering, where the text it
makes goes to exactly one place - the argument of the capability that asks a
person - and no program can name it.

This is safe for the reason §4 gives about erasure generally: the rule being
enforced is "this program may not be written", and bytecode that was not
compiled from a program which type-checked is the verifier's business rather
than the label's.

**It is paid for by the byte.** `TO_JSON` charges fuel per byte of the document
it writes, exactly as `CONCAT` does and against the same budget, because it is
the second instruction that allocates without a capability having been called.
That is also the answer to the next question.

### How much of it: all of it

Three candidates, and only one of them leaves the sentence at the top of this
section true.

| shown | checkable | readable | what a person does with it |
|---|---|---|---|
| a digest | yes | no | nothing they could not do with a coin |
| the first N characters | no | in part | reads a value that may not be the value |
| the whole document | yes | yes | reads it |

A digest is the tempting one, because it is small and it is exact, and it fails
on what a person is *for*. Nobody at a terminal hashes a plan by hand. The
digest is checkable by a machine that already has the value, and a machine that
has the value did not need to ask anybody. It would make `HumanApproved<T>` mean
*a person saw a number* - which is a shorter distance from the old meaning than
it looks.

A truncation is the worst of the three, and the reason is worth being exact
about: it is the only one that can **mislead**. A digest tells the person
plainly that they are not being shown the value. A cut diff does not - every
line they read is real, and nothing on the screen says whether the decision is
in the part that was cut. "Neither checkable nor readable" understates it; it is
readable and wrong.

So the whole document crosses. The case that has to be answered is the large
one - a model's answer that is a diff, a log, a document - and it has two halves
that are usually run together.

**Is it too big to send?** No, and the bound is one the run already has rather
than a number invented for a prompt. `TO_JSON` charges by the byte against a
default budget of ten million: a hundred-kilobyte diff costs one percent of it,
a megabyte ten percent, and a program that tries to approve a gigabyte runs out
of fuel and fails - which is the right outcome, because that is also a program
that was about to put a gigabyte in front of a person. Nothing new had to be
decided to get that bound, and no limit had to be picked, which is why this is
the version of the rule that is worth having. The value is in the checkpoint
already and, for a recorded run, in `responses.jsonl`; the boundary carries
nothing this run had not committed to disk.

**Is it too big to read?** That is a real question and it is a different one. A
diff is what has to be read before it is approved, and refusing to send it does
not make it shorter - it makes the approval uninformed. How a large value should
be *displayed* - a pager, a first screenful with the rest a command away - is a
question about a terminal, and #74 left it out on purpose. What is settled here
is that the value is there to be displayed.

### JSON, one line, escaped

The rendering is the document `FROM_JSON` would parse back: compact, escaped, on
one line. That is not a decision about taste.

**A prompt is line-oriented, and so is `sic runs --waiting`**, which prints one
run per row with the question last "because it is the only field that can
contain spaces". A rendering that let a string carry a raw newline into the
output would let a model's answer write a line of its own: a second row in that
table, or a plausible second question above the real one, in text a person is
reading in order to decide something. **Escaping is what stops a value from
forging the frame around it**, and the frame is the part the person is trusting.
It is the same argument `decisions.md` §1 makes for `choose` answering with an
index - what a person is shown must not be something the answer can rewrite -
one layer further down.

It costs readability, and it costs it exactly where the pressure is: a patch
arrives as one long line with `\n` in it. That is the honest trade, recorded
here rather than smoothed over. Unescaping for display is a rendering decision,
it is the one #74 put out of scope, and whoever makes it has to answer the
paragraph above rather than skip past it.

### What `sic explain` prints

Nothing in `sic explain` changed, and that is the result rather than an
omission.

`decisions.md` §6 already records the question beside the answer, in
`responses.jsonl`, because "an index on its own says nothing six months later".
The question now carries the value, so the record does:

```text
  asked a person:
    [deploying] deploy this?
      approving: {"action":"rm -rf /"}
    answered true
    because the build is green
```

A run where the person was shown the value and one where they were not do not
read the same, and the difference is a fact in the record rather than a flag
somebody remembered to set. `--interactive` inherits it for the same reason: it
prints the question the broker produced, and the value is in the question.

The journal moved too, without being asked. It digests a call's arguments, so
two runs that approved different values no longer have the same `human.approve`
request digest - which is the property `decisions.md` §2 claims for `choose`'s
alternatives, now true of the thing an approval is actually about.

### A capability called directly is unchanged

`human.approve`'s second parameter is an optional tail, the way
`process.exec`'s argument vector is. A program that calls the capability itself:

```sic
let ok = human.approve("go ahead?");
```

compiles as it did and shows what it showed. The omitted argument becomes an
empty string - the rule `arguments.md` gives - and the broker adds no line when
it is empty. The two cases cannot run together, because **no rendered value is
empty**: JSON has no empty document, and the shortest one is two bytes.

That is deliberate rather than a convenience. A program calling `human.approve`
with its own question is asking about whatever it likes and has no `T` in the
call at all. `approve` is the one that claims to be about a specific value, and
it is the one that has to produce it.

The third caller is an agent, through `sic mcp`, and it does not get the
shortcut: the tool `sic mcp` offers for `human.approve` names both fields, so an
agent asking a person to approve something says what it is asking about or sends
an empty string on purpose. The route repeats the signature rather than sharing
it - `sic-broker` may not depend on `sic-types` - and "a capability that grows a
parameter has to be added in both places" is the cost of that boundary, which
its own comment says out loud.

### A task cannot be approved

```text
error[E0376]: `Task<Plan>` cannot be shown to whoever is asked
```

`approve` takes a value of any type, and one type is not a value anybody outside
this run could be shown: a task is a computation in this run and means nothing
outside it, which is the same reason `to_cap_value` refuses to hand one to the
broker. Before the value crossed, `approve(q, t)` compiled and meant nothing;
now it is refused where it is written, and the note says what to do instead -
await the task and approve what it produced. Lists and record fields are checked
through, because a list of tasks is no more showable than a task.

### What is deliberately not here

- **A `to_json` builtin.** The instruction is not a feature. §2a's test refuses
  it, and the argument is above.
- **Rendering a large value nicely.** A pager, a summary, a diff that looks like
  a diff. #74 says the value should be in front of the person; what it should
  look like when it is a thousand lines is a separate piece of work, and the
  escaping argument above is what it has to answer.
- **A digest beside the value.** Two checkable facts where one is checkable by
  hand and the other by nobody. The journal already digests the request.
- **`sic plan` saying whether an approval will show a value.** It could be read
  out of the bytecode - the `TO_JSON` before the call - but tying that
  instruction to that call site is inference, and `plan.md` §3's rule is that a
  guess dressed as a fact is the one thing a plan must not be. A plan says a
  person will be asked; what they will be shown is in the record afterwards.
- **Showing the value anywhere else.** `sic explain` prints the question a
  person was asked, which now contains it. Nothing prints a value that nobody
  was shown.
- **Making `approve` cheaper.** An approval that covers a digest for a week, a
  batch of values in one question, a session. `checking.md` §7 has been holding
  that and it is still a different issue: cheaper is about how often a person is
  asked, and this was about what they are shown when they are.

---

## 4. Trust is erased

None of this exists at run time. `LLM<Plan>` and `Plan` are the same bytes in
the same register; the bytecode's type section does not mention trust, the
verifier does not track it, and the VM has never heard of it.

That is deliberate, and it is the reason this is a small change rather than a
large one:

- The rule being enforced is "this program may not be written", which is a
  compile-time claim. Checking it again at run time would not make it truer.
- A value's provenance cannot be forged at run time because there is nothing to
  forge - no tag to set, no field to overwrite.
- Bytecode that was compiled from a program which type-checked is safe by
  construction. Bytecode that was not is refused by the verifier for the
  ordinary reasons.

The cost is that a trust rule cannot be enforced on bytecode compiled elsewhere
by a compiler that did not check it. That is the same trust boundary the source
language already has, and it is what `sic plan` and the manifest are for.

---

## 5. `Secret<T>` is not here yet

Section 19 lists `Secret<T>`, and section 27 says a secret must never reach
telemetry. Both matter, and neither is in this change, because **nothing
produces a secret yet**: there is no capability that reads a credential.

Adding the type now would mean adding a type nobody can construct, which is the
kind of speculative structure this project is arranged to avoid. The protection
section 27 asks for is already there by a different route: the journal records
digests, never values, so no value reaches telemetry regardless of what it is.

When a credential capability arrives, `Secret<T>` comes with it, and it is the
one trust type that cannot be erased - a runtime check is the point of it.

The same goes for `Verified<T>` and `UserProvided<T>`: each needs something
that produces it before the type is worth anything.

`HumanChosen<T>` arrived the same way `Observed<T>` did - with something that
produces it. `choose` asks a person which of the program's own alternatives, so
it carries no restriction: unlike a model's answer or a program's output, its
text was written by whoever wrote the program. See `docs/design/decisions.md`
§4.

`Observed<T>` is no longer one of them. `process.capture` produces it - what a
program printed was not verified, not approved, and not written by whoever wrote
the program that read it - and it carries `LLM<T>`'s rule, because the sentence
covers both: *a value nobody signed off cannot decide what gets changed or run*.
See `docs/design/output.md` §5.

---

## 6. Units of work

| # | Unit | Done when |
|---|------|-----------|
| 1 | `LLM<T>` on agent output, and the type printing right | an agent's result is `LLM<Diagnosis>` |
| 2 | Trust types in signatures and annotations | `fn deploy(p: HumanApproved<Plan>)` parses and checks |
| 3 | Provenance through field access | `LLM<Diagnosis>.cause` is `LLM<String>` |
| 4 | `approve`, lowered to a capability call and a check | refusing an approval fails the run |
| 5 | The rule: no `LLM` into a write or exec capability | the specification's `deploy` example is a compile error |
| 6 | Erasure | the bytecode's type section never mentions trust |
| 7 | §2a: what a trusted value may decide | `len` stripping the label is a test rather than a sentence in a checker |
| 8 | §3: the person is shown the value | a run where they were shown it and one where they were not do not read the same |
