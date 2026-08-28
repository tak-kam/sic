# The loop that carries something

An agent loop is "keep going until it is done". This language cannot write one,
and the reason is one line long: **nothing in it assigns.**

`for x in xs` landed in 0.4.0 and walks a list without a frame per element. A
body can perform effects. It cannot count, cannot accumulate, and cannot decide
to go round again, because there is no name a second iteration can find changed.

This document is called `loops.md` rather than `assignment.md` because
assignment is the mechanism and the loop is the thing being decided. Every
question below - what bounds a loop, what a plan owes about one, what survives a
suspension in the middle of one, what a label does when a value crosses from one
iteration to the next - is a question about the loop. Assignment is what all of
them turn out to need, and it is the first section only because it is the part
that does not exist.

Everything a long loop *survives* on is already here. That was measured before
anything below was written, and most of this document is the measurement.

---

## 1. What is already here

The list is longer than it looks, and every row of it was checked with the
compiler rather than read out of a design document.

| the claim | what makes it true today |
|---|---|
| the HIR can assign | `let` **is** an assignment: `Stmt::Let` lowers to `InstKind::Move { dst, src }` into the slot the name resolved to (`crates/sic-ir/src/lower.rs`) |
| the bytecode can assign | `MOVE a, b` has been in §6 since v0.1, and `ADD_I64 a, a, c` writing its own operand is what `for` already emits |
| a register may be written on a back edge and read at the head | the `for` counter is exactly that, and it verifies |
| the verifier's fixed point handles it | `check_data_flow` is a worklist with a merge that intersects; `for` added no rule to it (v0.1 §5a) |
| the state of a loop survives a suspension | measured in §6 - a `for` body that stops for a person, three times, resumes at the right element |
| the one instruction that grows the arena without a capability call is charged by the byte | `CONCAT` costs a fuel per byte of its result, charged before the string is built (v0.1 §6) |
| a runaway loop takes a child, not the terminal | on unix the interpreter is a process of its own (`processes.md`) |

Here is the loop out of `for x in xs { let y = x + 1; log info "step"; }`, with
the `LEN` and the counter that precede it and the `RETURN` that follows:

```text
  0010  LEN         r9, r0
  0011  LOAD_CONST  r10, k3  ; 0
  0012  LT          r11, r10, r9
  0013  JUMP_IF_NOT r11, +9  ; -> 0023
  0014  GET_INDEX   r2, r0, r10
  0015  LOAD_CONST  r12, k0  ; 1
  0016  ADD_I64     r13, r2, r12
  0017  MOVE        r3, r13
  0018  LOAD_CONST  r14, k4  ; "step"
  0019  LOG         info, r14
  0020  LOAD_CONST  r15, k0  ; 1
  0021  ADD_I64     r10, r10, r15
  0022  JUMP        -11  ; -> 0012
  0023  RETURN      r1
```

Two instructions in that listing are the ones a program cannot write.

`0021` assigns. `r10` is written inside the loop and read at `0012` after the
merge of the entry edge and the back edge, and the verifier accepts it. That is
an accumulator - it accumulates one - and the only thing separating it from
`total = total + x` is that no name in the source reaches `r10`.

`0017` assigns too. The body's `let y` writes `r3` on every iteration, reusing
one register, because a `LocalId` is a register index (`Compiler::reg`) and the
body is one block of HIR whatever it runs. So a source-level binding being
rewritten each time round the loop is not a new thing either. It is what a
`let` inside a loop body already is.

**The machinery is complete. What is missing is a name on the left of an `=`.**

---

## 2. What that costs today, measured

### The fold is capped at about a thousand elements

Without assignment a fold is a recursion, and a recursion is a frame per
element against `MAX_FRAMES = 1024`:

```text
fn count(i: Int, acc: Int) -> Int {
    if i <= 0 { return acc; }
    return count(i - 1, acc + i);
}
```

`count(1020, 0)` answers `520710`. `count(1200, 0)` is

```text
error: call stack too deep
 --> depth.sic:6:12
```

That is the half of issue #66 the `for` loop did not close, and it is a wall
rather than a slope: a list of 1200 things cannot be summed in this language by
any means at all. Nor can a program build one: a list of 1200 elements comes
from 1200 literals or from something a capability handed back, because building
one out of parts is itself an accumulation.

