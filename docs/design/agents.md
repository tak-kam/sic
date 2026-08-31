# Agents and structured output (phase 7)

An agent is not a function that returns a string. What comes back from a model
is text; what a workflow needs is a value it can branch on, pass to a capability,
and be type-checked against. Everything in this phase exists to close that gap.

```text
agent → prompt → model → raw text → JSON → schema check → typed value
                                             ↑
                                    this is where a run fails,
                                    not three steps later
```

The phase is large, so it is built in two parts:

- **7a, structured data**: `type` declarations, objects, lists, field access, a
  JSON parser, and schema validation. Nothing about models.
- **7b, agents**: the `llm.invoke` capability, `agent` declarations, budgets,
  and the events that record what an agent did.

7a is worth having on its own - a capability that returns JSON needs exactly the
same machinery - and 7b is small once it exists.

---

## 1. User-defined types

```text
type Evidence {
    source: String,
    detail: String,
}

type Diagnosis {
    cause: String,
    confidence: Float,
    evidence: List<Evidence>,
}
```

- Declarations are collected before bodies are checked, so order does not
  matter and two types may refer to each other's names.
- A type may not contain itself, directly or through another type: a value of
  it would have no finite size. `List<Self>` is fine, because a list is a
  handle.
- Fields are ordered. The order is what the bytecode uses; the source uses
  names.
- A body may end with `..`, which says the type describes part of a document
  rather than all of it. Only `from_json` reads it; §8 argues it.

### What a confidence should be declared as

`Float`, and since #85 that is advice rather than decoration.

The type above has shown `confidence: Float` since this document was written,
and until #85 nothing could be done with one: `Float` accepted no operator, so
a program that wanted a threshold could not write it against the field it was
about. What programs did instead is in `workflows/harness.sic`, which declares
`confidence: Int` and asks the model for a percentage - a workaround that
changed the question put to the model in order to fit the checker, which is the
wrong way round.

`Float` now orders (`v0.1.md` §4), so the threshold is written where it is
read:

```text
let d = diagnose(logs);
if d.confidence < 0.5 {
    log warn "the model is not confident in this";
}
```

The label does not stop it. A comparison answers a `Bool`, and a `Bool` is
never one of the values it was given, so `d.confidence < 0.5` on an `LLM<Float>`
is allowed by the rule `trust.md` §2a already had rather than by an exception
made for this.

A percentage in an `Int` is still a legitimate schema when per cent is what is
being asked for. It is no longer the only one that compiles, and a schema
should now be chosen for the question it puts to the model. What stays refused
is `==` on a `Float`: a score is not a value a program can name exactly, and a
threshold is the shape that question has anyway.

### Constructing one

```text
let e = Evidence { source: "syslog", detail: "disk full" };
```

`IDENT {` is ambiguous with the block after `if`, exactly as it is in Rust, and
the fix is the same: a struct literal is not allowed in the condition of an
`if`. Writing `if (Point { x: 1 }).x > 0 { }` still works, because the
parenthesis makes it unambiguous. The alternative - a call-like
`Evidence(source: ..)` - avoids the ambiguity but invents a second call syntax,
which is worse.

### Reading a field

```text
let cause = diagnosis.cause;
```

Field access already parses; phase 3 gave it the "not supported" diagnostic and
used the syntax for capability calls. A `base.name` is now: a capability call if
`base` is a capability namespace nothing shadows, and a field access otherwise.

---

## 2. Lists

```text
let xs = [1, 2, 3];
let first = xs[0];
let n = len(xs);
```

- A list literal is homogeneous; its type comes from its elements. An empty
  literal needs an annotation, because there is nothing to infer from.
- Indexing is a postfix operator. An index out of range fails the run: there is
  no option type to return instead, and a silent default would be worse.
- `len` is the built-in function this phase adds, and what it settles is that a
  built-in is a name rather than a method: `xs.len()` would mean a method
  namespace, and there was nothing else to put in one. `contains` and
  `starts_with` took the same route later, which is why it was worth settling.

There is no iteration in this phase. Adding `for` here would have been adding a
control structure to a phase about data, so it was left for one of its own:
issue #66, and `v0.1.md` §2 and §5a. Nothing about a list of answers changed
when it arrived - a loop reaches an element by the route `xs[i]` already
reached it by, provenance and all.

---

## 3. Objects and lists at run time

```rust
enum Value {
    // ...
    Str(Handle),
    List(Handle),
    Object(Handle),
}
```

