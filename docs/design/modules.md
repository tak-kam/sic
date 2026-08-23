# Modules

A program can be more than one file.

```text
// lib/deploy.sic
requires {
    process.exec;
}

fn deploy(binary: String) -> Int {
    return process.exec(binary);
}
```

```text
// main.sic
import "./lib/deploy.sic";

allow {
    process.exec "/usr/bin/deploy";
}

fn main() -> Int {
    return deploy("/usr/bin/deploy");
}
```

The library says what it needs. The program says what it allows, and with what
constraint. Neither alone is enough to reach an effect.

---

## 1. Why not the two simpler answers

**Letting the importer grant everything** would mean a library cannot use an
effect at all, and every program would have to know the internals of what it
imports in order to declare the grants those internals need. Changing a
library's implementation would then be a breaking change to its callers.

**Unioning the grants** - a library brings its own `allow`, and the manifest is
whatever is reachable - is how a dependency quietly acquires the ability to run
a process. That is the failure this project exists to prevent, and convenience
is not a reason to reintroduce it.

So a library declares a *need* and the program declares a *grant*, and the two
are checked against each other. The rule that has to survive is unchanged:

> Nothing reaches an effect that the manifest of the program being run does not
> name.

## 2. `requires`

```text
requires {
    process.exec;
    fs.read;
}
```

- Names a capability, never a constraint. **What** a library does is its own
  business; **which file or which binary** is the program's, because only the
  program knows what it is being pointed at.
- Only a file that is imported may have one. The program at the top has `allow`;
  a library has `requires`. A file with both is an error - it would be granting
  itself something.
- Every `requires` has to be covered by the program's `allow`, or it does not
  compile. The message names the file that needs it.

A `requires` for a capability nothing in the program calls is a warning, the
same as an unused grant: authority asked for and not used should never pass
unnoticed. It is a warning about the program rather than about the one file,
because after imports are resolved a call site is a call site.

## 3. `import`

```text
import "./lib/deploy.sic";
```

- A path relative to the importing file. Absolute paths are refused, and so is
  any path with a `..` component - the same rule the broker applies, for the
  same reason.
- **No network resolution, no registry, no version.** Section 11 of the
  specification, unchanged. A copy of something goes in `vendor/`.
- Importing the same file twice, directly or through a chain, brings it in once.
- A cycle is an error, and the message shows the chain.

### Names are flat

An import brings its names in as they are; there is no `lib.deploy`.

That is not because a namespace would be useless, but because `a.b()` already
means one specific thing - a capability call - and adding a second meaning to
the same syntax would make `fs.read()` ambiguous in a way no amount of
diagnostics fixes.

Two files defining the same name is an error, and the message shows both.

### Everything in a file is importable

There is no `pub`. Splitting a file is the visibility mechanism, which is enough
at this size and can be narrowed later without breaking anything.

## 4. Compilation

Imports are resolved into one module before type checking, so everything after
that point is unchanged: one type table, one manifest, one bytecode file, one
verifier pass, and a plan that sees the whole program.

Loading is the CLI's job, because reading a file is an external effect and
`sic-syntax` may not have one.

### Spans across files

Each file is parsed at an offset, so a `Span` stays what it is - a range of
bytes - and a `SourceMap` turns one back into a file, a line and a column.

The alternative, putting a file id in every `Span`, would touch every
construction of one in the compiler. This way the lexer takes a starting offset
and nothing else changes.

The bytecode's debug section gains the same distinction: it lists the files, and
a position names one. Without that, a failure in an imported file would be
reported against the wrong source, which is worse than reporting nothing.

## 5. What a plan shows

```text
Capabilities:
  process.exec  [exec]  "/usr/bin/deploy"  (not pinned)
    called from lib/deploy.sic
```

Reading a plan should tell you which part of the program uses a grant, or
approving one is approving something you cannot see.

The list comes from the call sites in the bytecode, not from the `requires`
declarations: a declaration says what a file asked for, and the call sites say
where the authority is actually spent. When those two disagree the second one is
the truth. A program built from one file does not get the extra line, because
there is no choice of file to report.

## 6. Not here

- **No registry, no versions, no lockfile.** Section 11 rules them out.
- **No conditional compilation.** A file is in the program or it is not.
- **No re-export.** An import is not transitive: if you use a name, you import
  the file that defines it.
- **No separate compilation.** Everything is compiled together, every time. The
  program is small; when it is not, the fix is caching, not a different model.

## 7. What this changed

The bytecode format goes from 0.1 to 0.2. The `DEBUG` section now begins with
the list of files a program was built from, and every position names one, so a
runtime failure inside an imported file is reported against that file. An older
file is refused rather than half-read, which is the same rule the checkpoint
format follows.

| Code | Means |
|---|---|
| E0212 | `import` without a path |
| E0400 | an import path that cannot be used |
| E0401 | an import that cannot be read |
| E0402 | an import cycle |
| E0403 | a file that both grants and requires capabilities |
| E0404 | a required capability the program does not grant |
| E0405 | a `requires` for a capability nothing calls |

`sic parse` still shows one file, imports and all: it is a view of the parser,
not of the program. `sic hir`, `sic compile`, `sic plan`, `sic run` and `sic
resume` all see the whole program.

The invariant this had to preserve, and does: **nothing reaches an effect that
the manifest of the program being run does not name.** An imported file adds
code, never authority.
