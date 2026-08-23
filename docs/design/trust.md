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
a file whose name it suggested is not, which is why the rule is about the
capability's kind rather than about the value.

**A trust type is not its inner type.** `LLM<Int> + 1` does not compile, and
neither does passing `LLM<Plan>` where `Plan` is expected. Arithmetic on a value
whose provenance matters is exactly where provenance gets lost.

**Reading a field keeps the provenance.** `LLM<Diagnosis>.cause` is
`LLM<String>`, not `String`. A field of a model's answer is still the model's
answer, and the alternative - losing the label at the first field access - would
make the whole thing decorative.

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