The arena grows two more stores beside its strings. A handle keeps meaning only
inside its arena, which is already true of strings and is what makes a
checkpoint's arena travel with its registers.

Four instructions:

```text
MAKE_OBJECT a, b, c   ; R[a] = an object of type T[b] from R[c .. c+fields]
GET_FIELD   a, b, c   ; R[a] = R[b].field[c]
MAKE_LIST   a, b, c   ; R[a] = a list of R[b .. b+c]
GET_INDEX   a, b, c   ; R[a] = R[b][R[c]]
```

Fields are addressed by position, not by name: the compiler knows the layout,
and a verifier that had to compare names would be doing the type checker's work
again. The type section gains object and list descriptors so it can still say
what `GET_FIELD` produces.

---

## 4. JSON

A model answers with text. Turning that into a value takes a parser, and the
parser has to be ours - `serde_json` is exactly the kind of dependency this
project is arranged to avoid, and JSON is small enough to write.

`sic-json` holds a parser and a value type. It performs no I/O and reads no
clock, like `sic-journal`, and has the same isolation test.

It also writes the other direction's leaf: `quoted` and `write_quoted`, which
turn a string into a JSON string. That is not a serializer, and is not meant to
grow into one - every document this workspace writes is a fixed shape built by
the code that owns it. Escaping is the one part they all needed, and it is a
rule of the format rather than of any of them, so it belongs to the crate that
owns the format. Four crates carried a copy of it before it did.

What it accepts is RFC 8259 and nothing more: no trailing commas, no comments,
no `NaN`. A model that produces those has produced invalid JSON, and saying so
is more useful than guessing.

That includes the encoding: §8.1 says a document is UTF-8, so `parse` takes a
`&str` and the type carries the rule - bytes that are not UTF-8 are refused
where they become text, which for a program's answer is the broker. Parsing
itself is done in bytes, since the structure of JSON is ASCII; a character is
decoded only to name one in a message. Casting the byte instead names a
character the document does not contain - `u8 as char` is Latin-1, so an answer
beginning `そ` was reported as `ã` - and what a model answers with is the one
input where non-ASCII is guaranteed.

Limits, because the input is untrusted text from a model:

- a nesting depth cap, so a deeply nested document cannot exhaust the stack
- a length cap on the whole document
- duplicate keys are an error rather than last-wins

### Schema validation

```text
raw text → parse → validate against a type → Value
```

Validation is a separate step from parsing, and it works against the type
section already in the bytecode. Failing here is a normal run failure with a
message that names the path: `evidence[2].source: expected String, found number`.

The VM does this, not the broker: the value is built in the VM's arena, and the
broker must not know about types.

One new instruction:

```text
FROM_JSON   a, b, c   ; R[a] = the value of type T[b] parsed from the string R[c]
```

---

## 5. The `llm.invoke` capability

```text
llm.invoke(prompt: String, shape: String) -> String
```

The second argument is the shape the answer has to take, and it may be left off
- a direct call that wants prose passes nothing. An `agent` fills it in from its
own `output` type, because that declaration is the only place the shape is
written down, and whoever answers has to be told: see
`docs/design/driving.md` §5.

The broker **defers** it, as it does `human.approve`. Calling a model means
HTTPS, which means TLS, which is not something to write by hand for this - and
the deferred mechanism already exists and is the right shape: the run suspends,
something outside answers, the run continues. A checkpoint means an answer can
arrive minutes later or in another process.

The grant's constraint names the model:

```text
allow {
    llm.invoke "claude-opus-4";
}
```

A later phase can add a broker that speaks HTTP, without the language changing.

---

## 6. `agent`

```text
agent diagnose {
    input: String,
    output: Diagnosis,
    budget: 8,
}

fn main() -> String {
    let d = diagnose(logs);
    return d.cause;
}
```

An agent declaration is a **function the compiler writes**. `diagnose(x)`
becomes:

```text
CALL_CAP  llm.invoke(prompt, shape) ; the shape comes from `output`
FROM_JSON Diagnosis            ; parse and validate the answer
```

So an agent is not a new kind of callable, and nothing in the VM knows what an
agent is. What the declaration buys is that the output type is declared once, in
one place, and the run fails at the model boundary rather than wherever the
malformed value is first used.

`input` is `String` in v0.1: building a prompt from a value would need a way to
render one, which the language does not have yet.

