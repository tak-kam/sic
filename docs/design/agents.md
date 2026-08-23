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
- `len` is the one built-in function. Adding methods would mean adding a method
  namespace, and there is nothing else to put in it yet.

There is no iteration. v0.1 has no loop of any kind - recursion is how a program
repeats - and adding `for` here would be adding a control structure to a phase
about data.

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
- **No iteration over lists.** v0.1 has no loops at all; recursion is how a
  program repeats.
- **No optional or nullable fields.** Every field of a type is required, so
  validation is a yes or no. Optionality is a type-system feature and belongs
  with the trust types of section 19.

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
