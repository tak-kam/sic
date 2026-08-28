# `sic plan`

What a program may do, said before it does any of it.

```console
$ sic plan examples/agent.sic
Execution plan for examples/agent.sic
bytecode sha256:...

  main
    1. INVOKE   llm.invoke      "claude-opus-4"  at most 2 in a run  any number of tool uses  no deadline of its own   ; 36:13
    2. VERIFY   Diagnosis   ; 36:13

Capabilities:
  llm.invoke      [invoke]  "claude-opus-4"  (not pinned)
    the agent may not  reach the network        (no tool it has can)
    the agent may not  run a shell of its own   (refused by the hook)
    the agent may not  use any other tool       (refused by the hook)

Budgets:
  at most 2 llm.invoke calls in a run, from 1 site: main 36:13

At most 2 capability call(s).
```

This block is checked. `every_sample_the_docs_show_is_what_the_binary_prints`
runs the command and compares, so a change to the renderer reaches this file in
the same commit rather than a week later - which is what happened twice before
the check existed. A line ending in `...` matches any line starting with what
comes before it, which is how the digest stays out of the way: it changes
whenever the compiler emits a byte differently, and a reader learns nothing from
which bytes.

The rule the check follows decides which samples can ever be covered: **a sample
is checked when the command that produced it changes nothing.** `plan`, `verify`,
`disasm`, `parse` and `hir` qualify. The `sic run` blocks elsewhere in these
documents do not, and not because it would be hard - a test that ran them would
be a test with side effects, and most of them name a program nobody wrote.

The point is the same as `terraform plan`: the decision about whether to run
something should be possible **before** running it, from what the program is
rather than what someone says it is.

---

## 1. It reads bytecode, and runs nothing

Everything it needs is already in the file, because each phase put it there for
this:

- the **capability manifest** says what may be reached, with the constraint that
  bounds it (phase 3)
- the **policy table** says how many times a call site may run and how long it
  may take (phase 6, 7b)
- the **type section** says what a `FROM_JSON` will insist on (phase 7a)
- the **debug section** maps every instruction back to a line of source (phase 2)

So `sic plan` is a reader. It opens no socket, starts no process, and does not
construct a VM. That is what makes it safe to run on a program you have not
decided to trust yet - which is the only time a plan is worth anything.

It works on a `.sic` file by compiling it first, or on a `.sicb` directly. The
second form matters: the thing you plan should be the thing you run, and for
bytecode that arrived from somewhere else there is no source to consult.

---

## 2. What it lists

Every effect site, in the order the instructions appear, per function:

| Kind | From | Reads |
|---|---|---|
| `READ` / `WRITE` / `EXEC` / `INVOKE` | `CALL_CAP` | the manifest entry's kind |
| `VERIFY` | `FROM_JSON` | the type it validates against |
| `SPAWN` | `SPAWN` | the function started |
| `AWAIT` | `AWAIT` | - |

`READ`, `WRITE`, `EXEC` and `INVOKE` are the capability's own kind, so the
distinction a plan most needs - does this only look, or does it change
something - is the manifest's answer rather than this tool's guess.

An agent shows as its two steps, `INVOKE` and `VERIFY`, because that is what it
is. Nothing here knows what an agent is either.

---

## 3. What it does not claim

**It does not say what will happen.** It says what may. A call inside an `if`
is listed like any other, with no mark saying it is conditional.

Working out which effects are unavoidable means dominance analysis over the
control flow graph. That is not hard, but a plan that says "these three
definitely, those two maybe" is a different and more useful thing than what this
is, and it should be built when someone is reading plans often enough to want
it. Claiming certainty this cannot establish would be worse than claiming
nothing.

**The bound it does give is honest, and it is narrow.** Only a `budget` bounds
anything over a whole run. `retry` says how many times *one visit* may call
out; how many visits there are depends on the path taken and on recursion, and
this does not analyse either. So the total covers the budgets, and the rest are
counted and named as unbounded:

```text
At most 2 capability call(s).

1 capability call site(s), none with a budget, so how often they run
depends on the path taken.
```

Summing `retry` counts and calling the result a maximum would be a guess dressed
as a fact, which is the one thing a plan must not be.

**A budget belongs to a declaration, so it is totalled once and not once per
site.** An agent called from two places is two `INVOKE` lines and one bound
between them (`agents.md` §6), and the summing this total used to do gave the
double. The lines say whose the number is - `at most 3 in a run, shared by 2
sites` - and a `Budgets:` block names each allowance once with the sites under
it, so the reader is shown that there is one three rather than left to work it
out from the total. That block is the correction; a total that disagrees with
the line above it is not one, because a reader who has just read the same
number twice has no reason to doubt it.

---

## 3a. `--graph`: the one thing a list cannot say

