# Concurrency, retry and timeout (phase 6)

Phase 6 adds tasks, a scheduler, and the policies that decide how an effect is
retried and how long it may take.

What is being made concurrent is **waiting**, not computing. A workflow spends
its time on capability calls - a file, a process, later a model or an API - and
the point of running two tasks is that one can proceed while the other waits.
This is not a way to use more cores, and nothing here should be mistaken for
one.

---

## 1. `spawn` and `await`

```text
fn main() -> Int {
    let a = spawn slow(1);
    let b = spawn slow(2);
    return await a + await b;
}
```

- `spawn f(args)` starts `f` as a task and evaluates to `Task<R>`, where `R` is
  what `f` returns. The arguments are evaluated first, in the spawning task.
- `await t` waits for the task and evaluates to its result. Awaiting a task that
  already finished returns immediately. A result is **moved** out of the task
  rather than copied, so awaiting one twice fails - at run time, because seeing
  it at compile time would need the ownership analysis the language does not
  have. The message says so plainly.
- Only a named function can be spawned, which is the same restriction calls
  already have in v0.1.

There is no `parallel { }` block. It would be sugar over these two, and a second
way to say the same thing is worth adding only once the first one is not enough.

### `Task<T>`

`Task` is the first type with an argument, so `Type::Task(TypeId)` joins the
type table and `TypeExpr` arguments stop being rejected. Only `Task` accepts one
in v0.1: a list type is still a separate piece of work.

A task value cannot be stored in a capability argument or returned from `main` -
it means nothing outside the run that owns it.

---

## 2. The scheduler is cooperative

There are no OS threads and no async runtime. The VM holds a set of tasks and
runs one until it cannot continue:

```text
        +-----------+
        |  Ready    |  <-- spawn
        +-----+-----+
              | scheduled
        +-----v-----+
        | Running   |
        +--+-----+--+
           |     |
  CALL_CAP |     | AWAIT on an unfinished task
           v     v
    +------+-+ +-+---------+
    |Waiting | | Waiting   |
    |on a cap| | on a task |
    +------+-+ +-+---------+
           |     |
           +--+--+
              | answer arrives / awaited task finishes
              v
           Ready
```

A task yields at exactly two instructions, `CALL_CAP` and `AWAIT`, and nowhere
else. Preemption would mean the VM could stop between any two instructions,
which would make every intermediate state something a checkpoint has to be able
to represent. Yielding only where the task is already waiting keeps the set of
suspension points small and named.

Scheduling is round-robin over the ready tasks. It is deterministic: the same
program with the same answers schedules the same way, which replay depends on.

When no task can run, the VM suspends with the capability requests it is waiting
on, exactly as a single-task run does today. The driver answers them and the run
continues.

### What a run looks like

```text
seq= 7 task=1 capability_requested   cap=process.exec
seq= 8 task=2 capability_requested   cap=process.exec
seq= 9 task=1 capability_completed   cap=process.exec
seq=12 task=2 capability_completed   cap=process.exec
```

Both requests are recorded before either answer, because a request is recorded
where the instruction runs rather than where it leaves the VM. That is the
whole visible effect of the scheduler: while one task waits, the other runs.

### Registers

Each task needs its own register stack, so the single `Vec<Value>` of phases 2
to 5 becomes one per task. The arena stays shared: a value passed to `spawn` or
returned from `await` crosses between tasks, and copying arenas would mean
copying every string on every handoff.

---

## 3. `SPAWN` and `AWAIT`

Two instructions, both ABC:

```text
SPAWN  a, b, c    ; R[a] = task running F[b](R[c .. c+argc])
AWAIT  a, b       ; R[a] = the result of the task in R[b]
```

`SPAWN` has the same shape as `CALL`, arguments in consecutive registers, for
the same reason.

### The type section grows

The verifier has to know that `AWAIT` on a `Task<Int>` produces an `Int`, so a
type tag can no longer be one byte. `TYPES` becomes a list of descriptors:

```text
tag u8   ; 0 unit, 1 bool, 2 int, 3 float, 4 str, 5 task
[ u32 ]  ; for task: the index of the type it produces
```

The first five entries stay in tag order, so a primitive is still its own index,
and `Task<T>` entries are appended. This is what the `TYPES` section was for in
the original design; phase 2 only ever needed the primitives.

---

## 4. Retry and timeout

Phase 3 left `CallPolicy` in the IR and deliberately did not fill it, on the
grounds that retrying inside the broker puts a workflow decision in the wrong
place. This is where it gets filled.

