# Arguments

```rust
CapSig {
    name: "process.exec",
    params: &[Types::STR],   // one absolute path
    ret: Types::INT,         // the exit code
}
```

A binary, and whether it succeeded. The only programs sic can drive are the ones
that need to be told nothing, and almost nothing real is in that set:
`git commit -m "..."`, `cargo test --workspace`, `tmux send-keys -t pane "..."`
are all out of reach.

This is the argument vector. Reading what a program said is the other half and
has its own document; a call that can say something but not hear the answer is
still half a call.

---

## 1. Everything is ready except the boundary

The comment in `crates/sic-types/src/cap.rs` says "no argument vector until
there is a list type". There is one now, and it goes further than the type
system:

| | where |
|---|---|
| `List<T>` | `sic-types/src/ty.rs:53` |
| a list literal | `sic-syntax/src/ast.rs:288` |
| `MAKE_LIST` | `sic-bytecode/src/inst.rs:101` |
| `Value::List` in the arena | `sic-vm/src/value.rs:22` |
| `len` | `sic-types/src/check.rs:1046` |

A sic program can already build a list of strings and pass it to a function.
What it cannot do is hand one to a capability:

```rust
pub enum CapValue { Unit, Bool(bool), I64(i64), F64(f64), Str(String) }
```

`CapValue` is the wire between the VM and the broker (`sic-core/src/cap.rs`).
It is the one place in this design where the two halves are meant to become two
processes, so widening it is a decision about a format rather than about a
struct.

---

## 2. `List(Vec<String>)`, not `List(Vec<CapValue>)`

The general one would carry anything, including another list. That buys nesting,
and nesting buys a depth limit, a recursive encoder, a recursive decoder that
has to refuse a hostile depth, and a checkpoint reader that has to do the same.
None of it is needed by anything that exists.

What exists is an argument vector: a flat sequence of strings.

Being wrong in the narrow direction costs one format version, and both formats
here are already versioned for it - the bytecode file is at 0.2 and the
checkpoint at 0.2, each having been bumped once already. Being wrong in the
general direction costs a recursive encoder that everything downstream carries
forever.

When something needs structured values across this boundary - routing an
agent's tool calls through the broker is the candidate, and it is issue #5 -
that is when it is designed, with the requirement in hand rather than guessed
at.

---

## 3. What a grant means once arguments exist

This is the part that matters, and it is not plumbing.

Today a grant's constraint is the path, and a path is the whole of what
`process.exec` can be told. Once there are arguments, this:

```sic
allow {
    process.exec "/usr/bin/tmux";
}
```

reads as "may run tmux" and means **may drive every pane on this machine**,
including the one a person is working in. The manifest would be printing one
thing and permitting another, which is the failure this project exists to
notice.

**A grant may pin a prefix of the argument vector, and pins the empty vector by
default.**

```sic
allow {
    process.exec "/usr/bin/true";                                    // no arguments at all
    process.exec "/usr/bin/git" args ["commit"];                     // git commit ..., nothing else
    process.exec "/usr/bin/tmux" args ["send-keys", "-t", "sic:0"];  // that pane, and no other
}
```

Three reasons for a prefix rather than something richer:

- **It is the constraint that already exists, applied to a second thing.**
  `fs.read "./docs"` bounds a path by its prefix. This bounds an argument vector
  by its prefix. One idea used twice is smaller than two ideas.
- **It is readable at a glance**, which a glob or a regular expression is not. A
  constraint nobody checks by eye is a constraint nobody checks.
- **It is static**, so it can be printed before the run (§4).

The call side matches: the vector is optional and defaults to empty, so
`process.exec("/usr/bin/true")` keeps meaning what it means today and
`process.exec("/usr/bin/true", [])` is the same call written out. A capability
with an optional trailing parameter is new, and it is the only one.

**An absent `args` means the vector must be empty.** Every grant written before
this change keeps exactly the authority it had; nothing silently widens. There
is deliberately no way to write "any arguments": a program that needs a variable
argument is naming the prefix it varies from, and one that truly needs anything
is asking for `sh -c`, which it can write in the open and be read doing.

"Be read doing" holds for the program and not for an agent answering its model
calls, which writes its own arguments at run time where no plan can print them.
That is why a `process` grant does not reach the agent unless it says
`delegable`: see `docs/design/authority.md` §4a.

What a prefix does **not** do is bound what is said. `send-keys -t sic:0
<anything>` still types anything into that pane. It bounds *which* pane, which
subcommand, which target - the deputy is scoped, not disarmed. Saying so here is
better than letting a manifest imply more.

---

## 4. What `sic plan` can still say

Arguments are runtime values. A plan reads bytecode and runs nothing, so in
general it cannot know them:

```text
    1. EXEC   process.exec  "/usr/bin/tmux"   ; 12:5
```

That line is true and nearly useless. The pinned prefix is not a runtime value -
it is in the manifest - so it can be printed:

```text
Capabilities:
  process.exec  [exec]  "/usr/bin/tmux"  args ["send-keys", "-t", "sic:0"]  ...
```

Which is the second argument for §3 on its own: without the pin, adding
arguments makes `sic plan` less honest than it was, because the thing that
decides what happens becomes invisible to it.

---

## 5. What changes

| | |
|---|---|
| `sic-core/src/cap.rs` | `CapValue::List(Vec<String>)` |
| `sic-vm/src/lib.rs:316` | `to_cap_value` reads a `Value::List` out of the arena; a non-string element is a run failure, not a coercion |
| `sic-vm/src/checkpoint.rs:591` | tag 5, and the checkpoint format goes to 0.3 |
| `sic-types/src/cap.rs` | `process.exec` gains a `List<String>` parameter |
| `sic-syntax`, `sic-bytecode` | `args [...]` in a grant, and the manifest entry that carries it - the file format goes to 0.3 |
| `sic-broker/src/lib.rs` | the prefix is checked before the child is started, next to where the digest pin is checked |
| `sic-plan` | §4 |

The prefix check belongs in the broker, beside `allowed_path` and the digest
pin, for the same reason those are there: the broker is what performs the
effect, and a check that runs anywhere else is a check the effect did not have
to pass.

---

## 6. Not here

- **Reading what the program said.** The other half of the same problem, and its
  own document.
- **Nested values across the boundary** (§2).
- **Argument constraints richer than a prefix**: globs, patterns, "this
  argument must be a path under X". Each is a small language, and a manifest
  full of small languages is not readable before a run.
- **Environment variables, stdin, a working directory, a shell, PATH.** The
  child gets a cleared environment and no shell, as it does today.