`budget` is a count of capability calls the agent may make in a whole run, and
the agent is the whole of it: an agent called from two places has two call
sites and one allowance of `budget` between them. Exceeding it fails the run.

It is enforced by the VM, which keeps a count per allowance rather than per
call site. A policy entry says how many calls an allowance is worth and which
allowance its site spends from, so the VM enforces the bound **without knowing
that some call sites are agents** - it counts against the number the table
gives it, and the compiler is what decides which sites share one. The count
travels in checkpoints, because otherwise resuming would hand the run a fresh
allowance.

### It used to be per call site, and #84 settled it the other way

The two readings are the same sentence for an agent called from one place and
nothing alike for any other. Per site, `budget: 3` on an agent used twice
enforced six; splitting one retry function into two doubled what a harness
could spend, and neither the source, the declaration nor the plan's per-line
text said so.

What decides it is that **the budget can only be written on the declaration**.
There is no syntax for a budget at a call site, so a per-site bound is one the
language gives no way to say: to declare "three from here and three from
there" you write one number and get six, and to declare "three in total" you
write nothing that means it. The number a person approves would then be a
number the shape of the program chose, which is what `harness.md` §2 means by
a bound nobody approved - a refactor must not move a safety bound.

The case for per site is that it is cheaper, and it is cheaper by less than it
looks. Which sites share an allowance is settled at compile time and written
into the policy table, so `sic plan` groups them by reading a word rather than
by analysing a call graph, and the VM keys a counter by that word rather than
by a pc. Per site is also said to be the right answer for a recursion; it is
not a distinguishing case, because a recursive function that calls an agent is
one site and both readings charge it the same.

`VERSION_MINOR` moves to 12. A policy entry carries the allowance in a word
after the budget, and a reader that did not know would take it for the
conversation and every field after it for the one before - the case a section
layout change is bumped for. The verifier refuses a file whose budget names no
allowance, or whose two sites share one and disagree about how large it is,
because the VM reads the size from whichever entry it reached.

The cost is that a site's line in `sic plan` cannot say what the site's own
bound is, because a site no longer has one. So the line says whose it is -
`at most 3 in a run, shared by 2 sites` - and the plan prints each allowance
once with the sites under it. A plan must not print a number that reads as the
other reading, and the summary's arithmetic used to be the only place the truth
appeared.

Tokens and cost need the broker to report them, which needs a capability result
richer than one value, and that is a later phase - counting calls is what can be
enforced honestly today.

Tools, memory and execution history from section 17 of the specification are not
in this phase. An agent that can call tools is an agent that can loop, and a loop
whose stopping condition is a model's output needs the budget work to be more
than a call count first.

---

## 6a. `retry`: asking again when the answer does not fit

The most characteristic thing a harness does is ask again because the answer
did not fit, and until #83 that was the one thing this language could not
express. `agent` validated and refused `retry`; `llm.invoke` could be retried
and its answer could not be validated. The two halves were in different places
and there was no way to put them together.

```text
agent propose {
    input: String,
    output: Fix,
    budget: 3,
    retry: 3,
}
```

`retry: N` is a total, not extra attempts, which is what `retry N` after a
capability call has always meant. `retry: 1` and no `retry` are the same thing,
and that is what every agent written before this meant.

### Why it is a field of the declaration

`retry N` is a word after a call everywhere else in the language, and here it is
not. The reason is #84's, applied to a second number: **a bound a person
approves must not depend on how many places call the thing it is written on.**
A retry at the call site would be multiplied by a refactor that moved nothing,
and there would be no way to say "three attempts at this agent's answer, wherever
it is asked from" - which is the only thing anybody means.

It also puts the four numbers that bound an agent in one place, which is what a
reader of a declaration is entitled to: `budget` for model calls, `retry` for
asking again, `tools` for tool uses, `deadline` for wall clock. E0330 still
refuses `retry` after an agent call and its note now names the field.

`retry: N` with N above one needs the grant to say `repeatable`, which is the
same claim `retry N` after any capability call needs, refused with the same
E0374. It matters more here than there: an agent with `tools` acts while it
answers, so a rejected answer can have left work behind it.

### Where it is enforced, and why not at `FROM_JSON`

The check is in `Vm::resume`, the one place every driver's answer arrives -
`--llm`, `sic attach`, a replay, a resumed checkpoint, the isolated process.
Not at the `FROM_JSON` two instructions later, for a reason that decides the
whole shape: **by the time `FROM_JSON` runs, the call it would ask again is
gone.** The pending call is dropped the moment the answer lands in a register.