### The program that looks like an accumulator compiles and is wrong

This is the finding that decides how urgent the work is.

```sic
fn main() -> Int {
    let xs = [1, 2, 3];
    let total = 0;
    for x in xs {
        let total = total + x;   // a new binding, every time round
    }
    return total;
}
```

It compiled with no error and no warning, and it printed `0`. Issue #81 came
out of this measurement and is closed: a `let` in a nested block whose
initializer reads the binding it hides is now E0313, so the program above is
refused rather than run. `docs/design/v0.1.md` §2 has that rule and its
argument.

That is the bug stopped, not the feature built. `let` still shadows, in the
same block as well as a nested one - `let x = 1; let x = 2; return x;` answers
`2`, which is what it reads like and is left alone - and the shape somebody
reaches for when they want to accumulate is still not writable at all. E0313
turns a wrong answer into a diagnostic with nothing to point at yet.

A language that has no assignment is a defensible position. A language in which
the assignment somebody writes compiles into a different program is not, and
that is where this one was.

### The agent loop cannot be written

"Ask, check the answer, ask again if it is not good enough, at most five times"
needs two things to change between two visits: the answer and the count. Neither
can. The only spelling available is recursion, which caps at a thousand and
costs a frame per attempt, and which puts the loop's state in a parameter list
where the reader has to reconstruct it.

---

## 3. Assignment, or something narrower

The narrow proposal is an accumulator the loop itself binds, so that nothing
outside a loop can ever be mutated:

```text
for x in xs into total { … }
```

It is the more attractive of the two on first reading, and it is in this
language's register: a construct that cannot be misused because it cannot be
written anywhere else. It is also, on inspection, **larger** than general
assignment rather than smaller, and it cannot express the program that motivated
the issue. Both of those are worth showing rather than asserting.

### The narrow form is assignment with a restriction, or it is block expressions

The header binds `total`. The body has to be able to say what `total` becomes.
There are exactly three ways to spell that, and none of them is cheaper:

1. **`total = e;` inside the body.** Then the feature *is* assignment, plus a
   rule about which single name may appear on the left, plus a scope in which
   that rule applies, plus an answer for a nested loop that wants two. More
   parser, more resolver, identical everything else.
2. **A trailing expression as the block's value** - `{ total + x }`. That is
   block expressions, which v0.1 §2 left out on purpose, and it changes what a
   `{ }` means everywhere in the language.
3. **A `yield` statement** (the word is reserved). A statement that means one
   thing inside one construct and is an error everywhere else, with a rule for
   how many times it may appear in a body and what happens on the path that
   does not reach it.

The cheapest of the three is (1), and (1) is `x = e` with a restriction bolted
on. The restriction is not free: it has to be written, tested, and explained,
and what it buys is that `let mut tries = 0;` outside a loop is refused - a
statement nobody has argued is dangerous.

### It cannot write the agent loop

`for … into …` walks a list. "Keep going until it is done" is not a list walk;
it has no list. The narrow form therefore needs `while` beside it to reach the
motivating program, and a `while` with no assignment has nothing that can change
between two visits to its condition - which is precisely the argument v0.1 §2
gives for `while` staying reserved.

So the narrow form does not remove the need for assignment. It postpones it, in
exchange for a second binding form that will still be there afterwards.

### What survives from it

One thing, and it is worth keeping as a habit rather than a rule: a loop that
carries something is easier to read when the thing it carries is named at the
top. `let mut total = 0;` on the line before `for` does that, for free, and a
reader can see the whole of what crosses an iteration boundary by looking at
which `mut` bindings are in scope. That is a convention, not a construct.

### The decision

**`mut` and `=`.** The narrow form is refused, and #79 should be told that the
accumulator is not enough: it needs `while` to reach a harness's retry loop, and
`while` needs assignment, so the narrow form buys nothing and costs a second
binding form.

The shape:

```ebnf
let_stmt    = "let" [ "mut" ] IDENT [ ":" type ] "=" expr ";" ;
assign_stmt = IDENT "=" expr ";" ;
```

