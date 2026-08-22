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

## Status: phase 1 (syntax)

```text
Source -> Lexer -> Parser -> AST     <- we are here
       -> Type Checker -> IR -> Bytecode -> Verifier -> VM
```

```console
$ sic parse examples/milestone.sic
(module
  (fn main
    (block
      (let x 10)
      (let y (+ x 20))
      (return y))))
```

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
| `sic-cli` | the `sic` command |

Crates are added per phase (`sic-types`, `sic-ir`, `sic-bytecode`, `sic-verify`,
`sic-vm`, `sic-journal`, `sic-broker`). `sic-vm` has no external effects and does
not depend on `sic-broker`; that boundary is where the VM and the capability
broker will later split into separate processes.

## Adding a dependency

`[dependencies]` stays empty. To propose a crate, document the following in
`docs/design/` first:

1. why it is needed
2. why `std` alone is insufficient
3. how much the dependency tree grows
4. the cost of implementing it by hand
5. the security impact
