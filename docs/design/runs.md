# Stored runs

`sic run --record` keeps a run, and `sic runs`, `sic explain`, `sic inspect-run`
and `sic replay` are what you do with one afterwards.

```console
$ sic run app.sic --record
run 3f2a...  recorded in .sic/runs/3f2a...

$ sic runs
3f2a8c...  main   completed  2 capability calls
9b41d0...  main   failed     the document does not fit the type

$ sic explain 9b41d0
$ sic replay 9b41d0
```

---

## 1. Where a run lives

```text
.sic/runs/<run-id>/
    program.sicb      the bytecode that ran
    journal.jsonl     what happened
    responses.jsonl   what the broker answered
    checkpoint.sicc   present only if the run is waiting
```

- **A directory per run, named by its id.** The id is already 128 bits and
  already in every journal line.
- **In the project, not in the home directory.** A run belongs to the program it
  ran, and a program lives in a repository. `SIC_RUNS` overrides the path for
  anyone who disagrees.
- **Recording stays opt-in.** Phase 4 decided that, and nothing here is a reason
  to change it: a run that was not asked to keep anything leaves nothing behind.

The bytecode is stored beside the journal because replaying needs the exact
program, and "the file on disk now" is not it. It is the same reasoning that put
a digest in the checkpoint.

---

## 2. `responses.jsonl` holds values, and the journal still does not

Replaying a run means answering its capability calls the way they were answered
the first time, and a digest cannot do that. So the values go in a second file.

This is the same split as a checkpoint: the journal is an account of a run that
leaves the process, so it records digests; `responses.jsonl` is the run's own
material. Keeping them apart means the file that is safe to ship stays safe to
ship, and the one that is not is one file, named, in a directory you can delete.

A recorded run therefore contains what its capabilities returned. That is a
decision the `--record` flag makes explicitly, and it is why the flag exists
rather than recording always.

---

## 3. `explain` and `inspect-run`

`explain` is the summary a person reads when something went wrong: the workflow,
how it ended, what it called and what came back, one line each, indented by span
depth so the shape is visible.

`inspect-run` is every event, in order, unabridged. When `explain` leaves out
the thing you needed, this is where it is.

Neither runs anything.

---

## 4. `replay`

```console
$ sic replay 9b41d0
replaying 9b41d0 (main)
  ✓ 14 events matched
  ✗ seq 9: capability_completed fs.read
      recorded sha256:31ddb3...
      replayed sha256:af5570...
```

Replay re-runs the stored bytecode, answering each capability call from
`responses.jsonl` instead of asking the broker, and compares the journal it
produces against the one that was recorded.

What that establishes is **determinism**: given the same program and the same
answers, the VM does the same thing. A difference is a real finding - the VM
changed, the compiler changed, or something in the run was not as deterministic
as it claimed to be.

What it does not do is call anything. A replay that asked the broker again would
be a second run, with a second set of effects, which is the opposite of what
replaying is for.

Two ways a replay can legitimately end early:

- **The run was suspended.** The recorded answers stop where the run stopped;
  replay stops there too and says so.
- **The replay asks for a call the recording does not have.** That is itself the
  finding: the program took a different path.

---

## 5. Not here

- **No pruning, no retention, no size limit.** A run directory is a directory;
  deleting old ones is `rm`. Anything cleverer is a policy, and policies belong
  where someone can see them.
- **No index.** `sic runs` reads the directory. When that is too slow there will
  be enough runs to know what an index should be keyed by.
- **No replay of a partial run into a live one.** Replaying stops where the
  recording does; continuing from there is `sic resume`, which already exists
  and takes a checkpoint.
- **No redaction.** `responses.jsonl` holds what the capabilities returned. If
  that must not be kept, do not pass `--record`.
