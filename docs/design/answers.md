# What shape a program answers with

`docs/design/output.md` gave a program a way to be heard. This is the question
it left open: **in what form.**

```sic
fn ran_but_failed(output: Observed<String>) -> Bool {
    return contains(output, "test result: FAILED");
}
```

That is `workflows/ci.sic` today, and it is the sharpest kind of debt this
project can carry. `sic plan` is complete about the call - the binary, the
arguments, the directory, every environment variable - and silent about the one
sentence the workflow's correctness actually rests on. Change cargo's wording
and a test failure is reported as a build failure: the bytecode verifies, the
tests pass, and the plan prints the same document it printed yesterday.

So a grant should be able to say what form a program answers in, and a plan
should print it. This document decides what that can honestly claim, and
measurement moved it a long way from where issue #75 started.

---

## 1. What the motivating program actually emits

The issue's premise is that `cargo test --message-format=json` gives the
workflow a machine format, and that the `contains` above is therefore a debt
taken rather than forced. That premise was measured, on cargo 1.98, against
both of the failures `ci.sic` has to tell apart. It is false.

**When the build fails**, stdout is JSONL and nothing else:

```text
{"reason":"compiler-message","package_id":"...","message":{...}}
{"reason":"compiler-message","package_id":"...","message":{...}}
{"reason":"build-finished","success":false}
```

**When the build succeeds and a test fails**, stdout is JSONL *and then prose*,
on the same stream:

```text
{"reason":"compiler-artifact","package_id":"...","executable":"..."}
{"reason":"build-finished","success":true}

running 1 test
test tests::fails ... FAILED
...
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

`--message-format` formats *the build*. The test harness is a second program
cargo starts, and its output is appended to the same file descriptor,
unformatted. The harness does have a JSON mode - `--format json` - and it is
`-Z unstable-options`, refused on stable with `the option 'Z' is only accepted
on the nightly compiler`.

Two consequences, and both are load-bearing:

- **The distinction `ran_but_failed` needs is available in JSON.**
  `{"reason":"build-finished","success":true}` with a non-zero exit means the
  build worked and something after it did not. That is exactly the branch.
- **The output as a whole is not JSONL**, in the case the workflow exists for.
  A grant claiming `jsonl` for that call would be refused on every run where
  the tests ran at all.

So the debt in `ci.sic` was not avoidable. It was forced, by cargo, and the
issue's strongest sentence - "the workflow greps prose from a program that
offers a documented machine format" - does not survive contact with the
program. §9 says what follows for that workflow, and it is not what the issue
expected.

None of this makes the feature not worth building. It makes the claim smaller
and true.

---

## 2. A gradient, and the loose rung is the default

A grant says as little or as much as it can honestly say:

```sic
allow {
    fs.read "./manifest.json" answers json;

    process.run "/usr/bin/cargo" args ["metadata", "--format-version", "1"]
        answers json
        in "/home/me/project";

    process.run "/usr/bin/cargo" args ["test", "--no-run", "--message-format=json"]
        answers jsonl
        in "/home/me/project";
}
```

| rung | what the grant claims |
|---|---|
| nothing | the output is text, and the program will do what it can with it |
| `answers json` | the whole output parses as one JSON document |
| `answers jsonl` | every non-blank line parses as one JSON value |

The order matters and the cheap rung is meant to be the one a program reaches
for. `answers json` costs a program nothing it was not already doing: the
output is still `Observed<String>`, and `let m: Config = from_json(text)` is
the same line it was before. What changes is that the manifest now says the
text is JSON, the boundary checks it, and a reader of the plan is told.

A grant that says nothing keeps meaning what it has always meant. This is not a
default that programs must opt out of.

There was a third rung in the issue - `answers jsonl of CargoMessage`, the
lines parse *and* fit a declared type - and §3 refuses it.

---

## 3. The typed rung is refused, and the measurement is the reason

The argument for it was a good one, and it is worth stating before it is
answered. `agent { output: Diagnosis }` already declares the shape of a model's
answer, and `from_json` already refuses one that does not fit
(`docs/design/agents.md` §6). A program's stdout and a model's answer are both
text somebody else produced that this program has to believe, and one of them
is checked. `AgentDecl` already carries `output: Option<TypeExpr>`, so a
declaration naming a type from something that is not a function is precedent
rather than new ground.

**It does not survive the motivating case, and there are four independent
reasons, any one of which would be enough.**

The measurement first. The 42 real cargo JSON lines above are three shapes,
and these are their top-level keys:

```text
compiler-artifact  9  executable features filenames fresh manifest_path
                      package_id profile reason target
