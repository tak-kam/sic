# sic

A language and execution environment for AI agents, workflows, and automation.

The goal is not only to express *what to compute*, but to make **what may be
executed**, **on what grounds it was executed**, and **what actually happened**
first-class concerns of the language and its runtime.

- Capability-based security: every external effect must be declared
- A register-based VM running verified, purpose-built bytecode
- Observability, audit, and replay derived from a single execution journal
- Durable execution (suspend / save / resume)

The implementation is Rust with **zero external crates**, because supply chain
attacks are treated as a primary risk.

## Status: phase 3 (capabilities)

```text
Source -> Lexer -> Parser -> AST -> Type Checker -> IR
       -> Bytecode -> Verifier -> VM
```

```console
$ sic run examples/milestone.sic
30
```

Bytecode can also be written, checked and read on its own:

```console
$ sic compile examples/factorial.sic -o factorial.sicb
wrote factorial.sicb (426 bytes)

$ sic verify factorial.sicb
ok: 2 function(s) verified
required capabilities:
  (none)

$ sic disasm factorial.sicb
...
  0000  LOAD_CONST  r1, k0  ; 1  ; 5:13
  0001  LE          r2, r0, r1  ; 5:8
  0002  JUMP_IF_NOT r2, +2  ; -> 0005  ; 5:8
```

### Effects are capabilities

Reaching outside the program takes a grant, and the grant is part of the
program:

```text
allow {
    fs.read "./examples/greeting.txt";
}

fn main() -> String {
    return fs.read("./examples/greeting.txt");
}
```

Calling a capability the module did not declare is a compile error, so the
manifest of a compiled module is complete by construction. `sic verify` reports
it without running anything:

```console
$ sic verify read-file.sicb
ok: 1 function(s) verified
required capabilities:
  fs.read [read] "./examples/greeting.txt"
```

The VM never performs the effect itself. It suspends, the driver asks the
broker, and the broker decides again - the manifest is the contract between
them, not a formality the compiler already handled:

```text
CALL_CAP -> Suspended(request) -> broker -> resume(value) -> next instruction
```

`sic-broker` is the only crate that touches the outside world. See
[docs/design/capabilities.md](docs/design/capabilities.md).

Every phase is verified: `sic run` compiles, verifies, and only then executes.
The VM never runs bytecode that has not passed the verifier, including bytecode
this process just produced.

Other commands: `sic parse` (AST), `sic hir` (high-level IR).

See [docs/design/v0.1.md](docs/design/v0.1.md) for the design.

## Building

```console
$ cargo test
$ cargo run -p sic-cli -- parse examples/milestone.sic
```

Linking needs a C linker (`cc`). If you see `linker 'cc' not found`, install one
(`sudo apt install build-essential` on Debian/Ubuntu). If you cannot, link with
the `rust-lld` that rustup already ships, against the musl target:

```console
$ rustup target add x86_64-unknown-linux-musl
$ LLD="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
$ RUSTFLAGS="-Clinker=$LLD -Clinker-flavor=ld.lld" \
    cargo test --target x86_64-unknown-linux-musl
```

## Layout

| crate | role |
|-------|------|
| `sic-core` | `Span`, `SourceFile`, `Diagnostic`, shared ID newtypes |
| `sic-syntax` | lexer, AST, parser (recursive descent; Pratt for expressions) |
| `sic-types` | interned types, type checker, name resolution |
| `sic-ir` | high-level IR, where workflow semantics still exist |
| `sic-bytecode` | instruction set, `.sicb` format, disassembler |
| `sic-compile` | HIR to bytecode |
| `sic-verify` | the bytecode verifier |
| `sic-vm` | the register VM |
| `sic-broker` | performs capability calls; the only crate with external effects |
| `sic-cli` | the `sic` command |

`sic-journal` arrives in phase 4. `sic-vm` performs no external effects and does
not depend on `sic-broker`; that boundary is where the VM and the capability
broker will later split into separate processes, and it is checked by a test
rather than left as an intention.

## Adding a dependency

`[dependencies]` stays empty. To propose a crate, document the following in
`docs/design/` first:

1. why it is needed
2. why `std` alone is insufficient
3. how much the dependency tree grows
4. the cost of implementing it by hand
5. the security impact