So the policy entry carries the type as well as the count, and `resume` parses
the document against it while the call is still in hand. `FROM_JSON` still runs
and is still what turns the document into a value; the earlier parse decides
only whether to ask again. That means the document is parsed twice, and the
alternative - a second implementation of `build_from_json` that answers yes or
no without building anything - is two procedures that must agree about what fits
a type and would disagree quietly, in the direction that lets a bad document
through. A model call took seconds.

A run that is out of attempts fails exactly where it failed before any of this
existed: at the shape, with the message `FROM_JSON` gives, at the pc the debug
section maps back to the agent call. Nothing was bolted on to catch it - there
is still no `Result`, no exception and no fallible `from_json` a program can
write, and #77 stays unspent. **A validation failure is still not a value.**
What changed is that the runtime may ask again before it becomes one.

### A rejected answer is charged to the budget

It has to be. A model that answers badly three times has been called three
times, and a bound that counted only the successes would not be a bound. So the
budget is the harder of the two numbers and wins: `budget: 2, retry: 3` gets
two attempts and a budget error, because the budget is the number a person
approved and the retry is a ceiling under it.

A transport retry - the broker could not answer at all - is **not** charged, and
the difference is not an oversight. A call that produced no answer produced no
model work to bound, and `attempts` is already what bounds how many times that
may be tried. The budget counts answers.

The two had never met before this: `budget` could only be written on an agent,
`retry N` could only be written on a call that was not one, so no site in any
program had both. This is the first thing that puts them on one instruction.

### What the retry says

The natural retry prompt is "your last answer was rejected because X", and
`harness.md` §5.4 found that a program cannot build it: the rejected answer is
`LLM<String>`, the reason comes from the type section, and `+` refuses to join
two provenances. That is `trust.md` being right.

The runtime is not the program, and may write it. The request carries `rejected`
- the sentence `FROM_JSON` would have printed - and the broker appends it to the
prompt the program wrote:

```text
why did it fail?

The last answer did not fit: confidence: expected Int, found a string
Try again.

Reply with JSON of this shape, and nothing else:
{"change": string, "confidence": integer}
```

The program's prompt is untouched; the runtime's half is appended, so a reader
of a transcript can see which is which. It travels in the checkpoint, because a
run that stops between two attempts is written out while it waits.

`memory: task` is still worth having and is now doing a different job: it is
what lets the model see its own rejected answer, where this sentence tells it
which part of it was wrong. Neither replaces the other, and an agent without
`memory` is now told enough to try again.

### What `sic plan` says

The line reads `at most 3 in a run  at most 3 attempts at an answer that fits`,
which is deliberately not the `3 attempts each` a transport retry prints. Since
every attempt comes out of the allowance named on the same line, the number
above it is the whole of what the site may do and there is nothing to multiply
it by. The summary total is unchanged for the same reason.

---

## 7. What the journal records

```text
BudgetConsumed { kind, amount, remaining }
AnswerRejected { cap, result, error, attempt }
```

and nothing else that is specific to agents, which is a change from the first
draft of this design.

`AnswerRejected` arrived with #83 and is deliberately not a `CapabilityFailed`:
the broker did its job and the model replied, and an account that called that a
failure of the call would hide what a reader of a bad run most needs to see.
The digest is the answer, so a recorded run's values file holds the document
itself; `error` is what `FROM_JSON` would have said. It is followed by another
`CapabilityRequested` when an attempt is left, and by nothing when there is not.
The metric is its own, `sic.capability.answers_rejected`, for the same reason:
an operator watching a model that started answering badly does not want a
broker that could not connect in the same line. `AgentStarted` and `AgentCompleted` would have to be
emitted by something that knows what an agent is, and the whole point of the
lowering is that nothing below the checker does. An agent's work already appears
as what it is: a function activation, a capability request and completion, and -
if the answer does not fit - a schema failure at the pc the debug section maps
back to the agent call.

Agent-specific events become worth their cost when an agent is more than one
call: a loop with tools, where "the agent started" and "the agent finished" are
not the same as "one capability call happened".

`remaining` is on the budget event so that a budget is visible while it is being
spent rather than only when it runs out.

---

## 8. Not in this phase

- **No HTTP and no TLS.** The `llm.invoke` capability defers; something outside
  answers it. Writing a TLS stack by hand is not the kind of dependency-freedom
  this project is after.