compiler-message   5  manifest_path message package_id reason target
build-finished     2  reason success
```

**The only key all three share is `reason`.** The stream is a sum of shapes
that agree on a discriminating field and on nothing else, which is how every
JSONL protocol worth reading is built - and it means the largest closed record
that could describe every line has one field, which then fails anyway because
the other eight are extra.

Three of the reasons are features `sic` does not have, each refused elsewhere
with an argument of its own:

| what `CargoMessage` would need | where it is refused |
|---|---|
| **a sum type**, since the lines are alternatives | nothing in the language alternates; `TypeDesc` runs unit, bool, int, float, str, task, list, record, and none of them is a choice between two others |
| **optional fields**, since `executable` is `null` for a library | `agents.md` §8: "No optional or nullable fields. Every field of a type is required, so validation is a yes or no" |
| **open records**, since no declared subset of the keys validates | `from_json` refuses an unknown field today, measured: `` `Msg` has no field `package_id` `` |

Any one of those is a type-system change with its own design document ahead of
it. All three, taken to make a grant clause work, would be the general schema
language the issue already refused, arrived at sideways.

Two of the three have since been argued on their own: open records landed as
`type Line { reason: String, .. }` (#76) and optional fields as
`executable: String?` (#78), both in `agents.md` §8. That is the table working
rather than the table being wrong - each was a change worth its own argument,
and neither was made to serve a grant clause. The fourth reason below is what
still stops the typed rung, and it was written to be the one that would.

And there is a fourth reason, which is the one that would still hold if the
other three were fixed. **The check would have to cross the broker boundary.**
`sic-broker` depends on `sic-core` and `sic-json` and nothing else;
`sic-core` has no type system, and `crates/sic-core/tests/workspace.rs`
enforces that. A broker checking a type would need one of:

- a type system in `sic-core`, which is the boundary that makes the capability
  boundary mean anything
- the shape serialised into the grant string, which is a schema language living
  in a manifest field
- the broker calling back into the VM, which inverts the suspension design that
  `docs/design/capabilities.md` §6 exists to protect

So the gradient stops below the typed rung, and the useful thing about stopping
there is not that something was avoided. It is that **the whole mechanism now
lives in one crate.** Both remaining rungs are `sic_json::parse` called on
bytes the broker already holds. No type crosses the boundary, no signature
becomes manifest-dependent, and no other crate learns a new word.

---

## 4. Where each check happens

| where | what it checks | why there |
|---|---|---|
| the broker | that the output parses, as one document or as one value per line | it depends on `sic-json` already, and it is the only thing that sees the bytes |
| `from_json`, in the program | that the value fits a declared type | it is the existing validator, it has the type section, and it already fails a run at the boundary |

That split is not a compromise between two half-measures. A malformed answer
never reaches the program at all, and the shape check stays in the one place
that has ever done one.

The broker parses and **throws the result away**. The value the program
receives is the same text it would have received without the clause, and a
program that wants a value still calls `from_json`. So a call with `answers
json` parses its output twice: once in the broker, to check the claim, and once
in the VM, to build a value. That costs one parse of at most `MAX_OUTPUT`
bytes and it is the price of not carrying a parsed value across the wire -
which is `CapValue`'s wall, recorded twice in `sic-core/src/cap.rs`, and not
this document's to move.

Provenance is unaffected and worth stating because it is easy to assume
otherwise. `from_json` on an `Observed<String>` produces an `Observed<T>` - the
label survives the parse, so a field read out of a program's JSON still cannot
decide what the next program runs without a person on the record
(`docs/design/trust.md`). A declared shape is not a discharge. Nothing about
`answers` makes a program's output more trusted; it makes it better formed.

### The limits are already there

`sic-json` bounds a document with `MAX_LEN` (1 MiB) and `MAX_DEPTH` (64), and
the broker bounds a program's output with `MAX_OUTPUT` (1 MiB). The two size
limits are the same number, so `answers json` introduces no new way for a call
to fail on size: output that got past the broker's cap is by construction
within the parser's. For `answers jsonl` the broker parses line by line, so
`MAX_LEN` bounds a line and `MAX_OUTPUT` bounds the stream.

What the measurements say about the headroom, for the programs this repository
would actually name:

| output | bytes | lines | deepest value |
|---|---|---|---|
| `cargo test --no-run --message-format=json`, this workspace | 33,989 | 37 | 3 |
| the same, with a compile error, so every diagnostic is carried | 6,340 | 5 | 6 |
| `cargo metadata --format-version 1`, this workspace | 40,567 | 1 | 8 |

Three orders inside the size limit and an order inside the depth limit. The
second row is worth a sentence: `cargo metadata` on a workspace with a real
dependency tree is routinely over 1 MiB, and would be refused. That this
workspace's is not is a consequence of `[dependencies]` being empty, which is
a fair thing for a document about limits to notice.

The strictness the program inherits is `sic-json`'s and is stricter than some
programs are: no trailing commas, no comments, no `NaN`, and duplicate keys are
an error rather than last-wins. A program whose JSON writer emits `NaN` - which
Python's `json.dumps` does by default - answers something `answers json`
refuses. That is the right refusal and it should be in the message, not a
surprise.

---

## 5. Output that does not parse fails the call

This is the question the documents pull hardest on, so both sides are set out
before it is decided.

**The case for making it an answer.** `docs/design/output.md` §9 deliberately
made a non-zero exit a value rather than an error, because a test suite that
fails is the entire reason to run one, and until `process.run` existed the
failure took the output with it. By the same instinct, a line that does not
parse is information, and a workflow might want to see it. `process.run` was
made a third capability precisely so that a program failing is not a call
failing; giving it a fresh way to fail looks like taking that back.

**It is decided the other way, and the analogy is where the argument breaks.**

A non-zero exit is the program answering. It is a documented outcome of the
program's own semantics, chosen by the program, and the caller asked for it.
A malformed answer is not the program saying anything - it is **the manifest
having been wrong**. Somebody wrote down that this program answers JSON, and it
did not. `sic` fails on a false manifest everywhere else and does not treat it
as data: a `sha256` that does not match refuses to run, an argument vector that
does not start with what the grant pinned is refused, a path with a `..`
component is refused before anything else happens. `answers` belongs with
those.

The second half of the argument is the one that would decide it even if the
first were a wash. **Making it an answer reintroduces the bug.** To hand a
program "it did not parse" there has to be a channel: another field on `Exit`,
or a sentinel value. Either way the program must remember to check it, and a
program that forgets branches on unstructured text believing it structured -
which is exactly the `contains` failure mode, with a new name and an extra
field. A check nobody is obliged to read is not a check.

So: **the call fails, with a `CapError`, and the message names where.** For
`json`, the byte offset the parser stopped at. For `jsonl`, the line number and
the offset within it. `retry` counts it like any other capability failure.

### What this does not change

The distinction is visible in the code and should stay that way. The check runs
on `out.stdout`, and `out.status.code()` is untouched:

- a **non-zero exit with well-formed output** is still `Exit { code: 1, .. }`,
  still an answer, still what makes a failing build reachable
- a **zero exit with malformed output** fails the call

Those really are independent, and cargo demonstrates it: a build failure under
`--no-run --message-format=json` emits perfectly well-formed JSONL and exits
101. A workflow that declares `answers jsonl` for that call gets the failure as
a value and the shape as a guarantee, which is both facts at once and is the
point.

The check also runs regardless of the exit code. The alternative - only check
when the program succeeded - was considered and refused: it makes the claim
conditional on something the grant does not mention, and it exempts the run a
reader most wants checked.

### Blank lines

`jsonl` ignores lines that are empty or whitespace-only. Every program that
emits JSONL ends the last line with a newline, and a rule that refused the
resulting empty final line would fail every grant on its first run. This is one
sentence and it has to be written down, because the alternative is discovering
it in an implementation.

---

## 6. stdout, and the hole this opens

**stdout only**, and the decision is not this document's to make - it was made
in `output.md` §3, and this is the sentence that made it:

> **Not merged**, because the output is going to be parsed. An agent CLI
> answering in JSON, with a warning interleaved into it at whatever point the
> operating system chose, produces a value that fails to parse for reasons that
> are not reproducible.

`answers` is that sentence becoming enforceable. cargo splitting JSON onto
stdout and prose onto stderr is not an awkwardness the design has to work
around; it is the arrangement the design depends on, and merging the two would
make a well-behaved program's output fail a check at random.

### The hole, found while checking this

`process.run` does not merely decline to return stderr. **It drops it
entirely.** `process_capture` puts stderr into the `CapError` when the exit was
non-zero, which is what `output.md` §3's "Not dropped" clause describes.
`process_run` has no such path - a non-zero exit is not an error there, so
there is no error for stderr to travel in - and `out.stderr` is never read.

So `output.md` §9's "stderr is still not a value (§3) ... for the same reason
in all three cases" is half true for `process.run`. "Not merged" holds. "Not
dropped" does not: what a program writes to stderr through `process.run`
reaches nobody at all, whatever it exits with.

That is a pre-existing gap and this document does not fix it, but `answers`
makes it worse and so has two obligations:

1. **The failure message must carry the tail of stderr.** When a grant's
   `answers` claim turns out to be false, stderr is where the program almost
   certainly explained why - a usage error, a missing subcommand, a flag the
   installed version does not have. A message that says "line 1 is not JSON"
   and withholds `error: unexpected argument '--message-format' found` is
   withholding the answer.
2. **Somewhere for `process.run`'s stderr to go is its own issue.** The journal
   is the obvious candidate, since it already records what a call did without
   recording values. It is not in scope here and should not be smuggled in.

---

## 7. What `sic plan` prints

Three renderings, because there are three different promises and a reader has
to be able to tell them apart at a glance.

```text
Capabilities:
  fs.read         [read]  "./manifest.json"  (not pinned)  answers JSON
  process.run     [exec]  "/usr/bin/cargo"  args ["metadata", "--format-version", "1"]  (not pinned)  answers JSON  in "/home/me/project"  with no environment
  process.run     [exec]  "/usr/bin/cargo"  args ["test", "--message-format=json"]  (not pinned)  (no declared shape)  in "/home/me/project"  env PATH, HOME, RUSTFLAGS
