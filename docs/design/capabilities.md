# Capabilities (phase 3)

Phase 3 makes external effects expressible, and makes them the only way out of
the VM. Nothing here adds network access or an LLM; those are later phases that
plug into the same mechanism.

The property to preserve is the one from section 9 of the specification:

```text
VM: no network, no secrets, no filesystem
 |
 | capability request / response
 v
Broker: performs the effect, holds the credentials
```

---

## 1. What a capability is

A capability is a named, typed operation that the VM cannot perform itself, and
that a module must declare before it can call.

v0.1 ships three, chosen because they need no credentials and no network:

| Capability | Signature | Kind |
|---|---|---|
| `fs.read` | `(path: String) -> String` | read |
| `fs.write` | `(path: String, data: String) -> Unit` | write |
| `process.exec` | `(path: String, args: List<String>) -> Int` | exec |
| `human.approve` | `(question: String) -> Bool` | invoke |

`process.exec` is the only one whose last parameter may be left off:
`process.exec("/usr/bin/true")` passes an empty vector, so a program written
before arguments existed still says what it said. What a grant may pin about
those arguments is in `docs/design/arguments.md`.

---

## 2. Declaring capabilities

```text
allow {
    fs.read "./input.txt";
    process.exec "/usr/bin/true";
}
```

- `allow` is a top-level item, like a function.
- Each grant names a capability and, optionally, a constraint string.
- A capability may be granted once per module. Two grants of the same name are
  an error rather than a silent merge.
- `allow` with no grants is legal and means the module needs nothing.

The constraint's meaning is per capability: for `fs.read` and `fs.write` it is
the exact path that may be touched; for `process.exec` it is the absolute path
of the executable, and `args [...]` after it pins what the argument vector has
to start with.

**A grant is not optional.** Calling a capability the module did not declare is
a compile error, not a runtime failure, so the manifest of a compiled module is
complete by construction.

---

## 3. Calling a capability

```text
let text = fs.read("./input.txt");
fs.write("./output.txt", text);
let code = process.exec("/usr/bin/true");
```

Syntactically this is a call whose callee is a field access, which the parser
already produces. The type checker recognizes the shape and resolves it against
the capability table; anything else keeps the existing "field access is not
supported" error.

A local binding shadows the namespace, so `let fs = 1;` makes `fs.read(...)` an
error rather than silently meaning something else.

---

## 4. The manifest in the bytecode

The `CAPABILITIES` section grows a signature:

```text
name        : str      ; "fs.read"
kind        : u8       ; read / write / exec / invoke
constraints : str      ; "./input.txt"
param_count : u8
param_types : u32 * param_count   ; indices into TYPES
ret_type    : u32
```

The signature is in the file for the same reason the function table carries
parameter types: the verifier has to check a call site without trusting whoever
produced the bytecode. It also means `sic verify` can answer "what may this
module do" from the file alone, with nothing executed - the basis for
`sic plan` in a later phase.

---

## 5. `CALL_CAP`

One new opcode, in ABC form:

```text
CALL_CAP  a, b, c     ; R[a] = cap[b](R[c .. c+argc])
```

Same shape as `CALL`, on purpose: arguments sit in consecutive registers, and
the argument count comes from the manifest. The verifier checks the capability
index, the argument types, and the type it writes to `R[a]`.

The design sketch had `CALL_CAP a, bx`, but an operand is needed for the
argument base, so the form matches `CALL` instead.

---

## 6. How the VM performs a call it cannot perform

The VM does not call the broker. It **suspends**:

```text
CALL_CAP
   |
   v
Status::Suspended(CapabilityRequest { index, name, args })
   |
   |  the driver asks the broker
   v
vm.resume(value)
   |
   v
next instruction
```

Two things follow from this, and they are the reason for the shape:

- The VM crate depends on nothing that can perform an effect. The isolation
  test from phase 2 keeps holding, with no `CapabilityHost` trait to smuggle a
  filesystem in through.
- The suspension point is exactly what phase 5 has to checkpoint. Durable
  execution then means writing out the state that already exists at this point,
  rather than adding a second mechanism next to a synchronous call.

Request and response values cross this boundary as `CapValue`, a small owned
enum with no handles into the VM's arena, because this is the future IPC
boundary and it has to survive being serialized.

---

## 7. The broker

`sic-broker` is the only crate in the workspace that touches the outside world.
It receives a request, decides whether it is allowed, performs it, and returns
a value.

Authorization happens per call, against the manifest, even though the compiler
already checked the grant. The broker must not trust the bytecode it is serving:
in the eventual split these are separate processes, and the manifest is the
contract between them.

Rules in v0.1:

- **A path with a `..` component is refused**, before anything else. Comparing
  paths that can climb out of their prefix is where this kind of check usually
  fails.
- `fs.read` and `fs.write` accept exactly the path the grant names.
- `process.exec` requires an absolute path and refuses anything else, so an
  executable is never resolved through `PATH` (section 10 of the specification).
- The process inherits no environment. It is given the argument vector the
  call passed, which has to start with what the grant pinned; a grant that
  pins nothing allows no arguments at all. Its exit code is the result, and a
  signal is a failure rather than an exit code.

### Pinning what runs

An absolute path says *where* to look, not *what is there*. A path that pointed
at the right binary yesterday can point at a different one today - a package
upgrade, a writable directory, someone with a shell. So a grant can pin the
contents:

```text
allow {
    process.exec "/usr/bin/true"
        sha256 "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";
}
```

The broker hashes the file and refuses to run it if the digest does not match.
That happens on every call, not once at startup: a check that ran earlier tells
you what was true earlier.

A pin is optional, because requiring one would make `process.exec` unusable
where the binary legitimately changes. Whether a grant without one is acceptable
is a question for whoever reads the plan, and `sic plan` says which grants are
pinned.

Only `process.exec` takes a pin. Pinning a path that `fs.read` will read is a
different feature - it would have to say what the contents must be, which is not
what a grant is for - and accepting the syntax while ignoring it would be worse
than refusing it.

Not in v0.1, and named so their absence is a decision rather than an oversight:
path prefixes and globs, credential injection, and any capability that opens a
socket.

---

## 8. Effects that cannot answer now

`human.approve` never answers within the call: a person is not in this process.
A broker call therefore returns one of

```rust
enum CapOutcome {
    Value(CapValue),
    Deferred { question: String },
}
```

`Deferred` is what makes durable execution necessary rather than optional. The
run stops, its state is written out, and it continues when the answer arrives -
possibly in another process, on another day. See
[durable-execution.md](durable-execution.md).

The grant's constraint says what an approval is about, and it travels with the
question (`[deploy to production] deploy build 42?`), so whoever answers, and
whoever audits it later, can see which grant was exercised.

---

## 9. What a failure is

A capability failure ends the run, the same way `FAIL` does. Retry is a workflow
concern, and the IR already has the `CallPolicy` slot for it, but nothing
populates it until phase 6.

The failure carries the capability name and the reason, and the run's exit
reports it with the source position of the `CALL_CAP` from the debug section.
