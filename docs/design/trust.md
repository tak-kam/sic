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
argument of a `write` or `exec` capability may carry `LLM`. Reading and invoking
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
let said = llm.invoke("...");
if len(said) > 5 { … }          // compiles, today
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
| a labelled value does not reach a changing capability | E0372 |
| reading a field keeps the label | §2, tested |
| `len` strips it, and that is on purpose | a test that says so, so that changing it is a decision rather than a regression |

The last row matters more than it looks. Before this section, `len`'s behaviour
was one sentence in a checker and nothing in a test: an edit that made it
propagate would have looked like a fix.

### What is deliberately not here

- **A rule about branch conditions.** The manifest is the unit of approval.
- **`Observed<T>`.** Text a program printed carries a different argument
  (`docs/design/output.md`), and it reaches `len` the same way for the same
  reason - `len(git.status()) > 0` is the question that capability exists to
  answer.
- **Narrowing E0371.** Above.
- **A channel budget.** Counting how many bits a model can push through `len`
  is information-flow analysis, and this language is not going to have one.
---

## 3. `approve`

```text
approve(question: String, value: LLM<T>) -> HumanApproved<T>
```

It asks `human.approve` with the question, and if the answer is no, the run
fails. There is no third outcome to return: without an option type, "approved or
not" would have to be a `Bool` beside the value, and nothing would stop the
program from ignoring it.

`approve` needs the `human.approve` capability granted, like any other path to
an effect. It is the only way to produce a `HumanApproved<T>`, which is what
makes the type mean anything.

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
