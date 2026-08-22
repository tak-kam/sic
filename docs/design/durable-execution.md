# Durable execution (phase 5)

A run stops at an effect that cannot answer now, and continues later - in
another process, after a restart, on another day. The answer to
`human.approve` arrives when a person gets to it, not while the call is on the
stack.

```console
$ sic run examples/approval.sic --checkpoint deploy.sicc --journal deploy.jsonl
run 79368b6cc64da8ca800777c749450807 -> deploy.jsonl
waiting: [deploy to production] deploy build 42?
saved 274 bytes to deploy.sicc
$ echo $?
3

$ sic resume deploy.sicc examples/approval.sic --value true --journal deploy.jsonl
0
```

---

## 1. Why the VM suspends instead of calling out

This was decided in phase 3 and is what makes this phase small. The VM does not
call the broker; it stops, and everything needed to continue is already in
`Vm` - program counter, registers, call stack, arena, the pending call, and
where the journal had got to.

So a checkpoint is **writing out state that exists**, not a second mechanism
beside a synchronous call. Nothing had to be added to the VM to make a run
resumable; the state was already the state of a suspended run.

The same property keeps the VM isolated: there is no `CapabilityHost` trait
that an implementation of an effect could arrive through.

---

## 2. What a checkpoint holds

```text
MAGIC "SICC" | VERSION | program digest (32 bytes)
run id | journal seq | next span | fuel
pending: register, capability, span, parent, question
frames:  func, pc, reg_base, ret_reg, span, parent
registers
string constant handles
arena strings
```

**A checkpoint holds values; the journal does not.** They are different things.
The journal is an account of a run that leaves the process, so it records
digests. A checkpoint is the run itself, so it has to hold the registers and the
arena as they are. Protecting a checkpoint at rest is a separate problem, and
not one that recording less would solve.

The program digest ties the checkpoint to the exact bytecode it came from.
Resuming against anything else would continue one program inside another, so
`sic resume` compiles the source again and refuses if the digest has changed.

---

## 3. A checkpoint is not trusted

It comes from a file, so it can be truncated, corrupt, or hostile. A VM restored
from one must not begin with its invariants already broken, which is the same
contract the bytecode verifier has. Decoding checks:

- the magic, the version, and that nothing is left over at the end
- that a suspended run has at least one frame
- that the pending call writes to a register that exists
- that every frame's registers are inside the saved stack
- that frames are ordered by their register windows, which the register
  arithmetic assumes
- that no value or string constant points outside the saved arena

and restoring adds what needs the program to check:

- that every frame's function exists and its pc is inside that function
- that the constant pool is the size the checkpoint expects

---

## 4. The journal continues

A resumed run is the same run. The checkpoint carries the sequence number and
the next span id, so the events form one stream across however many processes it
takes:

```text
seq 0  run_started
seq 1  function_entered   main
seq 2  capability_requested   human.approve
seq 3  run_suspended
seq 4  checkpoint_written
--- process ends, later another begins ---
seq 5  run_resumed
seq 6  capability_completed   human.approve
seq 7  function_exited    main
seq 8  run_completed
```

The saved sequence number is one past the `checkpoint_written` event, because
that event is written after the state is encoded. No number is ever reused.

---

## 5. Exit codes

`sic run` exits 3 when a run was suspended and checkpointed. Waiting is not
failing, and whatever is driving the run has to be able to tell them apart:
1 means the run is over and went wrong; 3 means it is not over.

A run that has to wait with no `--checkpoint` is an error, because the only
alternative would be to lose it.

---

## 6. Not in this phase

- **No scheduler and no automatic resumption.** Something outside decides when
  the answer exists. Phase 6 brings tasks and a cooperative scheduler, and that
  is where resuming becomes something the runtime can drive.
- **No storage of runs.** A checkpoint is a file whose path the caller chooses.
  `sic runs`, `sic explain` and `sic inspect-run` need a place where runs live,
  which is a decision about state, not about suspension.
- **No timeout on a suspension.** A run waits indefinitely. Deadlines belong
  with the retry and timeout work of phase 6.
- **No encryption of checkpoints.** They hold values, so they need protecting at
  rest, but doing it here without the secret types of section 19 would be
  guessing at what to protect.