Four decisions are taken by that grammar and each is deliberate:

- **A `mut` binding has an initializer**, exactly as a `let` does. There is no
  `let mut x: Int;`, so a register the verifier would call uninitialized cannot
  be produced from source, and §7's rule stays a rule about hand-written
  bytecode.
- **The target is a bare name.** `xs[0] = e` and `d.cause = e` are not in the
  grammar, so `SET_FIELD` cannot arrive by accident and a mutable field stays
  the separate decision v0.1 §6 says it is. A parser that reaches `=` after an
  expression statement reports it (a new code, E0222) rather than complaining
  about a missing semicolon.
- **Assignment is a statement.** `y = (x = 1)` is not an expression, so there is
  no value for one to have and no question about what an assignment inside a
  condition means.
- **Two tokens of lookahead.** `Parser::peek_next` already exists; a statement
  beginning `IDENT =` is an assignment and nothing else begins that way. No
  backtracking, which is what v0.1 §2 constrains the grammar to.

What may not be assigned to: a binding declared without `mut`, a parameter, and
a `for` binding. All three are one refusal - the thing on the left is not a
`mut` binding - and so they are one diagnostic (E0377) with three shapes. A
parameter is the caller's value; a `for` binding is the loop's, rewritten each
time round by `GET_INDEX`, and letting a body assign it would make v0.1 §2's
sentence about what it holds - "one element of the list, with the list's
provenance still on it" - false.

---

## 4. `while`, and what bounds it

```ebnf
while_stmt = "while" expr block ;
```

Read exactly as an `if` header is: no parentheses, a mandatory block, and the
expression may not be a bare struct literal. It lowers to the same three blocks
`for` produces - head, body, exit - without the counter and the `GET_INDEX`.

One difference from `for` is worth writing down because it is the whole of what
a `while` is. `for` evaluates `LEN` **once, before the head**, which is what
fixes the count; a `while` puts the condition's own instructions **inside the
head**, so they run again on every visit. That is the point, and it is also the
line where "every loop ends" stops being true.

### What bounds it

Fuel, and only fuel. A `while true { }` ends with `ran out of fuel` after ten
million instructions, which is a real bound and an unhelpful one.

That is a change to something v0.1 §2 states as a property of the language:

> The count is `len(xs)`, taken once when the loop starts, so **every loop
> ends**.

After `while`, that sentence is false, and it should be replaced rather than
qualified. What replaces it is weaker and still true: *every run ends, because
fuel bounds it.* The difference matters. "Every loop ends" is a property of the
program a reader can check by reading it. "Every run ends" is a property of the
runtime, and a reader cannot tell a loop that finishes in nine iterations from
one that finishes in nine million by looking at it.

Two things already in the tree make that affordable, and neither was built for
this:

- **`CONCAT` costs a fuel per byte of its result**, charged before the string is
  built (v0.1 §6). So the one instruction that can grow the arena without a
  capability being called is bounded by the same budget that bounds the loop -
  a `while` that joins strings forever runs out of fuel at the byte, not at the
  allocator. That charge was taken for `processes.md` §2's 230 MB measurement
  and it is what makes an unbounded loop's memory answerable.
- **The interpreter is a process of its own** on unix, and it is the default. A
  loop that calls a capability returning a megabyte, ten thousand times, fills
  an arena with no GC and spends almost no fuel doing it - `for` over a long
  list can already do that, and `while` removes the "the list had to come from
  somewhere" part. What stops it taking the terminal, the run store and the
  journal with it is the child.

There is still no `--fuel` flag, and this is the change that makes one wanted -
see §9.

### What the plan owes

`sic plan` currently ends with:

```text
1 capability call site(s), none with a budget, so how often they run depends on
the path taken.
```

The temptation is to think `while` weakens that sentence. It does not, and the
reason is measurable. A capability call inside a `for` body plans as:

```text
  main
    1. APPROVE  human.approve   "keep going"   ; 10:18
```

and the same call inside a recursion plans identically - and a recursion's
count is *already* unbounded by anything the plan can see, today, with no loop
in the language at all. So the honest summary of what `while` costs the plan is
**nothing in its numbers.** `plan.md` §3 already refuses to sum `retry` counts
and calls the total a guess dressed as a fact; the sentence it prints instead
is exactly as true after `while` as before it.