```

Three decisions are in there.

**The negative is printed.** `(no declared shape)` is not silence, and the
precedent is `(not pinned)`, which exists for a reason
`docs/design/capabilities.md` states outright: "A grant that says neither still
depends on both, so `sic plan` prints which. A reader who is not told assumes
the grant is the whole of it, and until this existed they were wrong." Silence
is ambiguous between *this grant claims nothing* and *this version of the tool
does not print that*, and the first is the thing a reader most needs to see.

This is also what stops the gradient from making an undeclared grant look
checked: an undeclared grant is not merely un-annotated, it is annotated with
its own absence, on the same line and in the same shape as a pin that is not
there.

With one limit the `(not pinned)` precedent does not itself observe. The
negative belongs only where the clause is available, so `(no declared shape)`
must not appear on `process.exec` or `fs.write` - a grant cannot fail to claim
something it could not have claimed. `(not pinned)` is printed today on
`fs.read`, which `capabilities.md` says may not be pinned, and that is a small
existing wart rather than a pattern to copy.

**The clause is prose, not the keyword.** `answers JSON` and `answers JSON, one
value per line` rather than `answers json` and `answers jsonl`. The rest of a
plan line is already prose - `` in the directory `sic` is started in ``, `with
no environment`, `reading no configuration but this repository's` - and the
grammar is not what a reader of a plan is checking.

