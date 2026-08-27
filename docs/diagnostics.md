# Diagnostic codes

Every diagnostic `sic` reports carries a code. They are grouped by the layer
that produces them, and the ranges are deliberately sparse so a layer can gain a
code without renumbering anything.

**A code names one error.** Not one site: `E0302` is every wrong argument count
anywhere in the checker, and that is one error reported in many places. Two
unrelated errors sharing a code is the thing this rules out, because a code is a
promise to whoever reads the output - somebody who greps this file for what they
were told should find one answer.

A test in `sic-core` fails if a code appears in the source and not in this file,
if a code is listed here and nothing reports it, or if a code is listed twice.
An index that drifts is worse than none.

| Range | Layer |
|---|---|
| E01xx | lexer |
| E02xx | parser |
| E03xx | type checker |
| E04xx | modules |

Failures that happen while a program runs are not codes: they are named in the
message and located by the debug section, because a run has a place as well as a
reason.

---

## E01xx — lexer

| Code | Means |
|---|---|
| E0101 | a character that cannot appear here |
| E0102 | a number literal that cannot be represented |
| E0103 | a string literal that is not closed |
| E0104 | a block comment that is not closed |
| E0105 | an escape sequence that is not one |

## E02xx — parser

| Code | Means |
|---|---|
| E0200 | a token is missing |
| E0201 | an identifier is missing |
| E0202 | something that is not a declaration at the top level |
| E0203 | a block where a statement belongs |
| E0204 | an expression is missing |
| E0205 | `spawn` without a call |
| E0206 | a call policy given twice |
| E0207 | `retry` or `timeout` without a positive number |
| E0208 | `budget` without a positive number |
| E0209 | an unknown setting in an `agent` body |
| E0210 | a word reserved for a later phase |
| E0211 | `sha256` without a digest |
| E0212 | `import` without a path |
| E0213 | `args` without a list of strings |
| E0214 | an expression tree deeper than the parser will build |
| E0215 | `memory` with anything but `task` |
| E0216 | `in` without a directory |
| E0217 | `env` without `NAME: "value"` pairs |
| E0218 | a `log` level that is not one of the four |

## E03xx — names, types and effects

### Names and functions

| Code | Means |
|---|---|
| E0300 | a name that is not defined |
| E0301 | a type that is not the one required |
| E0302 | the wrong number of arguments |
| E0303 | an operator that does not apply to these types |
| E0304 | a function defined twice |
| E0305 | calling something that is not a function |
| E0306 | calling a function whose return type is not settled yet |
| E0307 | a path through a function that does not return |
| E0308 | field access on something that has no fields (superseded by E0341) |
| E0310 | a type name that is not one, or the wrong number of type arguments |
| E0311 | binding a value of type `Unit` |
| E0312 | `null`, which has no type in v0.1 |

### Capabilities

| Code | Means |
|---|---|
| E0320 | calling a capability the module did not grant |
| E0321 | granting a capability that does not exist |
| E0322 | a grant that does not say what it is limited to |
| E0323 | the same capability granted twice |
| E0324 | calling a capability that does not exist |
| E0325 | using a capability as a value |
| E0326 | a pin that is not a sha256 digest |
| E0327 | pinning a capability that cannot be pinned |
| E0328 | pinning arguments on a capability that takes none |
| E0329 | `delegable` on a capability that does not need it |

### Tasks and policies

| Code | Means |
|---|---|
| E0330 | `retry` or `timeout` on something that is not a capability call |
| E0331 | `main` returning a task |
| E0332 | spawning a capability |
| E0333 | awaiting something that is not a task |
| E0334 | `in` or `env` on a capability that starts no process |
| E0335 | `in` with a relative path |
| E0336 | `env` on a `git` grant, which decides its own |

### Records and lists

| Code | Means |
|---|---|
| E0340 | a type that contains itself |
| E0341 | a field that does not exist, or a value with no fields |
| E0342 | an empty list in a position that names no type for it |
| E0344 | a type defined twice |
| E0345 | redefining a built-in type |
| E0346 | a field declared twice |
| E0347 | a struct literal of something that is not a record |
| E0348 | a field the type does not have |
| E0349 | a field given twice in a literal |
| E0350 | a struct literal missing a field |
| E0351 | indexing something that is not a list |
| E0352 | `len` of something with no length |
| E0353 | `from_json` with nothing to say what type to produce |
| E0354 | `for` over something that is not a list |

### Agents

| Code | Means |
|---|---|
| E0360 | an agent declared twice |
| E0361 | an agent whose name is already a function |
| E0362 | an agent without the `llm.invoke` grant |
| E0363 | an agent whose input is not a `String` |
| E0364 | an agent missing `input` or `output` |

### Trust

| Code | Means |
|---|---|
| E0370 | `approve` without the `human.approve` grant |
| E0371 | using a value where its provenance makes it unusable |
| E0372 | a model's answer reaching a capability that changes something |
| E0373 | `choose` without the `human.choose` grant |
| E0374 | `retry` on a capability whose grant does not say the effect can be repeated |
| E0375 | joining two strings whose provenance is not the same |

## E04xx — modules

Produced while gathering the files a program is built from, and while checking
what they ask of each other.

| Code | Means |
|---|---|
| E0400 | an import path that cannot be used |
| E0401 | an import that cannot be read |
| E0402 | an import cycle |
| E0403 | a file that both grants and requires capabilities |
| E0404 | a required capability the program does not grant |
| E0405 | a `requires` for a capability nothing calls |