What the plan does owe is a **mark**, and it owes it today:

```text
  main
    1. APPROVE  human.approve   "keep going"   (in a loop)   ; 10:18
```

The difference between a call site the program reaches at most once per visit
and one it reaches as often as it likes is the single most decision-relevant
thing about a call site, and a plan that prints them the same way invites a
reader to count lines and get a number that means nothing. It is computable
from the bytecode with no analysis the tool does not already do - the
instruction is on a cycle in the control flow graph - and it needs no dominance
analysis, so it does not cross the line `plan.md` §3 draws around "these three
definitely, those two maybe".

**That gap exists now.** `for` landed in 0.4.0 and the plan says nothing about
it. `while` does not create the debt; it makes it loud enough to notice.

### What a condition may be

A plain `Bool`, which E0301 already enforces, because `while` inherits `if`'s
rule without a new one being written. Measured:

```text
error[E0301]: expected Bool, found LLM<Bool>
  --> cond.sic:19:8
   |
19 |     if a.ok {
   |        ^^^^ this condition has type LLM<Bool>
```

That has a consequence for the loop this whole document is about, and §9 records
it as a finding rather than resolving it here.

---

## 5. What a mutable binding does to a trust label

This is the decision to get right. `trust.md` §2a's guarantee is:

> **Once attached, a label never comes off a value.**

Assignment is a new way to move a value, so the question is whether it is a new
way to move a label. The answer is that it is not, and the reason is one rule
with three consequences.

### The rule

> **A `mut` binding's type is fixed at its declaration, and every assignment to
> it is checked against that type exactly - label included.**

The type comes from the annotation if there is one and from the initializer
otherwise. There is no inference across assignments, no widening, and no
subtyping, which is the same position v0.1 §4 takes on numbers: there are no
implicit conversions here.

That rule is not new. It is the check `let x: T = e` already makes, and both
directions of it were run before this was written:

```text
error[E0301]: expected String, found LLM<String>
  --> label.sic:19:25
   |
19 |     let clean: String = d.cause;
   |                         ^^^^^^^ this initializer has type LLM<String>
```

```text
error[E0301]: expected LLM<String>, found String
 --> label2.sic:3:26
  |
3 |     let x: LLM<String> = "";
  |                          ^^ this initializer has type String
```

So a label cannot be annotated off a value and cannot be annotated onto one.
Assignment inherits both refusals by being checked the same way, and **no new
diagnostic code is needed for trust at all.**

### The three cases the issue named

**Assigning an `LLM<String>` to a `mut x: String`.** Refused, E0301, by the
first measurement above. The label does not come off.

**A binding whose label would differ between iterations.** Cannot happen, and
this is the part worth reading twice. The type is fixed at the declaration, so
either every assignment produces that type or one of them is refused *at its own
site*, statically, with a span. There is never a value with two labels arriving
at a merge point, so no rule about merging labels has to be invented, and
`trust.md`'s refusal to put an order between the labels (E0375) stands.

```sic
let mut answer = "";           // String
for x in xs {
    answer = ask(x);           // LLM<String> -> E0301, here, on this line
}
```

**Whether a `mut` binding's type is fixed at its declaration.** Yes, and the
alternative is worse than it sounds. Inferring a binding's type from all of its
assignments would mean a label that depends on a path, which needs a join over
the labels, which needs an order between them, which `trust.md` §2a refuses to
invent for a program nobody has written. Fixing the type at the declaration is
how that question is kept from being asked.

### What §2a gains, and it is one subsection

> **Assignment moves a value; it does not move a label.**
>
> A `mut` binding's type is fixed where it is declared, and every assignment is
> checked against it the way an annotated `let` already is. So a labelled value
> cannot be assigned into a plain binding (the label would be gone) and a plain
> value cannot be assigned into a labelled one (the label would be invented).
> Neither is a new rule; both are E0301 reaching a second statement.

### The accumulator that does work, and the one that does not

A labelled accumulator works when its initializer is itself the call:

