# Checking a model's answer without a person

`approve` is the only thing that takes a label off a value, and it asks a person
every time. A workflow that runs on every push needs a person on every push, and
the person becomes what the runtime is waiting for.

Issue #71 asks for a second discharge beside it - one whose argument is evidence
rather than a person:

```sic
let checked = because(r.code == 0, p);
```

**This document decides against it, and says exactly what would have to exist
first.** Not because an automatic discharge is unsound in principle: `choose`
already shows that a discharge can be honest by construction, and one shape of
automatic discharge is in the language today without anybody having named it
(§1). The reason is narrower and it is a fact about the capability table rather
than a matter of taste.

> **Nothing in sic can look at a labelled value and answer a fact about it.**
> The only capability that can be handed one in the position that matters is
> `llm.invoke`, which is self-certification, and #71 rules that out. The one
> thing that does look at the value is a person, and `approve` is the door they
> stand behind (§2a).

A declassifier needs evidence to be given. There is none to give, so `because`
would be a door with nothing behind it, and every program that could be written
with it would be vacuous, self-certifying, or vouching for one thing by
measuring another. §5 says what capability would change that, and the order it
has to arrive in.

---

## 1. The premise is not quite true: a label has three exits already

#71 opens with "the label has exactly one discharge, and it is the most
expensive one available." The first half is not true. Three things in the
language today take a labelled value in and hand back a plain one, and all three
compile:

| exit | what comes out | where it is decided | documented |
|---|---|---|---|
| `len(v)` | `Int` | `check_len` calls `untrusted` on its argument | `trust.md` §2a, and a test |
| `xs[i]`, `i` labelled | the element, unlabelled | `check_index` calls `untrusted` on the index | nowhere |
| `fs.read(p)`, `p` labelled | `String`, unlabelled | the capability table's return type | nowhere |

`len` is the known one and its argument is made in `trust.md` §2a. The other two
were found by compiling programs rather than by reading documents, and the
second of them is the more interesting result in this document.

### `xs[i]` is already the discharge #71 is asking for

```sic
type Pick { index: Int }

agent pick { input: String, output: Pick }

fn main() -> Int {
    let options = ["restart", "rollback"];
    let p = pick("which?");
    let chosen = options[p.index];
    return process.exec("/usr/bin/true", [chosen]);
}
```

This compiles, and `sic plan` prints the `EXEC`. No person is anywhere in it.
`p.index` is genuinely `LLM<Int>` - `return p.index + 1;` in the same program is
E0371 - and `check_index` strips the label off it in one line.

It is not a leak, and the argument for that is already written down in
`decisions.md` §1, about `choose`:

> The capability returns **which** option, and the VM reads the value out of the
> list the program itself built. ... `HumanChosen<String>` says a person picked
> one of these, and it is true by construction.

Every word of that survives replacing the person with a model. The string that
reaches `process.exec` cannot contain anything the source does not contain; the
model chose among the program's own alternatives and never handed the program a
value. What #71 calls **steering** is exactly this, and `trust.md` §2a settled it
in general: a branch is not an effect, the manifest is the unit of approval, and
this is a branch with the arms written as data.

So there is an automatic discharge, it is sound, and **the reason it is sound is
one `because` cannot borrow**: the value reaching the effect was never the
model's. `because` is asking for the opposite - the value *is* the model's, and
something else vouches for it. The existing one does not generalise to the case
that hurts, which is the useful thing it tells us.

What it is missing is everything #71's four questions ask for. It is not named,
so a reader of a program cannot see that a discharge happened; there is no test,
so an edit that made the label propagate would look like a fix; and `sic plan`
prints the effect without saying the model picked which one. §6.

### `fs.read` is a hole, and a separate one

```sic
let g = guess("which file?");        // LLM<Guess>
let evidence = fs.read(g.path);      // String, unlabelled
if evidence == "ok" {
    fs.write("./out.txt", evidence); // compiles
}
```

