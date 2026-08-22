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

## Status: phase 2 (the whole pipeline runs)

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

`sic verify` reports what a module is allowed to do before anything runs, which
is the foundation the capability model of phase 3 builds on. Nothing can declare
a capability yet, so the answer is always `(none)`.

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
| `sic-cli` | the `sic` command |

`sic-journal` and `sic-broker` arrive in phases 3 and 4. `sic-vm` performs no
external effects and does not depend on `sic-broker`; that boundary is where the
VM and the capability broker will later split into separate processes, and it is
checked by a test rather than left as an intention.

## Adding a dependency

`[dependencies]` stays empty. To propose a crate, document the following in
`docs/design/` first:

1. why it is needed
2. why `std` alone is insufficient
3. how much the dependency tree grows
4. the cost of implementing it by hand
5. the security impact