```sic
let mut answer = ask(q);            // LLM<String>
let mut tries = 1;
while tries < 5 && !good(answer) {
    answer = ask(q);                // LLM<String>: the same type
    tries = tries + 1;
}
```

That is the agent loop, and it type-checks under the rule above with nothing
added. It is also the natural shape - ask, then check - rather than a
workaround.

A labelled accumulator does **not** work when it is a fold:

```sic
let mut report = "";                // String
for x in xs {
    report = report + ask(x);      // String + LLM<String> is LLM<String>: E0301
}
```

`+` on two strings carries the label (`trust.md` §2a), so the right-hand side is
`LLM<String>` and the binding is `String`. Declaring the binding
`LLM<String>` does not help, because the initializer would have to be a plain
`""` and that is the second measurement above. And two different labels joined
are E0375 regardless.

**So a program cannot fold a model's answers into one string.** That is a real
limit, it is found by the first person who tries, and it is not caused by
assignment: it is `trust.md` §2a's deliberate refusal to name the origin of a
joined value, met from a new direction. §9 records it as the issue that now has
its program.

### Two things assignment does not do

**It does not create shared state.** There are no references, and a spawned task
has its own register stack (`TaskSnapshot { regs, frames }` - one per task in a
checkpoint). A `mut` local is a slot in one frame of one task, so
`concurrency.md` needs no rule about a task seeing another's assignment: it
cannot name it.

**It does not reach the journal.** An assignment is a `MOVE`, and a `MOVE` is
not an event. A loop that runs a hundred times produces a hundred capability
events and nothing else, which is what a `for` loop already produces.

---

## 6. The checkpoint, measured

The issue says this "may be free" and that "may be free" is not "is free". It
is free, and here is the run that says so.

```sic
allow { human.approve "keep going"; }

fn main() -> Int {
    let xs = [10, 20, 30];
    for x in xs {
        let ok = human.approve("keep going?");
    }
    return 0;
}
```

Run and resumed twice, with the checkpoint written each time:

| step | checkpoint | exit |
|---|---|---|
| `sic run` | 469 bytes | 3 |
| `sic resume … --value true` | 479 bytes | 3 |
| `sic resume … --value true` | 479 bytes | 3 |
| `sic resume … --value true` | - | 0 |

Three suspensions, three resumptions, and the loop ended after exactly three
elements. **The loop counter survived a process boundary twice and the loop
resumed at the right element**, with nothing in `checkpoint.rs` written for it:
the counter is a register, registers are in `TaskSnapshot::regs`, and that is
the whole mechanism.

Two numbers in that table are the answer to the question the issue actually
asked.

**It grows once and then stops.** The ten bytes between the first and the second
are accounted for by registers the first iteration wrote and the first
suspension had not - `write_value` costs one tag byte for a `Value::Unit` and
nine for an `I64` - and 479 to 479 is every iteration after that. A checkpoint
is proportional to the registers a frame has, not to how many times a loop has
been round.

**An accumulator is one more register**: a tag byte plus at most eight, once, at
every suspension, whatever the loop does. That is the cost, in full.

So: no format change, no `VERSION_MINOR` bump, no new consistency check.
`check_consistency` already validates every register value against the arena it
points into, and an accumulator is a register value.

The one thing that is *not* free, and that is not the checkpoint's fault:
**fuel is carried across a resume** (`Checkpoint::fuel`) and does not reset. A
loop that suspends for a person on every iteration spends from one ten-million
instruction budget across however many days it takes, and there is no flag to
raise it. That is fine for the loops anybody writes today and it is a wall for a
loop that is meant to run for a month. §9.

---

## 7. The verifier, measured

Also free, and the measurement is the disassembly in §1.

`0021 ADD_I64 r10, r10, r15` writes a register on the back edge. `0012 LT r11,
r10, r9` reads it at the head, after the merge of the entry edge and the back
edge. That program verifies today, in every `for` loop that has ever compiled.

`check_data_flow` is a worklist over instructions with a merge that intersects:

```rust
(a, b) if a == b => a,
(Abst::Uninit, _) | (_, Abst::Uninit) => Abst::Uninit,
_ => Abst::Top,
```