- **No tool calls and no agent loops.** They need a budget that counts more than
  calls, and a way for an agent to decide it is done.
- **No retry on a *semantic* judgement.** `retry` asks again when the document
  does not fit the type. An answer that parses and that the program dislikes -
  a confidence below a threshold - is a value, and asking again about it is a
  loop the program writes, as `workflows/harness.sic` does. The two failures are
  different and only one of them is the runtime's to see.
- **No memory or execution history on an agent.** Both are state that outlives a
  run, which is a question about where state lives, not about agents.
- **No token or cost budgets.** The broker would have to report them, which
  needs a richer capability result.
- **No iteration over lists.** Not in this phase; recursion was how a program
  repeated, and `for x in xs` arrived later, on its own argument (issue #66).
- **No optional or nullable fields.** Every field of a type is required, so
  validation is a yes or no. Optionality is a type-system feature and belongs
  with the trust types of section 19.
- **A record is closed.** A document carrying a field the type does not declare
  does not fit it. That is the rule the rest of this section is about.

### A type may say it describes part of a document

The closed record is right about a model and wrong about a protocol, and one
validator now has both jobs. A model was told what its answer had to look like,
so an answer with a field the type does not declare is an answer to a different
question, and refusing it is the whole value of the declaration. A machine
protocol is the other way round: cargo's JSONL lines carry nine, five and two
keys and share only `reason` (`docs/design/answers.md` §3), so no declared
subset of them validates, and the day a protocol grows a field every reader that
refused an unknown one breaks. Forward compatibility is the reason protocols are
built that way, and a validator that cannot read them is not being strict, it is
being unusable.

So a type may say which of the two it is:

```text
type Line { reason: String, .. }
```

`from_json` then checks the fields the type declares and ignores the rest. **A
type without the marker is unchanged**, and that is the default because the
model case is the common one and its refusal is load-bearing.

**The marker is on the type, not on the call.** The alternative was a mode on
`from_json`, and it is more flexible; nothing has asked for the flexibility, and
it costs the thing the marker is for. Whether a document may carry more than the
type names is a property of what the type describes - `Diagnosis` is an answer
somebody was asked for, `Line` is a message somebody else designed - and it does
not change between two calls. On the call, two `from_json`s of the same type
could disagree about it, a reader of the declaration could not tell which
without finding every call, and `sic plan` would have to print the answer per
call site rather than once. On the type it is one word, read where the type is.

**Openness stops at the type that declares it.** A field whose type is a record
is checked by that record's own rule, so an open `Line` holding a closed
`Target` still refuses a document whose `target` carries an undeclared key, and
the message names the path:

```text
error: the document does not fit the type: target: `Target` has no field `kind`
```

Reaching into nested types would mean a reader of `Target` could not tell what
it accepts without finding every type that mentions it, which is the same thing
that ruled out putting the marker on the call.

**A value holds the declared fields and nothing else.** The rest of the document
is read, checked to be well-formed JSON, and dropped: there is no way to get at
it afterwards, and `..` is not a container. That is what makes this small enough
to be worth having, and it is also why an open `Line` does not solve the sum
type the same protocol needs (issue #77) - it tells a program which kind of line
it has and gives it no way to read the rest.

`TO_JSON` follows from that and needs no rule of its own: it writes the value,
so an open type's value writes back as the fields it declares. The one place
this is visible is `approve`, which shows a person the value rather than the
document - a `Line` read out of `{"reason":"build-finished","success":true}` is
shown as `{"reason":"build-finished"}`. That is honest about what the program
has, and it is worth knowing that `..` is the one marker that puts something in
a document a person approving will not see.

**`sic plan` says so, because it is a weaker claim.** A validation of an open
type checked part of a document and did not look at the rest, and a plan that
printed it the same way as a closed one would make an unchecked thing look
checked - which is the argument `answers.md` §7 makes about a grant that
declares no shape at all.

```text
    1. VERIFY   Line  (declared fields only)   ; 8:22
```

The asymmetry with `(not pinned)` is deliberate and is not an oversight of the
same rule. A grant that says nothing about a digest is the common case, so
silence there had to be spelled out or a reader would read it as a pin; a type
is closed unless it says otherwise, so a reader who has never seen `..` reads a
bare `VERIFY Line` and is right about it. The negative is printed where silence
would mislead, and here it does not.

**The trust label is untouched.** `from_json` takes a document's label off to
check the argument and puts it back on the result (#72), and that is about where
the document came from, while `..` is about what the document may contain.
Neither has anything to say about the other: `LLM<Line>` through an open type is
still `LLM<Line>`, and `line.reason` still cannot decide what the next program
runs. Said here rather than assumed, and checked by a test rather than said.

The flag has to survive the compile, because `FROM_JSON` runs against the type
section, so a record descriptor in the `.sicb` gained a byte and
`VERSION_MINOR` went from 8 to 9. A new instruction has twice been judged not
to need a bump - an old reader meets an unknown opcode and says so - but a
changed section layout is the other case, where a reader that did not know would
take the flag for the field count and decode a type section that happens to
parse.

Deliberately not in this: optional fields (#78), which are a different question
about what a type requires rather than about what a document may carry - the
section below is where that one was taken; sum types (#77); any way to reach
the fields that were ignored; and any change to the default.

### A field may say it is sometimes not there

`..` let a program **ignore** a field a protocol sometimes sends. It gave it no
way to **read** one, and that is the rest of the same problem: the sentence
above says every field of a type is required, so a document that leaves one out
does not fit, and a program that wants the value has to declare a field that is
not always there.

**Half of the refusal has aged and half has not.** "Validation is a yes or no"
is the good half and is untouched below: a document either fits a type or does
not, and what changes is which documents fit rather than whether the question
has an answer. What has aged is "optionality is a type-system feature and
belongs with the trust types". Trust types landed and are *erased* before the
bytecode (`trust.md` §4), because the rule they enforce is about which programs
may be written. Optionality cannot be erased - a program has to do something
different when the field is not there - so it is a different kind of feature
than the one it was filed beside.

#### What the protocols actually send, measured

Every line below is from a run, not from an issue. Cargo 1.98 building this
workspace with `--message-format=json`, and a second build of a throwaway crate
with a `dead_code` warning so that a `compiler-message` appears:

```text
14 compiler-artifact   13 with "executable":null,  1 with a path
 1 compiler-message    7 keys whose value is null, in one line:
                       message.spans[0].label
                       message.spans[0].suggested_replacement
                       message.spans[0].suggestion_applicability
                       message.spans[0].expansion
                       message.code.explanation
                       message.children[0].code
                       message.children[0].rendered
```

Three things follow, and the third is the one that decided the design.

- **The motivating case is real.** A library's `compiler-artifact` and a
  binary's are the same shape with the same `reason`, and one of them has a
  path where the other has nothing. Thirteen against one, in an ordinary build.
- **A field that is sometimes not there is not two arms of a sum type**, and
  `alternatives.md` §10 measured why: no field's value separates the two, so
  there is no discriminant, and a try-until-one-fits union was refused there
  for reasons that have nothing to do with this. That is issue #78's question 2
  answered, and it is answered *no*.
- **The key is never absent. It is present and `null`.** Every one of those
  fields is written, with `null` in it, in every line. This repository's own
  journal is built the same way - `"parent":null` at the top of a trace - and
  `crates/sic-journal/src/read.rs` already reads the two cases with one arm:

  ```rust
  let parent = match json.member("parent") {
      Some(Json::Int(v)) => Some(SpanId(*v as u64)),
      _ => None,
  };
  ```

So the question a design here had to answer was not mainly "what does a program
do with an absent field". It was "what does a program do with a `null`", and
the two collapse - which §"`null` and absent are one case" below takes as a
decision rather than as a convenience.

Those seven are of every kind a field can be: strings, a record (`code`), and a
record that is a cycle (`expansion`). That is what ruled out encoding an
optional field as a list of at most one, which needs no new instruction and
would have been the smaller change - `filenames: List<String>?` would then be a
`List<List<String>>`, where `len` means presence rather than length and `[0]`
means the list rather than an element. A trap, in the one protocol the feature
was for.

#### The declaration, and what a program may do with it

```text
type Artifact {
    reason: String,
    executable: String?,
    ..
}
```

Two operations, and between them they are the whole feature:

```text
if a.executable? {        ; whether the document carried it: a Bool
    return a.executable;  ; the value, or the run fails here
}
```

**`a.executable` has type `String`.** Not `String?`, not an option, not a
one-element list. Nothing in this language holds a value that is sometimes
there, and that is what keeps `Option<T>` out rather than smuggling it in under
another name: `?` may be written after the type of a record's field and nowhere
else - not on a `let`, a parameter, a return type, or inside `List<...>` -
which is E0221, and a reader who meets one knows it is about a document.

**Reading one that was not there fails the run, at a named line.** That is the
decision this language has already made twice, and §2 wrote it down the first
time:

> Indexing is a postfix operator. An index out of range fails the run: there is
> no option type to return instead, and a silent default would be worse.

```text
error: the field was not in the document
 --> artifact.sic:8:12
```

What it costs honestly: nothing forces a program to ask before it reads, and
the verifier cannot say it should have. What it does not cost is a value nobody
chose, which is what `a_missing_field_is_a_mismatch_not_a_default` is a test
about and what the sentence at the top of this section protects. The
improvement that removes the cost is narrowing on the branch - "inside this
`if`, the field is there" - and `alternatives.md` §3b measured what that takes:
a per-edge state in the verifier's data-flow pass, a fact relating a `Bool` to
the register it was about, and an invalidation rule. It is deferred here for
the same reason it was deferred there.

**The question is spelled `?` rather than a builtin**, and the reason is that a
builtin taking `has(a.executable)` would have an argument it must not evaluate:
the argument is the read that fails. A postfix operator asks about the field
access it is written on, parses where `[i]` and `.f` parse, and reads in the
one order a person would say it out loud. A keyword - `has a.executable` -
reads as well and costs a reserved word, which every program using `has` as a
name would pay; `?` was a lexer error before this.

#### `null` and absent are one case

A document may leave the key out, or write `null` for it. Both fit an optional
field, and both produce the same value, and that is a decision rather than an
oversight:

| | |
|---|---|
| what the protocols do | write `null`; none of the measured lines omits a key |
| what this repository does | `read.rs` above, in Rust, since before this |
| what it would cost to split them | three states in a slot that holds two, and a second question a program could ask |
| who has asked | nobody |

The one protocol that means different things by them is JSON-RPC, where a
message with no `id` is a notification and one with `"id":null` is not - and
`crates/sic-cli/src/cmd/mcp.rs` distinguishes them, in Rust, today. A sic
program cannot express that, and this section is where that is written down
rather than discovered. The issue that wants it is the one that argues for a
third state; it is not this one.

**E0312 is untouched.** `let x = null;` is still refused, and the note now
points at where a `null` does have somewhere to go rather than at "there is no
optional type yet", which read as a promise. And it is worth recording that a
document's `null` was already readable before this, narrowly and undocumented:
`Unit` is a nameable type, so

```text
type A { reason: String, executable: Unit }
```

fits `{"reason":"...","executable":null}` and has done since phase 7a. That is
why an optional `Unit` field is refused (E0355): the value of an absent
optional field *is* `null`, so a `Unit?` would have no way to tell the two
apart. Refusing it is what makes "absent and `null` are the same thing" true of
the value and not only of the document - which is what lets the runtime carry
this with no new value at all.

#### A value holds `null` where a field was not there, and nothing was added

The slot of an absent optional field holds `Value::Unit`. No variant was added
to `Value`, the arena is unchanged, and the checkpoint format's own version
does not move. The invariant that makes it work is the one E0355 enforces: a
field's declared type is never `Unit` when the field is optional, so the two are
always different values.

Three instructions read a field where there was one, and the split is not for
tidiness:

```text
GET_FIELD      a, b, c   ; may not name an optional field
GET_OPT        a, b, c   ; may name nothing else; fails when the slot is null
HAS_OPT        a, b, c   ; may name nothing else; answers a Bool
```

`GET_FIELD` cannot fail and an optional field can be missing, so the two must
not meet, and **the verifier proves which fields each may name**. That is what
makes the VM's one comparison the whole of the check rather than a guard that
happens to be there: a plain read never reaches a slot that might be empty.
`MAKE_OBJECT` gains the matching rule - an optional field's register holds the
field's own type or `null`, and the message says both.

#### A literal may leave an optional field out

```text
let a = Artifact { reason: "built" };
```

That is not a default and it is not a hole in the rule this section keeps:
nothing was put in the slot, and reading it fails exactly as it does for a
document that did not carry the field. The program declined to give a value; it
was not handed one. A required field left out is still E0350.

#### `TO_JSON` writes `null`, and that is what `approve` shows

An absent optional field is written rather than omitted:

```text
approving: {"reason":"built","executable":null}
```

Both spellings would parse back to this same value, so the round trip does not
decide it. What does is that a person is being shown what the program has, and
an omitted key is indistinguishable from a type that never declared the field.
`null` shows the field and shows it empty. It is also how every protocol this
was built to read writes it, so a document sic produces and the documents it
consumes agree.

This is the second thing §8 has put in front of somebody approving that is not
quite the document: `..` drops the fields the type did not declare, and `?`
writes a key the document may not have carried. Both are the value rather than
the document, which is the rule `TO_JSON` has always followed.

#### A type may now reach itself through an optional field

```text
type Span { line: Int, expansion: Expansion? }
type Expansion { span: Span }
```

That compiles, and §1's rule that a type may not contain itself is unchanged in
what it is for. A list or a task breaks a cycle because both are handles; an
optional field breaks it for a different reason, which is that **every value of
the type terminates** - the chain has to stop at a field that was not there.
`Option<Box<T>>` is the same argument in another language.

It is worth having rather than a side effect worth mentioning, because it is
the last of the four walls `alternatives.md` §1 measured against declaring
rustc's diagnostic: `spans[].expansion` is a cycle through a record, and E0340
refused it. What is left of that list is `$message_type`, which the lexer
refuses and `..` already answers by not declaring the field.

The one thing this required was a `seen` guard on `unshowable`, which walks a
record's fields to decide whether `approve` can show it. Before this a record
could not reach itself, so the walk terminated by construction; now it can, so
it says so.

#### What is not changed, and why each is worth saying

**The trust label.** `from_json` takes a document's label off to check the
argument and puts it back on the result (#72), and a field read carries it
onward, so `LLM<Artifact>.executable` is `LLM<String>` and still cannot reach a
capability that changes something. `a.executable?` answers a **plain** `Bool`,
which is the rule `len`, `contains` and `starts_with` are already covered by
(`trust.md` §2a): a question about a labelled value answers something that is
not any value the label was on, and no `Bool` reaches a capability. Tested
rather than asserted.

**`sic plan`.** A `VERIFY Artifact` gains no qualifier, and the asymmetry with
`(declared fields only)` is deliberate. An open type's line is qualified
because the check *did not look* at the rest of the document. An optional field
was looked at: the validator asked for it, found it absent or `null`, and that
is a check that passed. The claim "this document fits `Artifact`" is exactly as
strong as `Artifact` is, and a reader of the plan who reads the type sees the
`?`.

**The default.** A field is required unless it says otherwise, which is what a
model's answer depends on.

#### `VERSION_MINOR` moves to 11

Every field of a record descriptor gained a byte. A reader that did not know
would take the first field's flag for the second field's name length and decode
a type section that happens to parse, which is the case 9 and 10 were bumped
for. The two new opcodes are **not** that case and would not have justified a
bump on their own: an old reader meets an unknown opcode and says so.

Deliberately not in this: `Option<T>` as a type a program can name; optional
function parameters or capability arguments, which are a question about calls
rather than about documents; defaults of any kind; narrowing on a branch, so
that a guarded read cannot fail (`alternatives.md` §3b prices it); a third
state that tells an absent key from a `null` one; and sum types (#77).

---

## 9. Units of work

### 7a: structured data

| # | Unit | Done when |
|---|------|-----------|
| 7a-1 | `type` declarations, object types, struct literals, field access | a type containing itself is rejected |
| 7a-2 | List types, literals, indexing, `len` | an empty literal without an annotation is rejected |
| 7a-3 | Objects and lists in the arena and in checkpoints | a checkpoint round-trips a nested value |
| 7a-4 | `MAKE_OBJECT`, `GET_FIELD`, `MAKE_LIST`, `GET_INDEX`, and their verifier rules | the verifier knows what `GET_FIELD` produces |
| 7a-5 | `sic-json`: parser, limits, isolation test | trailing commas and duplicate keys are refused |
| 7a-6 | `FROM_JSON` and schema validation | a mismatch names the path that failed |

### 7b: agents

| # | Unit | Done when |
|---|------|-----------|
| 7b-1 | `llm.invoke`, deferred by the broker | a model call suspends the run and resumes from a checkpoint |
| 7b-2 | `agent` declarations, lowered to a capability call and a validation | nothing in the VM knows what an agent is |
| 7b-3 | Budgets as a call count, enforced per call site | exceeding one fails the run, and the count survives a checkpoint |
| 7b-4 | The budget event | a budget is visible while it is spent |