`trust.md` §2 permits this - "so is reading a file whose name it suggested" -
and says nothing about what comes back. What comes back is a plain `String`,
because `fs.read`'s return type in the capability table is `Types::STR`. So a
model chooses which of the machine's files is written to `./out.txt`, and no
label is in the way at any point.

This is not this document's to fix and §6 says so. It matters here for one
reason: it is the counterexample to the only vacuity rule #71 could propose
(§4.1).

---

## 2. Why `because` would have no evidence to be given

E0372 refuses a labelled value as an argument to a capability whose kind is
`Write` or `Exec`. That leaves `Read` and `Invoke`, and there are six of them.
Ask of each what it could tell a program about a labelled value passed into it:

| capability | kind | takes a labelled value? | what it would say about it |
|---|---|---|---|
| `fs.read` | read | as a **path** | which file's contents come back - a fact about the machine, not about the value |
| `git.rev_parse` | read | as a revision | checked against a short allowlist before it reaches git; a fact about a repository |
| `git.status` | read | takes nothing | - |
| `llm.invoke` | invoke | yes, as the prompt | what a model says about a model's answer |
| `human.approve` | invoke | yes, rendered, since #74 | what a **person** says about it, which is `approve` |
| `human.choose` | invoke | the question is a `String` | this is `choose`, and the answer is an index |

Two of these six can be handed the value itself in the position that matters.
One is `llm.invoke`, which is a model reporting on a model. The other is
`human.approve`, and what it answers with is a person - which is the door this
document is about not needing. Everything else takes the value as a *name* - a
path, a revision - or as the text of a question, which is the model steering a
read rather than a read reporting on the model. So the sentence at the top of
this document is not a summary of an argument; it is the capability table read
out, and #74 changed which row it has to name rather than whether it holds.

A second discharge would be a rule about a kind of evidence that no program can
obtain. The right response to that is not to build the rule and wait.

---

## 2a. `approve` did not show the person the value either

> **Fixed by #74, after this document was written.** `approve` now renders its
> value and passes it to `human.approve` beside the question, and whoever
> answers is shown it. `trust.md` §3 is the decision and says what
> `HumanApproved<T>` claims as a result. This section is kept as it stood
> because the argument it feeds does not depend on the gap being open - it is
> sharper with it closed, since what `because` proposes to remove is now a
> person who was shown the thing. Two sentences below have been overtaken and
> are marked where they stand: what the door buys, and the second of §4.2's two
> facts.

This was the assumption most worth checking, and it is false. `approve` lowers
to `CALL_CAP human.approve(question)` - **the question only**. The value is
`Move`d into the destination register and never reaches the broker, so nothing
that answers the call has it.

A run of `examples/approval-flow.sic`, with the model's answer supplied by hand:

```console
$ sic attach 418bfc3c --value '{"action": "rm -rf /"}'
saved 536 bytes to .sic/runs/418bfc3c.../checkpoint.sicc
waiting: [deploying] deploy this?
answer with:  sic attach 418bfc3c --value <VALUE>
```

That is the whole of what the person is shown, and `sic runs --waiting` shows
the same line and no more. The value is in the checkpoint and, for a recorded
run, in the answers file - so `sic explain` prints it *afterwards* - but at the
moment somebody types `--value true`, `{"action": "rm -rf /"}` is not in front
of them.

This is not a bug to fix in passing; it is `capabilities.md`'s signature rule
meeting `trust.md`'s. A capability's parameters are a fixed list of types, which
is why `approve` had to be a builtin at all (`decisions.md` §2), and a builtin
that passed its second argument through would need a way to render an arbitrary
value as text for a human - which the language does not have, and which
`decisions.md` §7 has already refused once for `choose`'s options.

**It changes what `approve` is understood to buy.** Not "a person examined this
value", which is what a reader of `trust.md` would take `HumanApproved<T>` to
mean. What it buys is:

> a person was asked, at this point in this run, and said yes, and their answer
> and their reason are in the record.

That is **accountability**, not verification. Somebody is answerable for the run
having continued, and `sic explain` can name what they were asked and why they
agreed.

