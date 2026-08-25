# The VM and the broker as separate processes

Section 9 of the specification says the VM and the broker are separate
processes. They are separate crates that cannot see each other, sharing only
the value types in `sic-core`, and they run in one process. Every design
document in this repository has called that shared edge "the future IPC
boundary" since phase 1.

This is the document that decides whether to make it a present one, and it
starts by asking what the split would buy - because most of what a reader
assumes it buys is already true without it.

---

## 1. What is already true

| the claim | what makes it true today |
|---|---|
| the VM cannot reach the outside world | `sic-vm` depends on `sic-core`, `sic-bytecode`, `sic-journal`, `sic-json` and nothing else; `tests/isolation.rs` greps its source for `std::fs`, `std::net`, `std::process`, `std::env`, `std::time`, `std::io` and the macros that read a file at compile time |
| a bug cannot corrupt memory across the boundary | `unsafe_code = "forbid"` in `[workspace.lints.rust]`, inherited by all fourteen crates |
| the values can cross a wire | `CapValue::write`/`read`, `CapRequest::to_bytes`, `answer_to_bytes` - all written and tested, for the agent's socket |
| a framed protocol exists | `sic-broker::route`, length-prefixed with a maximum frame, served without a thread |

So the split does not buy "the VM cannot perform an effect". That is enforced
by the dependency graph and checked by a test, on every platform, with no
runtime cost.

A design document that claimed otherwise would be selling the process boundary
on a property the crate boundary already has.

---

## 2. What it does buy, measured

### The arena has no bound of its own

`docs/design/v0.1.md` chose an arena per run and no GC, deliberately: a run is
short and its memory goes back when it ends. Nothing bounds how large it gets.
What bounds it in practice is `fuel`, and fuel counts instructions rather than
bytes.

That is not theoretical. A program that spawns tasks which recurse and allocate:

```console
$ /usr/bin/time -v sic run mem.sic
error: ran out of fuel
	Maximum resident set size (kbytes): 229760
```

230 MB, and what stopped it was the fuel budget rather than anything about
memory. A bigger record or a longer budget goes further. There is no `--fuel`
flag, so the only bound on this is a constant in the source.

Today that memory is `sic run`'s, and `sic run` is also holding the run store,
the journal sink, the tmux panes and the terminal. A run that goes too far takes
all of it. As a child it takes the child, and the parent still has the journal
it has written, the checkpoint it was handed, and something to say.

**This is the benefit that is real today and it is about resources rather than
about security.**

### Fewer privileges for the side that runs the bytecode

The one thing a crate boundary cannot do is give one side less than the other.
The broker opens files, runs programs and drives panes; the VM needs a socket
and nothing else. A child that has been through `seccomp`, `landlock`,
`pledge` or `sandbox_init` before it reads its first instruction is a different
claim from "we checked that the source does not mention `std::fs`".

It is also the reason to put the *VM* in the child rather than the broker: the
less privileged side is the one that can be given less.

### Two machines

Speculative, and the document should say so. Nothing in the design needs it and
nothing has asked.

---

## 3. What it costs

**The journal has to cross.** The VM emits events; a sink writes them. If the
child writes the file, it needs the filesystem and §2's second benefit is gone
before it arrives. So events cross the wire too, and the parent's sink is the
only one - which is right, and is a second stream to design.

**Checkpoints too.** The state is in the child. `Vm::checkpoint` returns bytes
and the CLI writes them; across a boundary the child sends the bytes and the
parent writes the file. Same shape as the journal.

**Failure gets a vocabulary.** A child that dies mid-call, a child that hangs, a
parent that dies leaving a child. Today none of these exist: a panic in the
interpreter is a panic in `sic run`. Each needs a decision, and "the run failed"
is not a good enough answer for all three.

**Windows has no unix socket.** After #57 the tree compiles and runs there, and
`route` and `tmux` are `#[cfg(unix)]`. A split built on a unix socket is
unix-only, so `sic run` would be one process there and two here.

That last one is the uncomfortable part, and it is worth stating rather than
arranging around: **a security property that holds on one platform and not
another is worse than one that holds nowhere**, because a reader learns it once.

The answer is that this is not the security boundary. §1 is: the crate graph and
the grep, everywhere. This is defence in depth and a resource bound, and defence
in depth that exists on some platforms is ordinary. The document has to say that
in those words, and `sic plan` has no business claiming anything about it.

---

## 4. What does not split

**`sic replay` and `sic recheck` stay one process.** Both answer every call from
a recorded file and reach no broker at all - that is the point of them, and
`runs.md` §4 argues it. A boundary between a VM and a broker that is not there
buys nothing.

This is a rule rather than an exception: **the split is for runs that perform
effects.** Three commands do - `run`, `resume`, `attach` - and three do not.

---

## 5. The shape

The parent is `sic run`. It holds the terminal, the run store, the journal sink,
the manifest and the broker. The child is the VM: it is handed the bytecode and
a socket, and it has nothing else.

```text
sic run p.sic
  ├── compiles, verifies                      (parent, pure)
  ├── spawns `sic vm --socket <path>`         (child)
  ├── sends the program and the entry point
  └── loop:
        child → parent   an event for the journal
        child → parent   a capability request      →  the broker performs it
        parent → child   the answer, or the error
        child → parent   checkpoint bytes
        child → parent   the run's status, and it exits
```

Five message kinds, four of them one way. The framing is `route`'s: a length
prefix with a maximum, refused rather than believed.

`sic vm` is `sic mcp`'s sibling - a command started by a run rather than by a
person, and listed as such.

### Why the child is the VM and not the broker

