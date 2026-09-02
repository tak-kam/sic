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
    logs.jsonl        what the program said, if it said anything
    checkpoint.sicc   present only if the run is waiting
    driver.json       what answered its model calls, if anything did
    conversations     which agent conversations it has open, if any
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

Where a person answered, the line also holds the question they were asked and,
if they gave one, the reason: `docs/design/decisions.md` §6 says why free text a
person wrote belongs in this file rather than in the journal.


`logs.jsonl` is the same split used a second time. A log line is a value the
program wrote, so the journal keeps its level and its digest and the text lives
here - which is what let §26 be built at all, because putting the text in the
journal would have cost the rule that makes telemetry safe by default. See
`docs/design/logging.md`.
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

### It refuses a journal and a program that do not belong together

A replay reads `program.sicb` back off a disk anybody could have written to
since. If those bytes are not the bytes the journal is a record of, the
comparison answers a different question, and every difference it reports is
noise that reads exactly like the finding above - a determinism bug that is not
one.

`RunStarted` names the bytecode (#88), so the pair can be checked before
anything runs:

```console
$ sic replay d0b0d9b5
error: this journal was recorded from different bytecode
  recorded sha256:bc04a041...
  stored   sha256:5d8ff875...
```

Both digests, because a reader told only that they differ cannot tell which of
the two files is the one they did not expect. This is the same claim a
checkpoint has made since it had a digest, about the other artifact; §1 already
said a run's directory holds three files, and this is what makes two of them
provably about each other.

Suspending, checkpointing and resuming are left out of the comparison. They
record how a run was carried out - in how many sittings the answers arrived -
rather than what the program did. A run that waited two days for a person is the
same run as one answered immediately, and a replay that called those different
would report a difference nobody can act on.

A logged line *is* in the comparison, and it is compared as a digest. It is the
program talking, so which lines it wrote is a fact about the path it took - and
`docs/design/harness.md` found that the programs which log are the programs a
harness is made of, so leaving them out would give the check almost nothing to
say about the programs it exists for. What made that awkward is §2: the file
holds the digest of a message and the VM emits the text, which are two spellings
of one thing, and comparing them made every replay of every program that logs
report a difference - issue #82, unnoticed because nothing here replayed a run
that logged. The replayed event is put in the form the file keeps before the two
are compared. Comparing the digests is also what keeps the rule intact:
establishing that two runs said the same thing never requires either side to
produce what was said, and the report names two digests rather than two
sentences.

```console
  seq 3: recorded logged info sha256:2cf24dba, replayed logged info sha256:82e35a63
```

Two ways a replay can legitimately end early:

- **The run was suspended.** The recorded answers stop where the run stopped;
  replay stops there too and says so.
- **The replay asks for a call the recording does not have.** That is itself the
  finding: the program took a different path.

---

## 5. `recheck`: a recorded run is a test case for the program

```console
$ sic recheck 9b41d0 deploy.sic
rechecking 9b41d0 (main) against deploy.sic
  ✓ 11 of 12 calls matched
  ✗ call 12: the recording answered `process.exec`, this program asks `fs.read`
```

`replay` re-runs the *stored* bytecode and establishes determinism. `recheck`
compiles a source file and runs *that* against the same recorded answers. It is
a different claim, so it is a different verb rather than a flag: `replay`
failing means sic changed, `recheck` failing means the program did.

### What it is actually for, which is not what it sounds like

"Does my edit break the run that is waiting" is the question, and the literal
answer is no - because of §6 and issue #11. A recorded run keeps its own
bytecode, and `sic attach` resumes it against that rather than against whatever
the source compiles to today. Editing the file cannot reach a recorded run that
is already waiting.

What an edit can do is make the program stop being the program those runs were
runs of. Every recorded run is a case the program has actually been through,
with real answers, and the useful question before shipping an edit is whether
those answers still fit. That is Temporal's practice, arrived at from the same
place: pull recent histories, run them against current code, fail the build if
any of them no longer lines up. sic has the same three files sitting in the run
directory already - `program.sicb`, `journal.jsonl`, `responses.jsonl` - and
replaying a *different* program against the same answers is the same machinery
with one substitution.

### What counts as a difference

Not the journal. The program is deliberately different, so most of the journal
differs and almost none of it means anything: spans are renumbered, function
digests move, an edit to a comment changes the bytecode.

What is compared is the sequence of capability calls, by **name and argument
digest**, which is the shortest statement of what a recording is worth:

> Every recorded answer is still being given to the same question.

If the calls line up, the recorded answers still apply and the recording is
still a case this program passes. If call twelve asks `fs.read` where the
recording answered `process.exec`, the twelfth answer is being handed to a
different question, and using it would prove nothing.

A digest rather than the arguments themselves, for the reason the journal
records digests at all: a run's arguments may hold a secret, and `recheck` reads
the journal.

### Running out, and the one case where that is fine

**The recording runs out.** The edited program asks for a call the recording has
no answer for. Usually the finding - the program now does more than it did.
Except when the recorded run was **suspended**: then the recording stops where
the run stopped, by construction, and running out at exactly that point is the
recording ending rather than the program diverging. `recheck` knows which,
because the journal says so.

**The program runs out.** The edited program stops before the recorded answers
do. Always a finding: the recording went somewhere this program no longer goes.

**The program fails.** A finding, and the plainest one there is.

### What it must never do

Call anything, and refuse `--llm`. The same rule as `replay`, for a stronger
reason: a check that reached a live agent would be answering the question with
a different agent's answer, which is not the question. Every call is answered
from `responses.jsonl` or it is a difference.

The compiled bytecode goes through the verifier before it runs, like every other
path into the VM.

### Exit codes

`0` when every recorded answer still applies, `1` when one does not. So a
directory of recorded runs is a test suite:

```console
$ sic runs | awk '{print $1}' | while read id; do sic recheck "$id" deploy.sic || exit 1; done
```

---

## 6. Picking a waiting run up again

A run that stopped is detached, in the sense a terminal multiplexer means: it
exists, it is not attached to a process, and something can come back to it.

```console
$ sic runs --waiting
b4b6776d  main  llm.invoke  [claude-opus-4] what should we deploy?

$ sic attach b4b6776d
waiting: [claude-opus-4] what should we deploy?
answer:  sic attach b4b6776d --value <String>

$ sic attach b4b6776d --value '{"action": "restart the service"}'
waiting: [deploying] deploy this?
```

Everything needed is in the run's directory, so a run is named by its id and
nothing about a path has to be remembered.

**Reading the question is a separate step from answering it**, and that is the
half that makes this usable by something other than a person who already knows
what the run wants. Whatever answers - a person, or an agent driving `sic` -
has to be able to find out what is being asked first. `sic attach` with no value
prints the question and exits 3; with a value it answers and carries on.

That is also why `llm.invoke` deferring is not a limitation to be fixed later.
The thing outside that answers a model call can be whatever is driving `sic`,
and it finds its work with `sic runs --waiting`.

### A journal cut mid-write says so

A journal is append-only and a run can be killed while writing one, so its last
line may be a fragment. It is skipped rather than refused, because refusing to
look at a run whose last line is half-written would refuse exactly the runs
worth looking at.

But every command that reads a recorded run now says which lines it could not
read, and the reason is which line that usually is. The last line of a journal
is `run_completed`, `run_failed` or `run_suspended`, and those three are the
only ones an outcome is read from - so a run that was waiting becomes
`unfinished`, drops out of the list above without a word, and whatever was going
to answer it never learns that anything was missing. `sic replay` has the same
problem wearing a different hat: it reports the missing event as a determinism
finding against the VM.

A warning does not make `unfinished` correct. It makes it visibly uncertain,
which is all the reader needs.

---

## 7. Not here

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
- **`recheck` over every run at once.** A shell loop is one line and says what
  it does. A built-in would have to decide which runs, in what order, and what
  to print when half of them differ - three policies, none of which anybody has
  wanted yet.
- **No repair, no suggestion, no diff of the source.** `recheck` says where the
  edited program stopped matching. What to do about it is the edit, and sic has
  no opinion about somebody else'"'"'s program.