> Overtaken by #74. The value crosses now, so the person was shown it, and
> `trust.md` §3 writes the sentence this paragraph said was missing. The half
> that does not move is that nobody can establish they *read* it - which is why
> §3's sentence is about what was in front of them.

And it is the sharpest form of the argument against `because`, because a
criterion buys neither half. Nothing examined the value - §2 - and nobody is
answerable, because a condition is not a party. Replacing a discharge that
verifies nothing but records somebody with one that verifies nothing and records
nobody is not a saving; it is the removal of the only thing that was there.

It also says where the useful work is, and it is not a second door. `approve`
asking a question the program's author wrote, about a value the answerer cannot
see, is a real weakness of the door that exists - and one that a person can
close by writing a better question, which is more than can be said for anything
in §4. §6 has it as an issue.

> That issue is #74, and it closed the weakness rather than working around it:
> the value crosses, so the question no longer has to describe it.

---

## 2b. `len` is the one fact a program can compute, and building on it is wrong

`because(len(m) < 72, m)` is a real check, on the value, from outside it, and it
is relevant - a commit message that is too long is a commit message that is
wrong. It is also the *only* shape of real check available, because a length is
the only thing the language computes from a labelled value.

`trust.md` §2a wrote the test for whether a builtin may strip a label, in
advance:

> **can the result be used where the labelled value could not?** A length cannot
> be written to a file, passed to `exec`, or turned back into the answer.

A discharge is nothing but a way of using the result where the labelled value
could not. Building one on top of `len` would take the exception that survives
because it does not generalise and make it the foundation of the thing that
does. That section also names the risk of the exception becoming a precedent,
and this would be the precedent it named.

---

## 2c. The check that already runs, and deliberately does not discharge

Every `agent` validates its answer against a declared type before the program
sees it (`agents.md` §4). That is a check, performed by the runtime, on the
value, from outside the model - and it does not take the label off. The reason
is worth saying out loud because it is the same reason as the rest of this
section: **"this is a `Patch`" is not "this `Patch` is safe to apply."**

Anything `because` could be given today would be a claim of that shape with a
weaker guarantee behind it, since `from_json` at least inspects the whole value.

---

## 3. Where it bites, and where it does not

### Steering is free, and that was re-checked rather than assumed

```sic
fn main() -> Int {
    let r = process.run("/bin/sh", ["-c", "exit 0"]);
    if r.code == 0 {
        return process.exec("/usr/bin/true");
    }
    return 1;
}
```

`sic plan` accepts this and lists both call sites. `Exit.code` is a plain `Int`
on purpose - `Types::EXIT`'s comment says being usable in exactly this `if` is
why the type exists - so the whole class "decide whether the effect happens this
run" needs no discharge and no person. #71 says this and it holds.

### When the evidence exists, the value is usually not the artifact

`authority.md` gives the agent its own `Read`, `Write` and `Edit`, scoped by the
manifest, and `sic plan` prints them:

```text
  llm.invoke      [invoke]  "claude-opus-4"  (not pinned)
    the agent's Write  "./p.txt"                (its own permissions)
    the agent's Edit   "./p.txt"                (its own permissions)
```

So the ordinary way a model's work reaches the filesystem is **through the
agent's own tools, under the program's manifest** - not as a value crossing the
program's type system. And once the work is on the filesystem, evidence about it
is a `process.run` away, and acting on that evidence is the `if` above.

`workflows/ci.sic` is exactly this program and has no `approve` in it. It is
also why the residual case is narrower than it first looks.

### The residual case, stated precisely

The label is in the way when **the model's value is itself the artifact**: the
commit message, the file's content, a field of a JSON answer the program will
write or pass to a command. That case is real, `approve` is the only door, and
§2 is the reason there is nothing to put in a second one - the artifact cannot
be tested without being written somewhere, and writing it is E0372.

The loop is closed by the rule the discharge is trying to open. That is the
shape of the problem, and §5 is the way out of it.

---

## 4. #71's four questions, answered

Answering them is what makes this a decision rather than a shrug, and three of
the four answers are constraints a later implementation issue would otherwise
discover late.