**It sits next to the pin.** `sha256` says which program runs and `answers`
says what comes back; those are the two claims about the program itself, and §8
is about how they compose, so they read better adjacent than separated by the
directory and the environment.

### The type is not printed, because there is no type

Had the typed rung survived, this section would have had to decide between a
bare name - checkable only with the source to hand, which is what `VERIFY
Diagnosis` already is - and the full record, which the bytecode can supply:
`TypeDesc::Object` carries `fields: Vec<(String, u32)>`, and the comment on it
says the names are there because validating a JSON document needs them. So the
option existed and the question was real.

It is moot, and worth recording as moot rather than as unexamined: with §3's
refusal there is no type on a grant to print, and the existing `VERIFY` line at
the `from_json` site remains the only place a plan names a shape. Whether
*that* line should print fields is a separate question about `sic plan` and
belongs to whoever asks it.

---

## 8. `sha256` is not enough, and it is not an alternative

The issue's last section asks whether pinning the binary is the cheap answer
that makes this unnecessary, and offers a reason it is not: a pin gives no plan
that says the shape "in words a reader can check without running anything".

That reason is weaker than the true one, and the true one is measurable.
**A pinned binary does not have fixed output.** The same cargo, byte for byte,
emits pure JSONL when the build fails and JSONL-then-prose when the tests run -
that is §1, and neither the digest nor anything else in the grant distinguishes
them. What the output looks like depends on the workspace, the toolchain, the
number of warnings, and which of the program's own outcomes occurred. A pin
fixes *which program runs*. The shape of what comes back is not a property of
the file.