```text
let text = fs.read("./flaky.txt") retry 3;
let code = process.exec("/usr/bin/slow") timeout 500;
```

- A policy is written directly after a call, and only after one. It is not an
  operator and cannot appear anywhere else, which keeps the grammar and the
  precedence table untouched.
- `retry N` means up to N attempts in total, not N extra ones.
- `timeout N` is milliseconds.
- **A policy may only be attached to a capability call.** Retrying a pure
  function computes the same answer again; the diagnostic says so.

### Where each one is enforced

They are enforced in different places, and for a reason.

**Retry belongs to the VM.** It knows the attempt count, it re-issues the
request, and every attempt is a journal event, so an audit shows what actually
happened. A broker that retried on its own would hide attempts from the run's
own account of itself.

**Timeout belongs to the broker.** It is the only side with a clock, and the VM
must stay unable to read one. The timeout travels in the request; a broker that
cannot honour it for a given capability fails the call rather than ignoring it.
For `process.exec` that means killing the child and reporting a failure.

The policy also travels in the bytecode, attached to the `CALL_CAP` site, so
`sic plan` can eventually say "this call may run three times, each up to half a
second" without executing anything.

---

## 5. Checkpoints hold tasks

A suspended run now has several tasks, so a checkpoint holds them all: each
task's registers, frames, state, and pending call. The format version goes to
0.2; a 0.1 checkpoint is refused rather than half-understood.

The rest of the reasoning is unchanged from phase 5. The checkpoint is still
the run's state written down, still tied to its bytecode by digest, and still
read back with the same suspicion as bytecode.

---

## 6. What the journal records

```text
TaskStarted   { task, func, parent }
TaskCompleted { task, result digest }
TaskFailed    { task, error }
```

plus `attempt` on the capability events, so a retried call shows each attempt
rather than only the one that worked.

`task` is the field that has been zero in every event so far. It stops being
zero here, which is why it was in the model from the start.

---

## 7. What failure means with tasks

A task that fails does not by itself fail the run: the failure surfaces when the
task is awaited, and the awaiting task fails then. A task nothing ever awaits
can fail unnoticed, so the run reports it at the end - silently discarded
failures are how a workflow ends up claiming to have succeeded.

If the main task finishes while other tasks are still running, the run ends.
Those tasks are abandoned, and each is recorded as such. Waiting for them
instead would mean a program could not choose to stop early.

---

## 8. Not in this phase

- **No OS threads and no async runtime.** Cooperative scheduling is what the
  specification asks for at this stage, and adding tokio would mean adding the
  dependency tree the whole project is arranged to avoid.
- **No preemption and no fairness beyond round-robin.** A task that never calls
  a capability runs to completion; fuel is what stops it running forever.
- **No cancellation.** `await` has no way to give up, and a task cannot be
  killed. Cancellation interacts with retry, timeout and checkpoints all at
  once, and each of those has to exist first.
- **No parallel capability calls in the broker.** Two tasks waiting on effects
  suspend the run once and are answered one after the other. Answering them at
  the same time is a broker change, not a language one, and it is worth making
  when there is a capability whose latency justifies it.
- **No backoff.** `retry N` retries immediately. Backoff needs a clock in the
  place that schedules the retry, and that place is the VM.

---

## 9. Units of work

| # | Unit | Done when |
|---|------|-----------|
| 6-1 | `Task<T>`, `spawn` / `await` syntax, type checking | a task cannot be awaited twice, and a policy on a pure call is rejected |
| 6-2 | Type descriptors in the bytecode, `SPAWN` / `AWAIT` | the verifier knows `await` on a `Task<Int>` gives an `Int` |
| 6-3 | Tasks in the VM, the scheduler | one task proceeds while another waits on a capability |
| 6-4 | Retry in the VM, timeout in the request | a retried call shows every attempt in the journal |
| 6-5 | Timeout in the broker | a slow child process is killed and reported |
| 6-6 | Tasks in checkpoints, format 0.2 | a run with several tasks survives being written out |
| 6-7 | Journal task events, `attempt` on capability events | the task field stops being zero |

### A note on deadlock

`FailKind::Deadlock` exists and the scheduler reports it, but v0.1 has no way to
reach it: a task can only be awaited by whoever holds its `Task<T>`, and a task
cannot be given a handle to itself or to the task that spawned it. The check is
there because that will stop being true the moment tasks can be passed around,
and a scheduler that hangs is worse than one that says why it stopped.