An accumulator hits the first arm. The binding is initialized before the head
dominates it, so the entry edge carries `Val(t)`; the rule in §5 fixes the type,
so the back edge carries `Val(t)` too; `a == b`, nothing changes, the fixed
point is reached on the first pass over the back edge. Nothing is added to
`sic-verify` and no opcode is added to `sic-bytecode`.

Two properties are worth recording because they are what makes it free rather
than lucky:

- **The lattice is per-register and the merge only loses information**, which is
  what makes the fixed point terminate. An assignment is a register write like
  any other; the pass has never cared where a write came from in the source.
- **`Uninit` is unreachable from source.** Every `mut` binding has an
  initializer that dominates every assignment to it, and a binding declared
  inside a branch is out of scope outside it. So the initialization rule stays a
  rule about hand-written bytecode, which is what it has always been.

The one place the verifier could be reached by a source-level mistake is
`Abst::Top`, whose message is

```text
r10 holds different types depending on the path taken
```

That is a bad message for a person who wrote `x = "no"` where `x` is an `Int` -
it names a register. Which is exactly why the rule in §5 belongs in the type
checker: E0301 names the line and the two types, and the verifier's message is
then only ever produced by bytecode that did not come from this compiler.

---

## 8. The smallest useful version

**Assignment, and the `for` loop that already exists.** Units 1 to 3 below.

That is smaller than the issue asks for, and it is worth being precise about
what it closes:

- counting, and accumulating, and folding a list of any length in one frame -
  the half of #66 that `for` did not close;
- building a list, which is an accumulation and which today cannot be done at
  all beyond a literal;
- the retry loop, bounded: `for i in [0, 1, 2, 3, 4] { … }` compiles and runs
  today - that was measured - and with assignment its body can carry the answer
  and a `done` flag, so it runs at most five times and stops asking once the
  answer is good;
- the silent wrong answer in §2, which is the thing that should not be left
  standing.

and what it does not:

- "until done" with no bound written down. The `for`-over-a-literal spelling
  above says "for each of these five numbers" when it means "at most five
  times", and a reader has to be told the difference.

If only one thing ships, it is units 1 to 3. `while` is worth building and is
worth building second, because it is the unit that takes "every loop ends" away
and it should not be the unit that also introduces `mut`.

---

## 9. What this turned up

Each of these is its own piece of work with its own argument, and none of them
is closed by this document. They are listed because they were found by running
the compiler rather than by reading it, which is where issues in this repository
come from.

- **`sic plan` says nothing about a call site in a loop, and `for` has been in
  the language since 0.4.0.** §4 has the argument and the shape. This is a debt
  the tool already carries.
- **A labelled `Bool` cannot be a loop condition, so "the model says it is
  done" cannot be asked directly.** A program has to declare `output: String`
  and ask `contains(answer, "DONE")` instead of `output: Answer` with an `ok:
  Bool` field, which is a worse program written to satisfy a rule. `trust.md`
  §2a's "a branch is not an effect" argues that a model deciding a branch is
  fine; the refusal is E0301 being a type equality rather than a decision.
  `trust.md` already names this as "a decision about `if`, `while`, `!` and the
  connectives at once" - `while` arriving is when it stops being hypothetical.
- **A fold over labelled strings cannot be written** (§5). `trust.md` §2a says
  the program that finds E0375 intolerable is the issue that argues for an order
  between the labels. This is that program, and it appears the first time
  anybody accumulates model output.
- **There is no `--fuel` flag, and a durable loop spends one budget across every
  resume** (§6). `processes.md` §2 noted the missing flag as a fact about
  memory; a loop that waits for a person on every iteration makes it a fact
  about how long a run may live.