So they are orthogonal, they compose, and each says something the other cannot:

| | what it fixes | what it still allows |
|---|---|---|
| `sha256` alone | which program runs | anything at all on stdout |
| `answers json` alone | the grammar of the answer | a different program at that path tomorrow |
| both | this exact program, and its answer checked to be JSON | this program's JSON to mean something new |

The third row's remainder is the honest limit of all of this, and it should not
be glossed. Neither clause pins *meaning*. A cargo that renamed `reason` to
`kind` would pass `answers jsonl` and change what every field access on those
lines means. What the declaration buys is that a **structural** change is
caught at the boundary, loudly, naming the line - and that a reader of the plan
knows which kind of dependency the program has taken.

Pinning `cargo` in `workflows/ci.sic` remains a reasonable separate idea with a
real cost - a rustup update makes the pin stale, and a stale pin refuses to run
at all. It is not a cheaper version of this. It answers a different question.

---

## 9. Nothing crosses the wire that did not before

`answers` is a check, not a conversion. `process.run` still returns
`Exit { code: Int, output: Observed<String> }`, `process.capture` still returns
`Observed<String>`, and `fs.read` still returns `String`. Three reasons, in
increasing order of how much they decide:

**`CapValue` does not grow.** The issue proposed that JSONL cross as
`List<String>`, one line per element, since `CapValue::List(Vec<String>)`
already exists. It does - but `Exit.output` is a `String`, so `process.run`
answering a list of lines needs either a new variant or an `Exit` whose field is
a list, and that is the nesting `cap.rs` refuses twice, in comments, with
reasons.

**A capability's signature would stop being static.** `CapSig.ret` is a `const
TypeId` in a `const BUILTIN_CAPS` table. Making the return type depend on
whether the grant said `jsonl` makes every capability call's type a function of
the manifest - a change to how the checker resolves capability calls, for a
clause on one grant.

**The program could not use the list anyway.** This is the one that settles it.
The issue says "`for line in lines` walks it (that landed in #66)". `for` did
land, and it walks a list - but the language's built-in functions are `len`,
`approve`, `choose`, `from_json`, `contains` and `starts_with`. **There is no
`lines` and no `split`.** Nothing in `sic` turns a string into a list of lines.

Which means the rungs are not equally finished today, and saying so is more
useful than a table that implies they are:

| rung | checkable at the boundary | usable by the program today |
|---|---|---|
| `answers json` | yes | **yes** - `let m: Config = from_json(text)` works now |
| `answers jsonl` | yes | no - nothing splits the string into lines |

`answers jsonl` is still worth having, because refutation does not require
consumption: a grant that claims JSONL is refused the moment the program stops
emitting it, which is the whole failure mode this issue is about, and the plan
prints the claim either way. But a workflow cannot yet read fields out of
those lines, and the implementation issue should either build `json` first or
build `jsonl` alongside whatever gives a program its lines. **That splitter is
a separate issue and this document does not design it.**

---

## 10. What this does for `workflows/ci.sic`, which is less than was hoped

The workflow that motivated the issue cannot use the feature the issue asked
for, and the reason is §1: on stable, cargo has no machine format for the fact
`ran_but_failed` reads.

The obvious rewrite does not work either, and it is worth walking into the wall
in writing so nobody walks into it in code. Split the call in two: build with
`--no-run --message-format=json`, which is pure JSONL and takes `answers jsonl`
honestly, then run the test binary whose path the JSON just supplied. That path
is `Observed<String>`, and passing it to `process.run` is refused, measured:

```text
error[E0372]: Observed<String> cannot be passed to `process.run`
  = note: `approve(question, value)` turns it into one a person signed off
```

The way through is `approve` - a person, per run. For an unattended development loop
that is not a workflow, and `docs/design/output.md` §5 says why the rule is
strict on purpose: a string a program printed deciding what the next program
runs is the oldest injection there is.

So `ci.sic` keeps its `contains`, and what it gets from this design is one
line in the plan:

```text
  process.run     [exec]  "/PATH/TO/cargo"  args ["test"]  (not pinned)  (no declared shape)  in "/PATH/TO/sic"  env PATH, HOME, RUSTFLAGS