### 4.1 What stops a vacuous check

The mechanically checkable rule is the one #71 proposes: **the condition has to
depend on a capability call.** It is easy to check and it does not do the job.

It does not exclude self-certification. §1's `fs.read` program satisfies it
completely: the condition depends on a capability call, the call is a read, the
read is of a file the model named, and the comparison is against a string the
model could have arranged to be there. That is self-certification with a
capability call in the middle, and no rule about *whether* a capability call is
involved can tell it from the honest case.

It does not establish relevance either, and this is the part a reader would get
wrong:

```sic
let p = propose("fix it");
let r = process.run("/usr/bin/cargo", ["test"]);
let checked = because(r.code == 0, p);
```

The tests ran. They passed. The patch was never applied, so what passed is the
tree without it. The condition is honest, depends on a capability call, is not
self-certifying, and says nothing whatever about `p`.

The gap between "depends on a capability call" and "is evidence about this
value" is the whole of the guarantee, and nothing mechanical narrows it while §2
holds. A reader who sees a discharge in a plan will assume the second and be
given the first. **That assumption is worse than the bottleneck**, for the
reason `authority.md` §2 gives about gates and boundaries: a manifest that
claims more than its mechanism is the one thing this project cannot afford.

### 4.2 What the result would be labelled

Not `HumanApproved<T>`; #71 is right that a type which lies about a person is
worse than the bottleneck. The problem with `Checked<T>` is not the name.

The two labels that may reach a capability that changes something are each true
by construction, and each carries its own argument for why:

| label | may write or exec because |
|---|---|
| `HumanApproved<T>` | a person was asked, at the point this value existed, and said yes - and §2a is what that does and does not mean |
| `HumanChosen<T>` | its text is the program's own, and a person picked which (`decisions.md` §4) |

Both are true by construction, and neither depends on what happened to be
written at the site. `Checked<T>` would be true by whatever the condition was
there, and a type cannot carry that. Two values of the same type would mean "a
validator ran on this and exited zero" and "an unrelated exit code was zero",
and §2's reach rule would have to answer for both at once. **A label whose
meaning varies per construction site is not a label**, and this is the same
objection `trust.md` §5 makes to adding a type before the thing that produces it
exists - sharpened, because here more than one thing would.

Two related facts, neither written down anywhere, and a later issue would trip
on both. **`approve` does not require its value to be labelled**: it takes any
value and answers `HumanApproved<T>` over it. And, from §2a, it does not show
the person the value. Put together, `approve` is not a "discharge" in the sense
#71 uses at all - it is a constructor for a type that means *somebody is
answerable*, and the value it is applied to is the program's choice of what that
person's yes will be taken to have covered.

> The second fact was the issue #74 became, and it no longer holds: the value is
> shown. The first still does, and so does the sentence they were put together
> to make - the program still chooses *which* value the yes covers, and that is
> the part a reader of a program has to check. What changed is that the person
> now sees which one it was.

### 4.3 What `sic plan` would print

The mechanical constraint first, because it decides the rest.

`approve` is in the plan because it lowers to a capability call. `sic-ir`'s
`approve` emits a `TO_JSON` of the value, `CALL_CAP human.approve(question,
text)`, a `Move` of the value, and a branch to `Fail`; the value itself is
untouched, because trust is erased before the bytecode (`trust.md` §4) - the
rendering is a second string beside it, not a change to it. A `because` that is
only a type-level operation would
lower to a `Move` and **nothing else**, and `sic plan` reads bytecode. It would
appear nowhere.

So the criterion has to be put into the bytecode deliberately, as a constant.
That is possible - there is a precedent, and it is a close one. `sic-plan` reads
`alternatives` for a `human.choose` back out of the `MAKE_LIST` that built the
argument, because "how many choices somebody will be asked to make between is
the thing a plan is being read for." A compiler could intern the condition's
source text the same way; the debug section holds `(pc, file, line, col)` and no
text, and `sic plan` runs on a `.sicb` with no source to consult, so interning is
the only route.

