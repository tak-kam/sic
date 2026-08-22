# sic

A language and runtime for AI agents, workflows, and automation. See
[docs/design/v0.1.md](docs/design/v0.1.md) for the design of the current version.

## Repository language: English only

Everything committed to this repository is written in English:

- code comments and doc comments
- identifiers and test names
- `README.md` and everything under `docs/`
- commit messages and PR descriptions
- user-facing output of the `sic` binary (diagnostics, help text, error messages)
- comments inside `.sic` example programs

Conversation with the user happens in Japanese; only repository content is English.
When editing a file that still contains Japanese, translate it rather than leaving it.

The one exception is non-ASCII text used as test *data* -- multi-byte column
arithmetic, rejecting non-ASCII identifiers, and similar cases -- where the
characters are the thing under test.

## No external dependencies

`[dependencies]` stays empty in every crate. Supply chain attacks are treated as a
primary risk, so the lexer, parser, type checker, IR, bytecode compiler, verifier,
VM, JSON handling, scheduler, and journal are all written by hand.

Never add a crate because it is convenient. To propose one, first document in
`docs/design/`:

1. why it is needed
2. why `std` alone is insufficient
3. how much the dependency tree grows
4. the cost of implementing it by hand
5. the security impact

## Structure

| crate | role |
|-------|------|
| `sic-core` | `Span`, `SourceFile`, `Diagnostic`, shared ID newtypes |
| `sic-syntax` | lexer, AST, parser (recursive descent; Pratt for expressions only) |
| `sic-cli` | the `sic` binary |

Crates are added per phase (`sic-types`, `sic-ir`, `sic-bytecode`, `sic-verify`,
`sic-vm`, `sic-journal`, `sic-broker`). Two boundaries must hold:

- `sic-vm` performs no external effects and never depends on `sic-broker`.
  Effects go through capabilities; the VM holds no credentials.
- `sic-core` depends on nothing else in the workspace.

## Building

`cargo test` needs a C linker (`cc`). On a machine without one, link with the
`rust-lld` shipped by rustup against the musl target:

```console
$ rustup target add x86_64-unknown-linux-musl
$ LLD="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
$ RUSTFLAGS="-Clinker=$LLD -Clinker-flavor=ld.lld" \
    cargo test --target x86_64-unknown-linux-musl
```

## Implementation priorities

Simple, small, explicit, deterministic, testable, dependency-free, auditable.
Do not build generic abstractions for features that do not exist yet.