```

That is a smaller result than the issue expected and it is the correct one.
The workflow's comment already says it depends on cargo's wording; a comment is
read by whoever edits the file, and a plan is read by whoever decides to run
it, and those have never been the same person. Making the absence visible to
the second one is the whole of what is available here, and it is worth having.

The programs that get the full benefit are the ones that do answer in JSON -
`cargo metadata`, a linter with a `--format=json`, an agent CLI answering
structurally - and there are more of those than there are cargos.

---

## 11. Grammar, and where it goes

`answers` joins the order-free clause loop that already parses `repeatable`,
`delegable`, `in` and `env`, as a fifth arm, at most once. Like `args`,
`sha256` and `env` it is an ordinary identifier and nothing is reserved.

```text
answers json
answers jsonl
```

The format is a **bare identifier, not a string**, which is the one place this
deliberately departs from `sha256 "..."`. A digest is data and its content is
unbounded; a format is one of two words, and an identifier makes `answers
jsonl1` a diagnostic at the point it is written rather than a string that means
nothing to anybody until the broker refuses it.

Three things carry it, none of them new mechanism:

- `CapGrant` in `sic-core` gains one flat field. The broker already receives a
  `CapGrant` and nothing else, and a format tag needs no type system - which is
  §3's refusal paying for itself immediately.
- The `CAPABILITIES` section of the `.sicb` gains one byte. It already carries
  `param_types` and `ret_type` as indices, so a small fixed field is precedent.
- `sic plan` reads it from the manifest, like everything else it prints.

**Which capabilities accept the clause.** Only the ones that hand a program
text it has to interpret: `fs.read`, `process.capture`, `process.run`.

- `process.exec` returns an `Int`. There is no output to shape.
- `fs.write` returns `Unit`, and `git.status` and `git.rev_parse` answer values
  the broker itself built - a shape declared over those would be the program
  telling the broker about the broker.
- `llm.invoke` is out for a different and better reason: `agent` already
  declares the shape of a model's answer, and a second place to say it would be
  two mechanisms for one claim, disagreeing on the day somebody edits one.

`fs.read` deserves the sentence it costs, because `capabilities.md` refuses a
`sha256` on it - "Pinning what `fs.read` reads would have to say what the
contents must be, which is not what a grant is for". That refusal does not
reach this clause, and the difference is exactly the one §8 draws: a pin says
which bytes, and `answers` says which grammar. A grammar is checkable by
somebody who does not know the contents in advance, which is what made the pin
the wrong shape and makes this one the right one.

**Two diagnostics, and this document does not add them.** `E0219` for
`answers` followed by something that is not `json` or `jsonl`, and `E0337` for
`answers` on a capability with nothing to shape - the neighbour of `E0334`,
`in` or `env` on a capability that starts no process. They are named here so
the implementation issue does not have to choose, and they stay out of
`docs/diagnostics.md` until something reports them, because the test in
`sic-core` fails on a code that is listed and never produced.

> **Corrected on implementation: the first one is `E0220`.** `E0219` was taken
> between this document being written and being built, by `..` in a type body
> other than the end - the open-record work, one commit earlier. Reserving a
> code in prose does not reserve it in `docs/diagnostics.md`, and this is the
> cost of that: a number picked by reading the index is stale as soon as
> anything else lands. `E0337` was still free. The rest of the sentence holds,
> and both codes are in the index now that something reports them.

---

## 12. Not in this

- **XML, and it is a refusal rather than a deferral.** Issue #75 argues this at
  length and the argument is carried forward rather than restated, because
  measurement sharpened it. `sic-json` defends itself with two constants,
  `MAX_LEN` and `MAX_DEPTH`, and §4 shows real cargo output sitting three
  orders inside one and an order inside the other. Two numbers close the
  hostile-input surface because a JSON document is a tree of data with no way
  to refer to anything: the only unbounded things are how long it is and how
  deeply it nests, and each is one comparison.

  XML's unbounded things are not size and depth - they are **references**.
  External entities name a local file or a URL, and a parser that resolves one
  reads the file for whoever sent the document. Entity expansion turns a
  kilobyte into a gigabyte through nesting that lives in the *definitions*,
  where a depth limit does not look. A DTD is a schema the document brings with
  it, which is a small language of its own. There is no pair of constants for
  any of that, because a limit on references is a limit on a language rather
  than on a document, and the boundary of the safe subset - not the parser - is
  the hard part. Getting it wrong is a file-disclosure bug in the one crate
  that talks to the outside world.

  `CLAUDE.md` treats supply chain as a primary risk and answers it by writing
  everything by hand. That answer is right when hand-writing is the safer
  option, and here it is not: a hand-written XML parser trades a dependency
  risk for a vulnerability class.

  The escape is better than it was when the issue proposed it. A program that
  must read XML runs a converter through `process.run` and answers JSON - and
  now that converter's own grant can say `answers json`, so the parser sits
  outside the trust boundary at a path a reader can see, with a digest that can
  be pinned and a shape that is checked. The refusal comes with somewhere to
  go.

- **A general schema language.** The `type` declarations the language already
  has are the schema, and a second one is a second type system. §3 refuses the
  rung that would have needed it.

- **A `List<Record>` across the capability boundary.** §9, and it is a
  `CapValue` question rather than this one.

- **Checking the shape of what a program is *given*.** `args` pins a prefix of
  the argument vector and that is a different mechanism with a different
  argument (`docs/design/arguments.md`). This is about what comes back.

- **Optional fields, sum types and open records.** Each would be needed by the
  refused rung and each needs its own document. Naming them together here is
  not a plan to build them; it is a record of what `answers jsonl of T` would
  cost, so that the next person to propose it starts from the bill.

- **Splitting a string into lines.** §9. `answers jsonl` checks something the
  program cannot yet consume, and the missing built-in is a separate issue.

- **Somewhere for `process.run`'s stderr to go.** §6 found the gap and does not
  close it. The failure message obligation in §6 is in scope; the journal
  question is not.

- **Removing the `contains` from `workflows/ci.sic`.** §10. It cannot be
  replaced, because cargo has nothing to replace it with, and reverting it
  would trade a visible dependency for an invisible one.

---

## 13. Units of work

| # | Unit | Done when |
|---|------|-----------|
| 1 | ~~`answers json \| jsonl` in the grant, in the order-free clause loop, with `E0219`~~ | a grant parses and reprints, and `answers jsonl1` is a diagnostic |
| 2 | ~~`E0337` for a capability with nothing to shape~~ | `answers` on `process.exec` is refused at compile time |
| 3 | ~~The field in `CapGrant` and the byte in `CAPABILITIES`~~ | a `.sicb` round-trips the clause and the verifier reads it |
| 4 | ~~The broker check for `json`, using `sic_json::parse`~~ | a call whose output is not one JSON document fails, naming the offset |
| 5 | ~~The broker check for `jsonl`, line by line, blank lines skipped~~ | a stream that stops being JSONL fails, naming the line, and a trailing newline does not |
| 6 | ~~stderr's tail in the failure message~~ | a grant refused because the flag was rejected says what the program said |
| 7 | ~~The three plan renderings, including `(no declared shape)`~~ | an undeclared grant reads as claiming nothing, and `sic plan workflows/ci.sic` says so |

Unit 7 is the one that pays for the motivating case. That is worth knowing
before the order is chosen.

**Done, in one commit, and four things are worth recording about how.**

**The diagnostic is `E0220`.** §11 has the correction; the short version is
that `E0219` was taken by other work between this document and its
implementation, and a code reserved in prose is not reserved.

**`VERSION_MINOR` moved to 10.** A byte in the middle of a `CAPABILITIES`
entry is the case the version exists for: an old reader takes it for
`repeatable` and every field after it for the one before, and the section
still decodes. It had moved to 9 one commit earlier for an unrelated flag, and
moving it twice before a release costs nothing, while two layouts sharing a
number costs the guarantee the number is for.

**Which capabilities take the clause lives in `sic-core`**, as
`Answers::available_on`, rather than in the checker's `CapSig` table beside
`accepts_pin`. Three crates ask the question - the checker refuses the clause,
the broker performs it, and `sic plan` decides whether a grant that said
nothing is one that could have said something - and the third is the reason:
§7's rule that `(no declared shape)` must not appear where the clause was
unavailable is a rule `sic-plan` has to be able to check, and `sic-plan` does
not depend on `sic-types`. One list, in the crate everything depends on.

**The broker's check runs in three places, not two.** §11 names `fs.read`,
`process.capture` and `process.run` as the capabilities that take the clause,
and all three now check it. `process.capture` was easy to overlook because §5
argues entirely about `process.run`'s exit code; it refuses a non-zero exit
already, so its `answers` check runs only on output it was going to return.