Because §2 says the point is to give the side that runs the bytecode less than
the side that performs effects, and a parent cannot be given less than its
child. It is also the side whose memory is the problem.

### What the child is not allowed to do

Nothing, once the sandbox exists. Until then it is an ordinary process that
happens not to use the filesystem, which is what §1 already guarantees - so the
first version of this buys §2's first half and not its second, and should say
so rather than implying a sandbox that is not there.

### The first half, demonstrated

Under a 200 MB limit, with the program from §2:

```console
$ sic run mem.sic
memory allocation of 63488 bytes failed
Aborted (core dumped)                                     # exit 134

$ sic run mem.sic --isolate
memory allocation of 63488 bytes failed
error: the interpreter stopped without saying how the run ended;
       the journal has everything it managed to say        # exit 1
```

One process aborts and nothing is left to say what happened. Two processes:
the child aborts, the parent has every event it was sent, and it says so. That
is the whole of what this buys today, and it is worth having on its own.

---

## 5a. A run that stops to wait

```console
$ sic run examples/approval.sic --isolate --record
run c85806cc0897f0c257fca3a8ca412191  recorded in .sic/runs/c85806cc...
waiting: [deploy to production] deploy build 42?
saved 394 bytes to .sic/runs/c85806cc.../checkpoint.sicc
answer with:  sic attach c85806cc --value <VALUE>
```

Word for word what the one-process shape prints, which is the bar: a person
should not be able to tell from the output which shape ran their program.

The bytes are the same bytes too, apart from the run id - which differs between
any two runs of anything. That matters because `sic resume` checks a
checkpoint's digest against the bytecode it is handed, so a checkpoint the child
wrote has to be one the parent would have written.

The digest is of the bytes that *arrived*. Re-encoding what was decoded from
them would be a second opinion about one program, and the comparison on resume
is between the parent's opinion and the checkpoint's.

---

## 5b. Failure, and the case that is not one

Three were expected. Two are real and the third is not.

### A child that dies

Its exit status is the only account left of what happened to it, so it is read
and said, and the three cases are three sentences rather than one:

| | |
|---|---|
| killed by a signal | `the interpreter was killed by signal 6; the journal has everything it managed to say` |
| exited non-zero | `the interpreter exited 2 without saying how the run ended` |
| exited zero | `... which is a bug in sic` |

The first is what this whole arrangement is for. The third is unreachable - a
child that finished cleanly sent an ending first - and it says so, because a
person who meets it should know it is not about their program.

### A parent that leaves

Handled by construction, and now checked. The child waits on exactly one thing,
the socket, so a parent that dies is a read that ends:

```console
$ sic vm --socket /tmp/x.sock          # the run is gone
error: the run went away
```

The test opens a socket, lets the child connect, and drops it. An interpreter
left running with nobody reading its socket is the failure this is meant to
bound, so it is checked rather than reasoned about.

### A child that hangs, which cannot happen

**There is no timeout, and this is the argument for not having one.**

A sic program cannot run forever. Fuel is spent at the top of every
instruction, v0.1 has no loops, and recursion stops at `MAX_FRAMES`. So the
child always reaches either an ending or a read.

Which leaves two ways for it to be quiet for a long time:

- **It is waiting for this side**, and this side is running `cargo test`. That
  is a build that takes an hour, and killing it would be the wrong answer to a
  run that is working.
- **It has a bug.** A timeout there is a guess about how long sic's own bugs
  take, and the guess would kill the first case to catch the second.

`timeout` on a capability call already bounds what a program asked to bound, in
the place that has the clock. A second bound on the interpreter would be a
number nobody chose - which is the thing #52 was about, arriving from the other
direction.

---

## 6. Not here

- **The sandbox.** `seccomp`, `landlock` and their cousins are per-platform and
  each is its own argument. The split is what makes one possible; it is not one.
- **Two machines.** Nothing has asked, and a socket path is not a network.
- **Splitting `replay` and `recheck`** (§4).
- **A Windows transport.** Named pipes would work and nothing needs them yet;
  Windows runs one process and §3 says why that is acceptable here and would
  not be for the boundary in §1.
- **Making the one-process path go away.** It is what Windows uses, what
  `replay` uses, and what every test that builds a `Vm` uses. Two shapes, and
  the second is the one with a wire in it.

---

## 7. Units of work

1. ~~The protocol: message kinds, framing, and a round trip over a socketpair
   in a test. No command yet.~~
2. ~~`sic vm`: the child, reading a program and answering a socket.~~
3. ~~`sic run --isolate`: the parent, behind a flag, so that both shapes are
   exercised while the second is new.~~

   **Done, and as one piece rather than three.** Unit 1 did not finish on its
   own: a protocol with no endpoints is dead code, and this repository has
   said since phase 1 that crates are added in the phase that needs them. A
   unit that leaves `#[allow(dead_code)]` behind is a unit that was split
   wrongly, and saying so is worth more than pretending three commits happened.

   The journal crosses too, because a sink is the parent's and the events are
   the child's. What does not cross yet is the checkpoint.

4. ~~The checkpoint across the wire.~~ **Done.** The child produces the bytes -
   it has the state - and the parent writes them, because it has the
   filesystem. The digest a checkpoint is tied to is of the bytes that
   *arrived*, not of a re-encoding of what was decoded from them: two opinions
   about one program is what `sic resume` would then be comparing.
5. ~~Failure: a child that dies, a child that hangs, a parent that leaves.~~
   **Done, and one of the three turned out not to exist** - see §5b.
6. `resume` and `attach`.
7. The flag becomes the default on unix, and `docs/status.md` moves §9.

Each is a piece of work that finishes on its own, and the first three are what
decide whether the rest is worth having.
