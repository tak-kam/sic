# `sic plan`

What a program may do, said before it does any of it.

```console
$ sic plan examples/agent.sic
Execution plan for examples/agent.sic
bytecode sha256:a99e44d39ed16d77f1ca5a048b4b2258b37a1be3148091a303a16ad46eb9842d

  main
    1. INVOKE   llm.invoke      "claude-opus-4"  at most 2 in a run   ; 36:13
    2. VERIFY   Diagnosis   ; 36:13

Capabilities:
  llm.invoke      [invoke]  "claude-opus-4"  (not pinned)
    warning: this grant says what the program may ask for, not
             what the agent may do while answering

At most 2 capability call(s).
```

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

**The bound it does give is honest, and it is narrow.** Only a `budget` bounds a
call site over a whole run. `retry` says how many times *one visit* may call
out; how many visits there are depends on the path taken and on recursion, and
this does not analyse either. So the total covers the budgeted sites, and the
rest are counted and named as unbounded:

```text
At most 2 capability call(s).

1 capability call site(s), none with a budget, so how often they run
depends on the path taken.
```

Summing `retry` counts and calling the result a maximum would be a guess dressed
as a fact, which is the one thing a plan must not be.

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