And that is the point at which the design fails rather than the point at which
it works:

```text
  main
    1. INVOKE   llm.invoke   "claude-opus-4"
    2. VERIFY   Patch
    3. CHECKED  "r.code == 0"          ; 14:19
    4. WRITE    fs.write     "./p.txt"
```

Line 3 reads as "the tests passed on this patch". It means "some `Exit` in scope
had code 0". `plan.md` §3 refuses to sum `retry` counts into a maximum because
"a guess dressed as a fact is the one thing a plan must not be", and this is
that sentence about a different guess - worse, because the guess is being made
by the reader on the plan's invitation.

`--graph` is worse again, for the reason `plan.md` §3a already gives: **an arrow
is much harder to qualify than a sentence.** The graph's nodes are functions and
grants; a criterion is neither, so the only place for it is a label on the edge
into the effect, which is the strongest possible form of the claim drawn in the
output that can carry a caption once for the whole picture and not per edge.

### 4.4 What the journal would record

Measured. A recorded run of `examples/approval.sic`, answered with
`sic attach --value true --because "the build is green"`:

```text
    task main
        call human.approve
  waiting for human.approve
  resumed with human.approve
          human.approve answered sha256:7cc15f3c
        call process.exec
          process.exec answered sha256:35966776

  asked a person:
    [deploy to production] deploy build 42?
    answered true
    because the build is green
```

Two facts, and both were assumptions worth checking:

**The journal's line is `call human.approve`, and it is there because approving
is a capability call.** Nothing in the journal knows what `approve` is; it sees
the effect.

**"asked a person" is not from the journal at all.** `sic explain` reads it from
the run's answers file, and a line is a person's answer exactly when it carries
an `asked` field - which `decisions.md` §6 established, along with why a reason
lives there rather than in the journal: the journal records digests, never
values.

A criterion has neither. It performs no capability call, so the journal has
nothing to record; nobody answers it, so there is no line in the answers file to
carry it. Making a criterion-passed run distinguishable from a person-approved
one therefore needs **a new journal event and a new kind of line in the run's
values file**, and the digests-never-values rule forces the split the same way it
forced `--because`. That is buildable and it is written here so a later issue
does not find it late.

---

## 5. What would make the answer yes

One thing, and it is not a language feature:

> **A capability that takes a labelled value and answers a fact about it,
> without changing anything a later step can read.**

With one of those, the vacuity question in §4.1 stops being "is this condition
relevant?" - unanswerable - and becomes "was this value an input to the call
whose result the condition tests?", which is a data-dependency question a
checker can decide and a plan can print without inviting a reader to supply the
missing half. The evidence would be about the value by construction, the same
way `choose`'s honesty is by construction, and that is the only kind of honesty
this project has ever accepted for a trust type.

Three shapes would qualify. None is designed here, and each is its own argument:

- **A scratch write nothing downstream reads** - apply the patch to a copy, run
  the test suite in it. The write is real; what would make it safe to hand a
  labelled value is that the result is unreachable, which is a claim about the
  grant rather than about the type, in the register of `repeatable` and
  `delegable`.
- **A validator pinned by digest** - a program the grant names by SHA-256, given
  the value, answering an exit code. `capabilities.md` §7 already hashes on every
  call, which is the property that makes the answer mean something.
- **`git apply --check`** - the smallest of the three, already the shape
  `git.md` argues earns a capability: something the broker runs with the
  environment it controls, which a `process.run` grant could not say.

**The order is the capability, then the discharge**, and `trust.md` §5 is the
precedent for that being a decision rather than a delay: `Secret<T>` is not in
the language because nothing produces a secret yet, and adding it would be
adding a type nobody can construct. This waits on the same rule one step further
out - not for something that produces the value, but for something that produces
its evidence.

---

## 6. Three things this found, which are not this document's to fix

Each is one piece of work and each needs its own issue.

