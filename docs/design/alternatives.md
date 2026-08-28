# Reading a stream that carries more than one shape

Nothing in the language alternates. `TypeDesc` runs unit, bool, int, float, str,
task, list, record, and none of them is a choice between two others. That is
fine until a program has to read a stream somebody else designed, because a
discriminated union is the only way to put more than one kind of message on one
stream, and so every JSONL protocol is one.

Issue #77 asks for the type. This document settles it, and the reason it is one
document rather than a patch is the second question in that issue:

> **A sum type nothing can take apart is a sum type nobody can use.**

`sic` has no pattern matching - `v0.1.md` §2 left it out and `match` is still a
reserved word - so the answer cannot be borrowed. §3 finds it, and the finding
is that **the discriminating field is the runtime tag**, which turns out to mean
that reading a sum needs no new instruction, no new runtime representation, no
new element in the verifier's lattice and no change to the checkpoint format.
One instruction is added, for the extraction, and §6 argues it is the same
decision `xs[i]` already made.

It is called `alternatives.md` rather than `sums.md` because the word the rest
of the tree uses for the gap is "alternates", and the file a reader reaches for
when a stream carries three kinds of line is not one named after a type theory.

---

## 1. What the stream actually looks like, measured

Every number below comes from a run, not from the issue. Cargo 1.98, building
two crates of this workspace with `--message-format=json`, once clean and once
with a deliberate `dead_code` warning so that all three shapes appear:

```text
build-finished     2  reason success
compiler-message   5  manifest_path message package_id reason target
compiler-artifact  9  executable features filenames fresh manifest_path
                      package_id profile reason target
```

That reproduces what `answers.md` §3 recorded, and it is where the issue's
account stops. The issue then says a sum of closed records "needs no openness,
because each arm declares the whole of its own shape". **Walking the same lines
one level further down shows that is not true of this stream**, and the three
reasons are each a wall of their own:

```text
.                       9  executable features filenames fresh manifest_path
                           package_id profile reason target
.target                 8  crate_types doc doctest edition kind name src_path test
.profile                5  debug_assertions debuginfo opt_level overflow_checks test
.message                7  $message_type children code level message rendered spans
.message.children[]     6  children code level message rendered spans
.message.spans[]       13  byte_end byte_start column_end column_start expansion
                           file_name is_primary label line_end line_start
                           suggested_replacement suggestion_applicability text
.message.spans[].text[] 3  highlight_end highlight_start text
.message.code           2  code explanation
```

- **`$message_type` is not an identifier.** `type M { $message_type: String }`
  stops at the lexer: ``error[E0101]: unexpected character `$` ``. No sum type
  changes that; only not declaring the field does.
- **`spans[].expansion` is a cycle through a record.** A rustc expansion holds a
  span, and a span holds an expansion. `agents.md` §1 refuses that, and the
  compiler says so:

  ```text
  error[E0340]: type `Span` contains itself
    = note: a `List<T>` or a `Task<T>` breaks the cycle, because both are handles
  ```

  `children`, which is a list, is fine - `List<Self>` compiles today and was
  checked. `expansion`, which is not, is not.
- **`executable`, `code.explanation`, `spans[].label`,
  `spans[].suggested_replacement` and `spans[].expansion` were all `null` in the
  measured lines.** `v0.1.md` E0312 refuses `null`, and `from_json` refuses a
  field whose value is one.

So of the three arms, exactly one - `build-finished` - can be written as a
closed record in `sic` as it stands, and it works today:

```sic
type Finished { reason: String, success: Bool }

fn main() -> Bool {
    let line = "{\"reason\":\"build-finished\",\"success\":true}";
    let f: Finished = from_json(line);
    return f.success;
}
```

```console
$ sic run finished.sic
true
```

