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

In v0.1 that is exactly the comparisons over `Int`, `String` and `Float` - `==`
and `!=` on the first two, `<`, `<=`, `>` and `>=` on `Int` and on `Float`.
`<` does not apply to a `String` until somebody argues for a collation, and
`==` does not apply to a `Float` at all (`v0.1.md` §4). They answer `Bool`, and
no `Bool` is ever one of the values they were given.

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
| `== !=` | `Int`, `String` | `Bool` | allowed, answers plain |
| `< <= > >=` | `Int`, `Float` | `Bool` | allowed, answers plain |

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

## 5a. Erasure was right, and it cost the artifact its strongest sentence

Unit 6 below says the bytecode's type section never mentions trust, and that is
right for the reason it was argued: a discipline the checker enforces does not
need a label the VM carries on every instruction. Nothing here proposes putting
one back.

What it cost is a different thing, and #89 is it. **E0372 refuses source.** The
artifact everything downstream trusts is the `.sicb`, and three commands read
one without ever seeing a `.sic`:

| command | reads | saw trust |
|---|---|---|
| `sic plan` | `.sicb` | no |
| `sic verify` | `.sicb` | no |
| `sic run` on a `.sicb` | `.sicb` | no |

So "this program cannot pass a model's answer to something that changes state"
was true of every program **this compiler** compiled, and was not a property of
the file. A `.sicb` from a compiler with three lines removed printed a clean
plan. And a person approving a plan is not, in the end, worried about the
manifest - they are worried about the *flow*, and the flow was the part thrown
away.

### `approve` had to leave a mark

The flow is in the instructions and can be read back out of them. The laundering
point was not. `approve` lowered to:

```text
TO_JSON     r8, t5, r0
CALL_CAP    r9, c1, r15      ; human.approve
MOVE        r7, r0
JUMP_IF     r9, +2
```

and a `MOVE` is what every assignment in the language lowers to. A reader could
have recognised the *shape* - a move guarded by a branch on a `human.approve`
whose `TO_JSON` named the same register - and that is a reader trusting the
compiler's habits rather than reading a fact. The habits are not a contract; the
next person to touch the lowering owes them nothing.

So the compiler writes the fact down. `APPROVE` is a new opcode that does
exactly what `MOVE` does, and it is always emitted, even where the register
allocator picked one register and a `MOVE` would have been dropped - the
instruction is not there to move anything.

`VERSION_MINOR` does not move. A new opcode is not a section-layout change: an
old reader meets an instruction it does not know and says so, which is the case
the number is not for.

### What `sic plan` says now

Every place a model's answer reaches a capability that writes or runs, and
whether a person agreed on every path that gets there:

```text
A model's answer reaches:
  fs.write in main at 14:5  (a person agreed)
```

and, for bytecode this compiler would not have produced:

```text
A model's answer reaches:
  fs.write in main at 14:5  ** nobody was asked **
```

The weaker claim is the one that is marked, which is the other way round from
`(not pinned)` and from `(declared fields only)` elsewhere in the plan. The
reason outweighs consistency: an unapproved flow is *the finding*, and a reader
scanning the list must not have to notice a missing word.

Since #87 the same fact is in `sic plan --json`, so this is a property a rule
can check rather than a sentence a person can read.

### It over-reports, in one specific way

The analysis is flow-sensitive within a function and context-insensitive across
them. A function's parameters take the join of every call site, so a helper
called once with a model's answer and once with a literal is analysed as if both
were the model's.

That is the safe direction and the only one worth having. A plan that missed a
flow would be a false assurance about the one question this language exists to
answer, and a person reading a plan with no such section has to be able to
believe there is nothing to report.

Flow-*sensitivity* is not a refinement but the thing working at all: the
compiler reuses one register window for every call's arguments, so a register
that held a prompt later holds a path. A taint per register per function
reported every program as carrying its answer into everything.

### What is still not a property of the file

The verifier does not refuse an unapproved flow; it reports one through the
plan. Refusing is the strong version and is where this belongs, and it is a
second decision rather than a smaller one: a verifier that refused a program the
type checker accepted would be a release-stopping bug, and the two analyses
would have to agree exactly. Doing the plan first and the refusal second is a
sequencing, not a hedge - the analysis exists now, and whether it is exact
enough to refuse with is a question that can be asked of code rather than of an
argument.

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
| 9 | §5a: the flow is in the file | `sic plan` says where a model's answer goes, and whether anybody agreed |
