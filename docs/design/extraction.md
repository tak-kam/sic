# When a function is worth splitting, and when its length is not the reason

The longest functions in the tree are these:

| lines | where | what it is |
|---|---|---|
| 433 | `sic-vm/src/lib.rs` `run_task` | one arm per opcode |
| 373 | `sic-verify/src/lib.rs` `check_data_flow` | one arm per opcode |
| 256 | `sic-ir/src/lower.rs` `expr` | one arm per expression kind |
| 210 | `sic-otel/src/traces.rs` `traces` | one arm per journal event |
| 192 | `sic-verify/src/lib.rs` `check_structure` | one arm per opcode |
| 190 | `sic-types/src/check.rs` `collect_capabilities` | one check per thing a grant may say |
| 185 | `sic-otel/src/metrics.rs` `metrics` | one arm per journal event |

Six of the seven are a single exhaustive `match`, and they should stay that
way. This document exists so that the next survey of the codebase finds the
argument rather than the line count.

Line numbers are left out on purpose: a table of them is wrong by the next
commit, and a document that carries a checkable fact it does not check is worse
than one that does not carry it. The counts drift too - four opcodes were added
in 0.4.0 and `run_task` grew by 76 lines, every one of them an arm - so they
are refreshed when this document is read rather than trusted between times.

`collect_capabilities` is the seventh and is not a `match`. It is a loop over
every grant in an `allow` block with one check per thing a grant may say - a
constraint, a `sha256`, an `args` prefix, `delegable`, `in`, `env`,
`repeatable` - each producing one field of the entry it builds. §3's rule
covers it for the same reason: no two of those checks share a procedure, so
extracting one would move the code without letting anything be checked in one
place. What it *has* shown is a different problem, and one worth fixing: which
family a capability belongs to is re-derived from its name by string prefix in
nine places across three crates.

## 1. An arm is not a piece of work

An arm of an exhaustive `match` over the instruction set is not an independent
thing that happens to live inside a function. It is one row of a table whose
completeness the compiler enforces.

Extracting the arms of `run_task` into `exec_add`, `exec_call`, `exec_call_cap`
and the rest leaves a `match` that is thirty lines of dispatch to thirty
functions: the same thirty cases, the same total size, and one more indirection
between "what does `CALL_CAP` do" and the answer. The largest function in the
tree gets smaller and nothing else improves.

It also costs the property that makes these functions safe to change. Reading
`run_task`, every case is on the screen or a scroll away, in the order the
opcodes are numbered; adding an opcode puts the new case exactly where the
compiler already demands one.

## 2. What extraction is for

The distinction is not length. It is whether the extracted thing is **shared**.

Two extractions in this tree were worth making, and both have the same shape:
more than one arm reached the same procedure, and the order between the steps
mattered.

**`begin_capability_call` and `charge_budget`** came out of `run_task`, taking
it from 397 lines to 331. The bug that prompted it was that the budget charge
was recorded and *then* the call was refused, so a refused call left a charge
behind. Naming the two steps is what made the order between them something a
reader could check. See issue #32.

**The verifier's register-window check** was written out five times, and one of
the five started at the wrong register. See issue #33.

Neither was an arm. Both were procedures several arms shared.

Applying the same reading to what is left: there is no shared procedure of any
size in the six functions above.

## 3. The rule

> A `match` arm is not extracted for length. A procedure that two or more arms
> share is extracted, so that the order between the steps can be named and
> checked in one place.

A function that is long because it is a table is finished. A function that is
long because it repeats itself is not, and its length is a symptom rather than
the problem.

## 4. Not here

**A line-count lint.** Any number would be wrong in both directions. It would
fire on all six of the functions above, which are correct as they are, and it
would not fire on the seventy-five line stretch inside `run_task` that #32 was
actually about - that stretch was never the longest thing in the file.

**Splitting the six by file instead of by function.** `sic-vm/src/lib.rs` is
1,744 lines and holds the VM's state, its scheduler, its interpreter and its
failure vocabulary. Whether those are four files is a separate question from
whether `run_task` is one function, and it is not answered here.

**A rule about `impl` blocks or modules.** This is about one function and the
`match` inside it. Nothing above generalizes to "small is better"; the whole
point is that one of the two kinds of long function is not a problem.