**This is the first thing this document decides, and it decides the order of
the work.** A sum type does not make cargo's stream readable. Open records
(#76) do, and they do it for both of the arms that a sum type on its own leaves
unwritable. §10 says what follows.

---

## 2. The one shared key is the whole design

The three shapes share exactly one key, `reason`, and it is the key that says
which shape it is. That is not cargo being awkward. This repository's own
journal is built the same way:

```json
{"ts":...,"seq":0,"run":"8805...","task":0,"span":0,"parent":null,
 "event":"run_started","workflow":"main","args":"sha256:af55..."}
{"ts":...,"seq":6,"run":"8805...","task":0,"span":1,"parent":0,
 "event":"task_completed","result":"sha256:4acb..."}
```

`event` is the discriminant; six keys are shared; `parent` is `null` at the top
of the trace. A protocol chooses one field to carry the answer to "which of
these is it", writes it on every message, and the reader branches on it. **The
tag is already in the document.** Every design below follows from taking that
seriously rather than inventing a second one.

---

## 3. How a program takes a sum apart

### 3a. The shape of the program already exists

Before designing anything, it is worth writing the program a sum type is
supposed to enable, in the language as it stands, and seeing what is missing.
This compiles and runs today:

```sic
type Finished { reason: String, success: Bool }
type Message  { reason: String, level: String }

fn classify(line: String) -> String {
    if contains(line, "\"reason\":\"build-finished\"") {
        let f: Finished = from_json(line);
        if f.success { return "built"; }
        return "failed";
    }
    if contains(line, "\"reason\":\"compiler-message\"") {
        let m: Message = from_json(line);
        return m.level;
    }
    return "other";
}
```

```console
$ sic run classify.sic
"built"
```

Three things are worth taking from that, because they are what a sum type has
to beat.

- **The destructuring problem is smaller than it looks.** Each branch parses
  the whole line into that branch's own record type, so every `GET_FIELD` in
  it is on a register the verifier already knows the exact type of. The two
  record types never meet at a merge point, because each is consumed inside the
  block that made it. That is precisely what an arm of a `match` would give,
  and the language's existing block scoping gives it for free.
- **What is missing is not destructuring.** It is that the discrimination is
  textual (`contains` on raw JSON, which is wrong the day a field value contains
  the same bytes), that the line is parsed once per candidate rather than once,
  that nothing declares the three shapes to be one protocol, and that nothing
  checks that the branch and the parse agree - a program may test for
  `build-finished` and then parse as `Message`, and it compiles.
- **A sum type is therefore an improvement on a working pattern, not a new
  capability.** That is worth stating plainly, because it is the honest size of
  the feature and it is why §11 does not propose building it first.

### 3b. What must not happen at a merge point

The verifier's data-flow pass keeps one abstract value per register:

```rust
enum Abst { Uninit, Val(u32), Top }
```

`Val` is an index into the type section, `merge` widens two different `Val`s to
`Top`, and reading a `Top` register is an error - "r4 holds different types
depending on the path taken". The worklist runs to a fixed point over blocks,
and the same `next` state is pushed to every successor of an instruction.

A design that narrows a register on the true edge of a branch - "inside this
`if`, `line` is an `Artifact`" - therefore needs three things the pass does not
have: a per-edge state rather than a per-instruction one, a fact relating the
`Bool` a test produced to the register it was about, and an invalidation rule
for when either register is written again. That is occurrence typing, it is a
lattice change, and it is the part of a `match` that is expensive.

**So the design below narrows at an instruction rather than at a branch**, and
the merge rule never has to represent a partly-narrowed register. `Abst::Val(s)`
where `s` is the sum's own index already means "one of these"; a narrowed
register is `Abst::Val(a)` for the arm, produced by an instruction whose result
type is a function of its operands, exactly as `FROM_JSON`'s already is. The
lattice needs no new element. That is the test the issue set, and this is how it
is passed.

### 3c. The discriminant is the tag

A record at run time is `Vec<Value>` in the arena, reached through
`Value::Object(Handle)`. **It carries no type.** `GET_FIELD` reads
`arena.object(h)[c]` and never consults the type section; `value_to_json` and
`build_from_json` are driven entirely by the type index the instruction names.
So a sum-typed register holding an object cannot, today, be asked which arm it
is - and any `is`/`as` pair would seem to need a tag, which means a new `Value`
variant or a tag column in the arena, which means a checkpoint format change,
because `checkpoint.rs` serialises the object store.

None of that is necessary, because **the protocol already put the tag in the
value**. Require two things of a sum declaration:

1. every arm carries the discriminating field, with type `String`
2. the compiler lays that field out at **position 0** of every arm

Field order is already the compiler's business - `agents.md` §3: "Fields are
addressed by position, not by name: the compiler knows the layout" - so the
second is a layout rule, not a language rule. With it:

| what a program asks | how it is answered | new machinery |
|---|---|---|
| which arm is this | `GET_FIELD a, b, 0` then `EQ` against a string constant | none |
| give me the arm | one new instruction, `AS_ARM` | one opcode |
| write it back out | `TO_JSON` reads field 0, finds the arm, serialises with that arm's field list | none |
| parse it | `FROM_JSON` matches the document's discriminant against the arms | none |

Discrimination costs **no instruction at all**: a sum type is a record with one
field, that field is the discriminant, and `line.reason` is a `GET_FIELD` at
position 0 that the verifier already knows produces a `String`. The whole of the
"how does a program branch on it" question is answered by `==` on strings, which
landed with its own argument in `v0.1.md` §4.

---

## 4. The declaration

```sic
enum Line(reason) {
    "compiler-artifact" -> Artifact { package_id: String, fresh: Bool },
    "compiler-message"  -> Message  { package_id: String, level: String },
    "build-finished"    -> Finished { success: Bool },
}
```

- **`enum` is the keyword, and it was reserved before it was real**, which is
  the whole point of the reserved list in `v0.1.md` §2. Nothing is reserved
  here that was not reserved already.
- **`(reason)` names the discriminating field.** Every arm gets it implicitly,
  typed `String`, at position 0. Writing it again in an arm is an error rather
  than a redundancy, so there is one place it is declared.
- **`->` is the existing `Arrow` token.** The lexer does not change. `=>` was
  the other candidate and would have been a new token for no gain.
- **An arm body is the record syntax `type` already parses**, so the arm
  production is one line of the parser and the field grammar is shared.
- **An arm name is a type name in the module's namespace.** `Artifact` is a
  type; `let a: Artifact = ...` works; two enums with an arm of the same name
  collide the way two `type` declarations do.

Reading it:

```sic
fn classify(line: String) -> String {
    let l: Line = from_json(line);
    if l.reason == "build-finished" {
        let f = l as Finished;
        if f.success { return "built"; }
        return "failed";
    }
    if l.reason == "compiler-message" {
        return (l as Message).level;
    }
    return "other";
}
```

`as` is reserved and unused, so it costs nothing to spell the extraction with
it. Its right operand is a type rather than an expression, so the Pratt parser
can take it at the postfix level and keep looping - the parentheses above are
for a reader, not for the parser, and `l as Message.level` parses the same way,
because a type is a single identifier and cannot swallow the `.`.

---

## 5. How the arm is chosen when validating

**By the declared discriminant.** `FROM_JSON` into a sum type finds the member
named by the discriminant, requires it to be a string, and looks that string up
among the arms. Exactly one arm can match, because two arms declaring the same
value is a compile error. What is then validated is that arm's record, by the
code that already validates records, with the path-naming error messages it
already produces.

The alternative in the issue - try each arm until one fits - is refused, and
the reasons are in the order that decides:

| | declared discriminant | try until one fits |
|---|---|---|
| ambiguity | impossible by construction | needs a rule, and any rule is arbitrary |
| diagnostics | "`Line` has no arm `compiler-warning`" names the field and the value | "no arm fits", with three abandoned paths and no way to say which was meant |
| cost | one lookup | validate up to *n* times, allocating into the arena on the way and abandoning it, since the arena has no collector |
| what protocols do | this | not this |

The last row is the argument and the third is the one that would have decided it
anyway. There is no garbage collector, and `crates/sic-vm/src/value.rs` says why
in as many words: "a workflow allocates, runs, and the whole arena is dropped at
the end. Reclaiming memory mid-run is a problem for the phase that has programs
long enough to need it." A validator that
speculatively builds a nine-field record and then throws it away leaks for the
rest of the run, once per line.

A stream whose arms cannot be told apart by a single field is a stream this
design cannot read. That is a real restriction and it is the right one: such a
stream cannot be read reliably by anything, which is why protocols do not build
them.

---

## 6. `as` fails the run, and that is the same decision `xs[i]` made

`l as Finished` compares field 0 against the arm's declared value and fails the
run if they differ. Nothing forces the program to have tested first, and nothing
checks that the test it did write is the test that matches the cast.

That is the shape of a decision this language has already made twice.

> Indexing is a postfix operator. An index out of range fails the run: there is
> no option type to return instead, and a silent default would be worse.
> — `agents.md` §2

`FROM_JSON` is the second: a document that does not fit fails the run, measured -

```text
error: the document does not fit the type: `Finished` needs a field `success`
 --> line.sic:8:23
```

- and both name a line, because the debug section maps a pc to one. A failing
`as` does the same. The alternative is an option type, which does not exist, or
narrowing on the branch, which is §3b's expensive thing.

What this costs honestly: a program can be wrong about which arm it holds, and
the verifier cannot say so. What it does not cost: memory safety or a wrong
value. The VM checks before it hands anything back, so the failure is a run
failure at a named line rather than a field read off the end of a record.

**Narrowing on the branch is the improvement that removes this**, and it is
deferred rather than refused. §12 says what it would take.

---

## 7. What the bytecode holds

```rust
pub enum TypeDesc {
    // ... 0..=7 unchanged
    /// A sum: its name, the field every arm carries at position 0, and the
    /// arms, each a value of that field and the index of the record it selects.
    Sum {
        name: String,
        discriminant: String,
        arms: Vec<(String, u32)>,
    },
}
```

Tag 8. **No existing tag's layout changes**, which matters for §8.

One new opcode:

```text
AS_ARM  a, b, c   ; R[a] = R[b] as the arm type T[c]; fails the run if the
                  ; discriminant of R[b] is not that arm's declared value
```

Opcode numbers may have gaps (`v0.1.md` §6), so this takes the next free number
and renumbers nothing.

### What the verifier proves, and the trap in the code it must not fall into

- `AS_ARM`: `T[c]` is an arm of some `Sum`; `R[b]` holds that sum;
  `R[a] = Abst::Val(c)`. Every part is a function of the instruction's own
  operands, so the abstract value is exact at every program point and no merge
  is involved.
- `GET_FIELD` on a sum: only index 0 is in range, and it produces `Str`.

The second has a trap that reading the code finds and reading the issue does
not. `fields()` on `TypeDesc` is used by **two** instructions:

```rust
Op::MakeObject => {
    let Some(fields) = p.types.get(inst.b() as usize).and_then(|t| t.fields()) else { ... };
```

```rust
Op::GetField => {
    ... p.types.get(ty as usize).and_then(|t| t.fields())
```

If `Sum` answered `fields()` with its one discriminant field, `MAKE_OBJECT`
would accept a sum type index and build a one-field object that every later
instruction would treat as a sum - a value claiming an arm it does not have, and
a hole under `AS_ARM`. **`fields()` must keep answering `None` for a sum, and
`GET_FIELD` must gain its own arm for it.** The two callers want different
questions and have been sharing an accessor because until now they had the same
answer.

That is also the reason a struct literal for an arm is refused in the first
version (§11): the only ways to obtain a sum-typed or arm-typed value are
`FROM_JSON` and `AS_ARM`, both of which set the discriminant from a declaration.

### `TO_JSON`

`TO_JSON` (opcode 34) is reached only from `approve`, which shows a person a
value. Given a sum type index it reads field 0, finds the arm whose declared
value matches, and serialises with that arm's field list. A value whose
discriminant matches no arm cannot exist, because nothing but `FROM_JSON`
produces one - and if a tampered checkpoint produced one anyway, the render
fails the way `value_to_json` already fails for a shape that does not match,
with a message rather than a panic.

---

## 8. `VERSION_MINOR` moves, and the reason is not the usual one

The rule on the record is that a changed section layout is the decoding
ambiguity that warrants a bump, and two new opcodes have been judged not to.
A new type tag is neither: it changes no existing entry's layout, and
`decode_types` already refuses what it does not know -
``unknown type tag {other}``.

But the decoder demands **exact** equality:

```rust
if (major, minor) != (VERSION_MAJOR, VERSION_MINOR) {
```

so every minor number is a hard boundary in both directions and no reader is
ever tolerant of another's file. That makes the bump cost nothing, and it buys
one thing: without it, a file with a sum type in it and a file without would
both claim 0.8, and an older `sic` would decode the header, the constants and
the functions of the new file and fail three sections in with "unknown type tag
8". With it, it fails at the header and says which version the file is.

So: **bump to 0.9, and record that the reason is the message rather than the
ambiguity.** The ambiguity rule is about when *not* to bump when nothing
changed; this changes what the section can contain.

Nothing else in the format moves. The `CAPABILITIES` section is untouched -
nothing crosses the broker boundary as a record, so `CapValue` does not learn
about sums, which is the same answer `answers.md` §9 gave.

---

## 9. Trust, and `sic plan`

**Trust.** `from_json` carries the document's label onto the record it builds
(`trust.md` §2). A sum changes nothing: the label goes onto the value, `AS_ARM`
hands back the same arena handle, and the label travels with it. `l.reason` on
an `LLM<Line>` is an `LLM<String>` for the same reason `d.cause` on an
`LLM<Diagnosis>` is. Nothing new can be laundered, because `AS_ARM` produces no
value that was not already in the register it read.

One thing does need saying rather than assuming: `l.reason == "build-finished"`
on a labelled value is a comparison, which `trust.md` §2a already permits -
it answers a plain `Bool` *about* a labelled value rather than handing back a
value of its operand's own kind. So a program may branch on the discriminant of
a model's answer, and that is the same channel `len` and `xs[i]` opened, argued
in the same place.

**`sic plan`.** A plan prints a step for every `FROM_JSON`, taking the name from
`program.type_name` - measured, on the program in §1:

```text
  main
    1. VERIFY   Finished   ; 8:23
```

A sum prints its own name, `VERIFY Line`, which is a true and complete claim:
the document was checked against the whole declaration, arm and all. `AS_ARM` is
not a capability call and adds no step, which is right - it performs no effect
and reaches nothing. `answers.md` §7's rule, that a plan must not make an
undeclared thing look checked, is satisfied without a change, because a closed
sum of closed records is fully checked.

That last clause is load-bearing and is where §10 comes back: a sum whose arms
are **open** records is a weaker claim, and whatever #76 decides for a record it
must decide identically for an arm.

---

## 10. #76 and #78

### #76 comes first, and this document is the argument for it

The issue says the two compose and that a sum of closed records needs no
openness. §1 measured that, and the measurement says otherwise:

| arm | as a closed record | why |
|---|---|---|
| `build-finished` | writable today | two fields, both declarable |
| `compiler-artifact` | not writable | `executable` was `null` in every measured line |
| `compiler-message` | not writable | its `message` field has a key named `$message_type`, which the lexer refuses; that field's `spans[].expansion` is a cycle through a record, which E0340 refuses; and five of its leaves were `null` |

Openness answers all four of those by the same route: a field that is not
declared is not checked, so it need not be nameable, need not terminate, and
need not be non-null. **A sum type landing on its own does not make the
motivating program compile.** #76 landing on its own does, for a program that
wants the discriminant and a subset of the fields, which is what a reader of a
protocol usually wants.

They compose in the direction the issue says. They do not compose in the order
the issue's numbering suggests.

`trust.md` §5 refused `Secret<T>` because "adding the type now would mean adding
a type nobody can construct, which is the kind of speculative structure this
project is arranged to avoid". A sum type built before open records is not quite
that - `build-finished` constructs one - but it is close enough that the order
should be the other way round, and cheap enough to say so.

### #78 does not dissolve into this, and the discriminant is why

Both issues raise it: "present or absent is the smallest union", so optional
fields might be two arms of a sum. Under this design they cannot be, and the
reason is exactly the thing that made the design small.

An arm is selected by the value of a declared field. `compiler-artifact` with an
`executable` and `compiler-artifact` without one **have the same `reason`**.
There is no field whose value distinguishes them, so they cannot be two arms of
a discriminated union. They could be two candidates of a try-until-one-fits
union, which §5 refused for reasons that have nothing to do with #78.

So the answer to #78's question 2 is **no**: #77 does not subsume it, #78 keeps
its own argument, and the two are independent. Cargo's stream needs both, and it
needs #76 more than either.

The whole ordering, then:

| | what it unblocks | needs |
|---|---|---|
| #76 open records | reading any JSONL protocol at all, cargo's included | nothing |
| #77 this document | one declaration for a protocol, one parse per line, a checked relationship between the branch and the fields | #76, to be worth having |
| #78 optional fields | a value that is genuinely sometimes absent - `executable`, `parent` | nothing, and not this |

---

## 11. The smallest useful version

Everything above, minus three things that can be added later without changing
what has been built:

- **No struct literal for an arm.** `Artifact { ... }` is refused. The
  discriminant is set from a declaration, so the only producers are `FROM_JSON`
  and `AS_ARM`. This is what keeps `fields()` honest (§7) and it costs a
  program nothing, because a protocol's messages are read rather than written.
- **No widening.** There is no way to turn an `Artifact` back into a `Line`.
  Nothing has asked for one.
- **No sum as an `agent` output.** `driving.md` §5 has the declaration tell the
  model what shape to answer in, and `Types::shape` would render a sum as
  `{...} | {...}`. That is easy and it is the wrong feature: a protocol's
  producer chooses the discriminant, and asking a model to choose its own is a
  different question with a different argument. `agent { output: Line }` is
  refused with a diagnostic that says so.

What is left is a type that can be declared, parsed into, branched on, taken
apart, and shown to a person. That is the whole of what the issue asked for.

### Units of work

| # | Unit | Done when |
|---|---|---|
| 1 | `enum` declarations: grammar, arm records, name resolution | two arms with the same discriminant value are refused, and an arm redeclaring the discriminating field is refused |
| 2 | `Type::Sum` in the checker; `l.reason` resolves and nothing else does | a field access on a sum that is not the discriminant names the discriminant in the message |
| 3 | `l as Arm`: parsing at the postfix level, checking that the arm belongs to the sum | `l as Diagnosis` where `Diagnosis` is a plain record is a compile error |
| 4 | `TypeDesc::Sum`, tag 8, encode and decode; `VERSION_MINOR` to 9 | a 0.8 reader refuses the file at the header |
| 5 | The compiler lays the discriminant at position 0 of every arm | a disassembly shows the arm's own fields shifted by one |
| 6 | `AS_ARM`, its verifier rule, and `GET_FIELD` on a sum - with `fields()` still `None` for one | a hand-built `MAKE_OBJECT` naming a sum type is refused by the verifier |
| 7 | `FROM_JSON` arm selection, and `TO_JSON` arm lookup | a document whose discriminant matches no arm names the field and the value it found |
| 8 | `AS_ARM` at run time, failing the run at a named line | the failure names a line, and `approve` of a sum shows the arm's own fields |

Eight units, one opcode, one type tag, no change to `Value`, the arena, the
checkpoint format, the capability manifest or the verifier's lattice.

---

## 12. Deliberately not in this

- **`match`.** The issue asked for the smallest thing that makes a sum type
  usable, and `if l.reason == ... { l as Arm }` is it. A `match` with bindings,
  guards and exhaustiveness is its own document, and the language has no pattern
  to bind with anywhere else - not in `let`, not in `for` (`v0.1.md` §2: "There
  is no pattern to destructure with").
- **Narrowing on the branch**, which would make `as` unnecessary and
  exhaustiveness checkable. §3b priced it: per-edge states in `check_data_flow`
  rather than per-instruction, a fact relating a `Bool` to the register it was
  about, and an invalidation rule. It is the right thing to build second and the
  wrong thing to build first, because until a program is written with `as` there
  is no evidence about how often the repetition actually hurts.
- **Fields shared by every arm, beyond the discriminant.** The journal's own
  lines share six. The layout rule generalises without a change in kind - a
  common prefix instead of a single field at position 0 - but nothing has asked
  for it, and one field is what makes the branch possible at all.
- **Sums of anything but records.** `Int | String` has no field to discriminate
  on. Every protocol this is for is a union of objects.
- **Nested sums.** An arm whose field is itself a sum works, because it is just
  a type index; a sum whose *arm* is a sum does not, and nothing needs it.
- **Generic types.** `Option<T>` and `Result<T, E>` are what a sum type usually
  arrives to enable. `v0.1.md` §2 left generic definitions out, and a sum of
  named concrete shapes needs no type parameters.
- **`answers jsonl of T`.** `answers.md` §3 refused a typed grant clause for
  four reasons, and only three of them were missing language features. The
  fourth - that the check would have to cross the broker boundary - is untouched
  by this. Sum types make the *program* able to read cargo's stream; they do not
  make the *broker* able to check it.
- **Anything about the wire.** Nothing crosses the capability boundary as a
  record, so `CapValue` learns no new case.