A program whose effect is behind an approval, planned:

```text
  deploy
    1. EXEC     process.exec    "/usr/bin/true"

  rollback
    1. WRITE    fs.write        "./rollback.log"

  main
    1. APPROVE  human.approve   "the deploy"
```

Three blocks, and nothing connects them. From this a reader cannot tell that
`main` calls either of the other two, which of them runs, or that they are
alternatives. §1 opens by saying what a plan is for - the decision about
whether to run something should be possible before running it - and the
decision here is "if I approve, a program runs; if I refuse, a file is
written". That sentence is not in the document.

It was not a rendering oversight. The walk recorded four opcodes and
`continue`d past the rest, and `Op::Call` was in the rest, so **an ordinary
function call was not in the model at all**. `--graph` is one more arm in that
match and a second output.

```text
flowchart TD
    may["may, not will.<br/>Every edge is a path this program has, not one a
         run will take.<br/>Which path, and how often, depends on the answers
         it gets."]
    f0(["deploy"])
    f1(["rollback"])
    f2(["main"])
    c0["APPROVE human.approve - the deploy"]
    c1["EXEC process.exec - /usr/bin/true"]
    c2["WRITE fs.write - ./rollback.log"]
    f2 --> f0
    f2 --> f1
    f0 --> c1
    f1 --> c2
    f2 --> c0
```

### Mermaid, and not the alternatives

DOT needs graphviz installed before a person can look at it. SVG needs a layout
algorithm, which this project could write and which would be its own piece of
work with no dependency argument behind it. Mermaid is text, it is a few dozen
lines to emit by hand, it renders in GitHub, GitLab and most editors with
nothing installed, and where nothing renders it is still readable - which is
the same standard the rest of this binary's output is held to.

### The risk, which is the whole design problem

**A diagram invites a reader to see certainty and order that the plan does not
have.** §3 is careful about this and ends with a sentence; an arrow is much
harder to qualify than a sentence.

#24 made the plan's rule that it must not under-report what a run reaches. A
graph needs the other half stated: **it must not over-claim either.** A reader
who takes `main --> deploy` for "this will happen" has been misled by a
document whose whole purpose is deciding whether to allow it.

Three things follow, and each is a decision rather than a detail:

| | |
|---|---|
| the caption is a node | not a footnote, and not a `%%` comment. It is the first thing in the flowchart, so it is in the reader's way before they have drawn any conclusions |
| `spawn` is dotted **and** labelled | it is a call that does not wait, and a graph that drew it as an ordinary call would describe a different program. A dotted arrow on its own means whatever the reader last saw one mean |
| a grant nothing calls is drawn | in a `granted, and never called` subgraph. It is still a grant - `sic mcp` serves it to the agent answering for the run - so a reader of only the picture is told what a reader of the list is told |

And it is tested the way the under-reporting is: against a recorded run. A run
is recorded, the capabilities it asked for are read out of its journal, and
each one has to be reachable from `main` by following arrows. A graph that drew
every node and lost a path would pass a test that only counted nodes, and fail
a reader.

### One node per grant, not per call site

Two `fs.write` calls in different functions are two arrows into one box. A
grant is what the manifest is about and what a reader is being asked to allow;
a budget belongs to a declaration, and §3 is where the numbers are. Collapsing
sites into grants is what keeps the picture readable at the size a real program
makes it, and nothing is lost that the list does not already hold.

### Branches, which are deliberately not drawn

`main` calls `deploy` *or* `rollback`, and the graph shows both without saying
they are alternatives. Drawing that needs a control-flow graph, which
`sic-plan` does not have - `sic-verify`'s `check_data_flow` does, and moving or
duplicating it is a piece of work with its own argument.

Call edges alone fix the demonstrated gap, which was that nothing said `main`
reached either. If the alternatives turn out to be what a reader actually
needs, that is the next issue to write, and it has this rendering to build on.

---

## 4. Not here

- **No cost or token estimate.** The broker does not report either yet, so any
  number would be invented.
- **No diff against a previous plan.** That needs plans to be stored, which is
  the same question about where state lives that `sic runs` raises.
- **No approval flow.** `sic plan` prints; deciding is a person's job, and
  wiring "approve this plan" to "run exactly this" needs the plan to be
  identified by something - the bytecode digest is the obvious candidate, and it
  is already printed.
- **No picture of a run.** `sic export --traces` produces OTLP, and every trace
  backend draws a recorded run as a waterfall with the durations it carries. A
  second mechanism for that would need an argument this does not have.
- **No picture of the IR.** `sic hir` prints blocks and `sic disasm` prints
  instructions. Both are read by whoever is working on the compiler; a plan is
  read by whoever is deciding whether to run a program, and they are not the
  same reader.