- **`let` shadows in the same block with no warning** (§2). Independent of
  everything here: `let x = 1; let x = 2;` is legal and silent. E0313 (issue
  #81) took the accumulator out of that set - a nested `let` that reads what it
  hides is refused - and deliberately left this one, because the answer matches
  the reading. What would report it is an unused-binding warning, which is
  another rule about another thing and does not exist.

---

## 10. Units of work

1. **Assignment.** `let mut x = e;` and `x = e;`. `mut` leaves the reserved
   list. Parsing is one statement arm on two tokens of lookahead; resolution
   carries a mutable flag; checking compares the right-hand side against the
   binding's declared type (E0301, existing); lowering emits `InstKind::Move`
   into the resolved slot, which is what `let` already emits.

   Done when `let mut total = 0; for x in xs { total = total + x; } return
   total;` answers correctly for a list longer than 1024, when `sic disasm`
   shows no instruction that did not exist before, and when the three refusals -
   a binding without `mut`, a parameter, a `for` binding - are one code (E0377)
   with a test each, plus E0222 for a target that is not a name. Both codes in
   `docs/diagnostics.md`, which a test checks.

2. **The trust rule.** The subsection in `trust.md` §2a from §5, and the tests
   under it: a labelled value assigned into a plain binding is refused, a plain
   value assigned into a labelled binding is refused, and a labelled
   accumulator whose initializer is the call works. The third is the one that
   matters - it is the agent loop, and a rule that refused it would be a rule
   that refused the feature.

3. **The checkpoint test.** A program that accumulates across a suspension,
   checkpointed and resumed, answering the accumulated value. The claim it pins
   is that `VERSION_MINOR` did not move, which is a claim that is easy to break
   later and impossible to notice.

   Units 1 to 3 are the smallest useful version and finish together.

4. **`while`.** The header, the block, and the lowering to `for`'s three blocks
   without the counter. Done when a retry-until-it-validates loop runs against a
   recorded run, when `while true { }` ends with `ran out of fuel` rather than
   hanging, and when v0.1 §2's "every loop ends" has been replaced by §4's
   weaker sentence rather than left standing.

5. **The plan marks a call site inside a loop.** Separable, and it should be
   lifted into its own issue if unit 4 is deferred - the debt is `for`'s and
   predates this. It should not be lifted if unit 4 lands, because `while` is
   what makes the mark load-bearing.

6. **The documents.** v0.1 §2's grammar, keyword list and the paragraph arguing
   for one loop; §5a's closing sentence, which says the fold "closes when `mut`
   does"; `docs/status.md`'s two `Deliberately not built` bullets. Each of those
   is a sentence that becomes false on the commit that makes it false, not a
   week later.

---

## 11. What is deliberately not in this

- **`break` and `continue`.** A second exit edge and a second merge point per
  loop. The verifier would take both without a change - a `break` is a `JUMP` to
  the exit block, which is what `JUMP_IF_NOT` at the head already produces - so
  the reason to wait is not cost. It is that `while cond && !done` and an `if`
  around the body express the same programs, and the loop that finds that
  intolerable is the issue. A `continue` in particular has to answer where the
  `for` counter's increment happens, which is a decision about §5a's lowering
  and not about control flow.
- **Mutable fields.** `SET_FIELD` is listed in v0.1 §6 as a later phase and the
  grammar in §3 refuses `d.cause = e` by construction rather than by a rule, so
  it cannot arrive by accident. A loop accumulating into a local is not a record
  being edited, and a labelled record whose field could be assigned would reopen
  the question §5 closes by fixing a binding's type.
- **Compound assignment.** `total += x` is a second spelling of one act, which
  is the failure #72 and #73 were both about from the other direction.
- **`let mut x: T;` without an initializer.** §3 says what it would cost: a
  register that is genuinely uninitialized on some path, which turns the
  verifier's initialization rule from a check on hand-written bytecode into a
  diagnostic a person has to understand.
- **`mut` on a parameter.** A parameter is the caller's value. A callee that
  wants a counter can declare one.
- **Assigning a `for` binding.** v0.1 §2 says what it holds and where its
  provenance comes from, and a body that could replace it would make that
  sentence false.
- **Closures.** The other way to write a fold and a much larger feature: a
  captured environment is the first thing in this language that would outlive
  the frame it was made in, which is a question about the arena as well as
  about the type system.
- **Making `for` fold by inference.** A loop that threads a value the program
  did not name would be worse than not having one, and it is what the narrow
  form in §3 turns into if the accumulator's update is ever made implicit.
- **Ranges.** `for i in 0..n` wants an iterator or a materialized list, and the
  loop already takes a list. Nothing here needs one.
- **A labelled condition, an order between labels, and `--fuel`.** Each is in
  §9 with its argument, and each is its own issue.
