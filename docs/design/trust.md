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

An operator that answers a value of its operands' own kind can hand the operand
straight back. The rule refuses that, everywhere, so that:

> **Once attached, a label never comes off a value.**

Not through `approve` either, and this is worth being precise about because the
first draft of this section got it wrong: `approve` turns `LLM<T>` into
`HumanApproved<T>`, which is still a label. E0371 refuses it in the operand
positions it refuses any other - `let n: Int = approved.severity + 1;` does not
compile - and what changed is only which capabilities the value may now reach
(§2, §3).

What a person's approval buys is reach, not arithmetic - and that is right,
because the person approved *using* the value, not every number a program might
derive from it.

The words "a value of its operands' own kind" are load-bearing, and were added
by #73; before it the rule covered every operator, which turned out to be a rule
about syntax rather than about laundering. "The asymmetry, answered" below is
where that is argued and where the rule is stated in full.

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
operator - which E0371 refused when this was written, and does not any more.
That contradiction became #73, and "The asymmetry, answered" is where it was
taken. So E0371's refusal is about what an expression **hands back**, not about
what a program can find out. It was already only that: `len(x) == 1` finds out a
great deal. This makes it plain, and the honest summary of the trust system's
guarantee is *a labelled value is opaque to computation* with "except that a
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

`+` on two strings answers a value of its operands' own kind, which is the shape
E0371 is for - and it is not refused. The reason is the sentence above rather
than an exception to it.

Read what E0371 is for again: *an operator that answers a value of its operands'
own kind can hand the operand straight back*, and the rule refuses that so that
once attached, a label never comes off a value. Joining does not do that. It
answers a **labelled** value:

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

### The asymmetry, answered

The subsection this replaces said that E0371 was not narrowed, and that if the
refusal turned out to be in the way of a real program, the issue that argued for
it had to answer this section. #73 was that issue. It is answered here, and the
answer is that the refusal was narrowed.

The two spellings it was written about:

```sic
let head = git.rev_parse("HEAD");        // Observed<String>

if head == "deadbeef" { … }              // was error[E0371]
if starts_with(head, "deadbeef")
    && len(head) == 8 { … }              // compiled
```

E0371 refused an **operand**. A builtin takes **arguments**, and no rule refused
those, so a program that wanted the answer wrote the other spelling and got it.
That is a rule about syntax, which is the same failure #72 was: two spellings of
one act, one checked and one not.

#### Which way the two spellings were made to agree

Refusing both was the other direction, and it is not available. Every question
`contains` and `starts_with` exist to answer is about what a capability
reported, so they would take `Observed<String>` or nothing; an `Observed<Bool>`
is not an `if` condition (E0301, checked - `expect_type(Bool, …)`) and `!` does
not apply to one either (E0371). A refusal that covered their arguments would
refuse every program they were added for. That was verified rather than taken
from #68: both errors were reproduced before this was written.

So the direction is to allow both, and the question is what the rule becomes.

#### The rule

> **A labelled operand is refused when the operator answers a value of the
> operands' own kind, because then the result may be the operand. An operator
> whose result cannot be one of its operands is asking a question, and a
> question may be asked about a labelled value.**

In v0.1 that is exactly the comparisons over `Int` and `String` - `==` and `!=`
on both, `<`, `<=`, `>` and `>=` on `Int`, which is all `<` applies to until
somebody argues for a collation. They answer `Bool`, and no `Bool` is ever one
of the values they were given.

Everything else answers its operands' type, and there the rule has two limbs
rather than one - which is the part worth reading twice, because it is what
makes `+` on two strings the same decision rather than a hole in this one:

| operator | operands | result | what happens |
|---|---|---|---|
| `+ - * / %` | `Int` | `Int` | refused (E0371) |
| `-` (unary) | `Int` | `Int` | refused |
| `! && \|\|` | `Bool` | `Bool` | refused |
| `== !=` | `Bool` | `Bool` | refused |
| `+` | `String` | `String` | **carries the label** |
| `== != < <= > >=` | `Int`, `String` | `Bool` | allowed, answers plain |

An operator whose result has its operands' type either refuses a label or
carries it onward. Joining carries it, because a joined string is still the
bytes it was made of and `process.exec` refuses it for the reason it refuses
what it was joined from. Arithmetic refuses it, and could have carried it
instead - `LLM<Int> + 1` being `LLM<Int>` would be defensible - but nothing has
asked for that, and the shape §2 was written about is a program that wants the
plain number. The issue that wants labelled arithmetic is where that gets
argued; it is not decided here.

#### `x == true` is the value, not a question about it

The one comparison that stays refused, and it is the rule rather than an
exception to it. `a == true` answers the `Bool` it was given; `a != false` does
too. A `Bool` compared with a literal *is* the operand, which is `d.severity +
0` spelled with a different operator.

This is not an information-flow argument, and it must not be read as one - a
budget on how much a program can learn is refused below and the refusal has not
moved. `x == "deadbeef"` also tells a program something, and may be asked again
with a different literal until the string is reconstructed. The difference is
not how much comes back; it is that one expression **hands back the operand
itself** and the other hands back an answer that is not any value the label was
on.