**`fs.read` answers a plain `String`, and a model may name the file.** §1 has
the program. `trust.md` §2 licenses the read and is silent about the result, so
a model chooses which of the machine's files reaches a write or an exec. The
question an issue has to decide is whether `fs.read`'s result should carry the
label of its path argument - which is a rule about a capability's *result*
depending on its *arguments*, and the capability table has no way to say that
today.

**Indexing strips the label from the index, in one line, with no comment and no
test.** The behaviour is right and `decisions.md` §1 is its argument, but
`trust.md` §2a says of `len` exactly what is true of this:

> Before this section, `len`'s behaviour was one sentence in a checker and
> nothing in a test: an edit that made it propagate would have looked like a fix.

Here the sentence is missing too. An issue should give it the paragraph in
`trust.md` §2a that `len` has, and the test beside
`len_takes_the_label_off_and_that_is_on_purpose` that says changing it is a
decision.

**The person `approve` asks is not shown the value.** §2a. This is the largest
of the three and it is a gap between what `HumanApproved<T>` means to a reader
of `trust.md` and what it means to whoever typed `--value true`. It has at least
three candidate answers, none of them free, and an issue has to choose between
them rather than list them: a digest of the value in the question, which is
checkable and unreadable; a rendering of the value, which needs a way to turn a
record into text that `decisions.md` §7 has already refused once; or nothing
changing in the runtime and `trust.md` saying plainly what the type claims, so
that a program's author knows the question is the whole of what will be read.

> **Done.** #74 took the second, and `trust.md` §3 argues it against the other
> two: the whole value crosses, as the document `FROM_JSON` would parse back,
> written by an instruction no syntax can reach. The way to turn a record into
> text that `decisions.md` §7 refused is still refused *as a builtin*, which is
> the distinction that made the second answer affordable.

---

## 7. What is deliberately not here

- **`because`, or any second discharge, until §5's capability exists.** The
  whole of §2. Not "not yet designed" - designed, and refused for a reason that
  names what would reverse it.
- **Self-certification**, in every spelling. #71 names the field-of-the-value
  form; §4.1 adds the one that would pass a checkable rule, which is a condition
  over a file the model chose the name of. Any future rule about vacuity has to
  answer that program specifically.
- **A confidence threshold on the model's own score.** The same thing wearing a
  number. E0371 refuses `LLM<Float>` as an operand today, so the literal
  spelling does not compile - #71 calls that an accident, and §4.1 is the reason
  it should stay refused on purpose.
- **Removing `approve`.** Whatever arrives later, the door with a person behind
  it stays, and stays the only thing that produces `HumanApproved<T>`.
- **General information-flow analysis.** Tracking how many bits of a labelled
  value reached a decision is a research programme, and `trust.md` §2a already
  refused a channel budget on the same grounds.
- **Policy in a file outside the program.** The criterion would have to be in the
  bytecode or `sic plan` could not print it, and §4.3 shows that even in the
  bytecode it prints badly.
- **A `Checked<T>` type on its own**, ahead of anything that produces one.
  `trust.md` §5's rule, and §4.2's sharpening of it: the label would have to mean
  a different thing at each site that built it.
- **Narrowing E0372 so a labelled value may reach a write nothing reads.** This
  is §5's first shape and it belongs there rather than here. "Nothing reads it"
  is not something a type system can establish; it is a claim a grant makes, like
  `repeatable`, and it needs the argument `authority.md` §4a gives for words of
  that kind.
- **Making `approve` cheaper rather than adding a second door** - an approval
  that covers a bytecode digest for a week, a batch of values approved in one
  question, a session. Every one of those is a change to `human.approve` and to
  the run store rather than to the label, `sic plan` already prints the digest
  that would identify what was approved, and `plan.md` §4 has been holding the
  question ("No approval flow") since it was written. It is where the pressure
  in #71 should go, and it is a different issue.
- **Making `approve` *better*, which §2a says is a different thing again.**
  Cheaper is about how often a person is asked; better is about what they are
  shown when they are. The second matters more and is the smaller change, and
  §6 has it. Neither belongs in a document about discharging a label without a
  person, which is why both are named here and neither is designed.
