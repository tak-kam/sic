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

`budget` is a count of capability calls the agent may make in a whole run.
Exceeding it fails the run. It is enforced by the VM, which keeps a count per
call site: a budget is attached to a pc in the policy table, so the VM enforces
it **without knowing that some call sites are agents**. The count travels in
checkpoints, because otherwise resuming would hand the run a fresh allowance.

Tokens and cost need the broker to report them, which needs a capability result
richer than one value, and that is a later phase - counting calls is what can be
enforced honestly today.

Tools, memory and execution history from section 17 of the specification are not
in this phase. An agent that can call tools is an agent that can loop, and a loop
whose stopping condition is a model's output needs the budget work to be more
than a call count first.

---

## 7. What the journal records

```text
BudgetConsumed { kind, amount, remaining }
```

and nothing else that is specific to agents, which is a change from the first
draft of this design. `AgentStarted` and `AgentCompleted` would have to be
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
about what a type requires rather than about what a document may carry; sum
types (#77); any way to reach the fields that were ignored; and any change to
the default.

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