The consequence is that a labelled `Bool` is still not a condition, and there is
no back door to making it one. Whether it should become one is a separate
decision that touches `if`, `while`, `!` and the connectives at once.

#### Two labels compared, and two labels joined

`d.cause + out` is E0375 - there is no name for where the result came from.
`d.cause == out` compiles.

That is the same rule from both sides rather than an inconsistency. A join
answers a value, and a value in this document has one origin; picking a winner
between "a model said it" and "a program printed it" would need an order between
the labels that nothing here has. A comparison answers a `Bool`, which has no
origin to name, so there is nothing to invent.

#### What E0371 still refuses, and whether it is worth refusing

#73 asked for a program the rule protects that `contains` and `len` cannot
already express. There is one, and it is the example §2 opens with:

```sic
let clean: Int = d.severity + 0;
```

No builtin expresses that, because it is not a question about the value - it *is*
the value, and `contains`, `starts_with` and `len` each answer something that is
not. The same goes for `!a` and `a && b` on a labelled `Bool`. So the answer is
that the rule still refuses the shape it was written for, and had stopped
refusing anything else.

The narrowing is worth being exact about, because it is larger than it sounds:

- On a labelled **`Int`**, E0371 bites on arithmetic and nothing else.
- On a labelled **`Bool`**, it bites on everything, because every operator that
  applies to a `Bool` answers one.
- On a labelled **`String`**, it now refuses nothing a program would write.
  `==` and `!=` are allowed, `+` carries the label, `<` is not an operator on
  `String` at all, and `-` and `!` do not apply. The refusal that remains there
  is a refusal of expressions that would not have type-checked anyway.

That last line is the honest measure of what #68 and #73 found together: for
strings, the operand rule had already stopped doing work, and only the error
message said otherwise.

#### The note that pointed at `approve`

It was wrong, and the narrowing is not what made it wrong.

```text
error[E0371]: LLM<Int> cannot be used as an operand
  = note: `approve(question, value)` turns a model's answer into one a person
          signed off
```

A program that took the advice wrote `approve("use this?", d)` and met
`error[E0371]: HumanApproved<Int> cannot be used as an operand` on the next
line. This section already said so - "E0371 refuses it just the same" - and the
diagnostic did not. It was the only place a program was told a way through, and
the way through was a dead end.

So the note is gone from E0371 and stays on E0372, which is the rule `approve`
actually answers: what a person's approval buys is reach. In its place E0371
says what a labelled value may do - be compared, be asked `len`, `contains` and
`starts_with` - and, for the `Bool` case, why comparing it is the value again.
And the shape people write first, `head == "deadbeef"`, no longer produces a
diagnostic at all, which is the better outcome than a better note.

#### One rule over four labels

`LLM<T>`, `Observed<T>`, `HumanApproved<T>` and `HumanChosen<T>` are still
covered by one rule, and a person's choice is still not a model's answer. Those
two facts do not conflict, because they are answered by different rules:

- **E0372 is where the labels differ.** `LLM` and `Observed` cannot reach a
  capability that changes something; `HumanApproved` and `HumanChosen` can. That
  is the whole of what vouching buys, and it is a rule about reach.
- **E0371 is where they do not.** Laundering is the same act whoever vouched
  for the value. `approved.severity + 0` hands back a plain `Int` exactly as
  `d.severity + 0` does, and the person approved *using* the plan, not every
  number derived from it.

A person's choice is where the old refusal was hardest to defend, and it is the
clearest gain here:

```sic
let picked = choose("deploy or roll back?", ["deploy", "rollback"]);
if picked == "deploy" { … }             // was error[E0371]
```

The program wrote both options itself. §5 already says `choose` "carries no
restriction: unlike a model's answer or a program's output, its text was written
by whoever wrote the program" - and the program could not ask which of its own
two strings came back. That was the operand rule reaching a value it had no
argument about, which is what happens when a rule is about syntax.

### What is checked

| claim | how |
|---|---|
| an operator that answers its operands' kind refuses a label, whoever vouched for it | E0371, tested on `LLM<Int>` arithmetic and on `HumanApproved<Int>` arithmetic, which is also what says `approve` is not the way out of it |
| a comparison may ask a labelled value, whoever vouched for it | one test over all four labels, because a rule that held for three of them would be about which capability produced the value |
| the two spellings of asking a string agree | `head == "deadbeef"` and `starts_with(head, …) && len(head) == 8` are pinned in one test, so neither can be changed alone |
| a labelled `Bool` compared with a literal is refused | a test, because it is the one comparison that is not a question and the reason is easy to lose |
| two labels may be compared though they may not be joined | tested beside E0375, so the difference stays a decision about naming a result rather than a coincidence |
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
- **Narrowing E0371 further.** It now refuses arithmetic, `-`, `!`, `&&`, `||`
  and equality on a labelled `Bool`. Making arithmetic carry the label instead
  of refusing it - `LLM<Int> + 1` being `LLM<Int>`, the way `+` on two strings
  works - is defensible and is not done, because no program has asked.
- **A labelled condition.** `if a` where `a` is `LLM<Bool>` is E0301, and
  `a == true` is E0371 for the reason above. Making a labelled `Bool` decide a
  branch is a decision about `if`, `while`, `!` and the connectives at once, and
  it is not taken by narrowing an operand rule.
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
