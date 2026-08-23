# Decisions

`approve` asks a person one thing: yes or no, about a value that already exists.
That is the last step of a workflow. The step before it is choosing between
things that do not exist yet, and nothing in sic could hold one.

```sic
allow {
    human.choose "the design decision";
}

fn approach() -> HumanChosen<String> {
    return choose("how should imports handle capabilities?", [
        "the importing program grants everything",
        "grants are unioned",
        "a library declares, the importer approves",
    ]);
}
```

Every document under `docs/design/` ends with what was deliberately left out and
why; `sic plan` says what a program may do before it does it; the journal is the
runtime's account of what happened and on what grounds. All three are about
**grounds**, and the one kind of ground a workflow could not record was a
judgment somebody made.

---

## 1. The answer is an index, not a value

`human.choose(question: String, options: List<String>) -> Int`.

The capability returns **which** option, and the VM reads the value out of the
list the program itself built. Nothing that answers - a broker, a person at a
terminal, a recorded run being replayed - can hand back a string.

That is what makes the type honest. `HumanChosen<String>` says a person picked
one of these, and it is true by construction: the worst an answer outside the
range can do is fail the run, because `GET_INDEX` is bounds-checked and the VM
does not re-check what it can already refuse.

It also settles what would otherwise be a real question - what happens when
somebody answers with something that is not on the list. Nothing happens,
because there is no way to say it.

The alternatives are shown to whoever answers **numbered from zero**, because
the number they read is the number they type and the answer is an index.
Counting from one would put an off-by-one in a translation layer forever.

---

## 2. `choose` is a builtin, like `approve`

A capability signature is a fixed list of types, so it cannot produce a trust
type over what it was given. `approve` is already a builtin for that reason, and
`choose` follows it exactly: the checker types the call, and lowering turns it
into an ordinary capability call plus the instruction that uses the answer.

```text
choose(q, opts)  ->  CALL_CAP human.choose(q, opts) -> Int
                     GET_INDEX opts[i]
```

Two consequences worth naming:

- **The options cross the boundary as an argument vector**, which is why this
  waited for `docs/design/arguments.md`. A decision is a capability call whose
  arguments are the alternatives.
- **The journal records it like any other call**: the digest covers the question
  and every option, so two runs that offered different alternatives do not look
  the same afterwards.

---

## 3. Choosing is not approving

`human.choose` is its own grant. Approving a value and deciding between
alternatives are different acts, and a plan that showed them as one would be
answering a question nobody asked.

```text
    1. CHOOSE   human.choose    "the design decision"  3 options   ; 14:19
    2. APPROVE  human.approve   "deploying"                        ; 22:5
```

Deciding is a person's job. `sic plan` telling them how many decisions a run
will ask of them, before it starts, is the same promise the rest of the command
makes about effects.

---

## 4. What a chosen value may do

`HumanChosen<T>` carries no restriction, and that is not an oversight.

`LLM<T>` and `Observed<T>` are values nobody wrote down and nobody signed off:
what a model answered, what a program printed. A chosen value is the opposite of
both. **Its text was written by whoever wrote the program, and a person selected
it from that list.** It cannot contain anything the source does not contain, so
there is nothing for a rule to protect against - it may reach a capability that
writes or executes, and the record says who chose it.

It is not `HumanApproved<T>` and does not convert to one. Approving *this value*
and choosing *among these* are different claims, and a signature that asks for
one is not asking for the other.

---

## 5. A decision with no alternatives is not one

An empty list fails the call rather than the compile, because the list may be
computed. The message says what happened; there is no sensible index into
nothing.

---

## 6. The reason is worth more than the choice

"The third one" is the decision. *Why* the third one is what anybody reading it
later actually needs, and it is the part that would otherwise survive only
because somebody typed it into an issue by hand.

```console
$ sic attach 7f3a --value 2 --because "the only one where reading a plan still tells you the truth"
```

Three things follow from where it is written down.

**It goes in `responses.jsonl`, not the journal.** The journal records digests,
never values, so that a secret cannot reach telemetry by default. A reason is
free text a person wrote, which is exactly the kind of thing that rule protects.
`responses.jsonl` is the run's own material - the file that is already not safe
to ship, one file, named, in a directory you can delete.

**The question is recorded with it.** A recorded answer used to be a bare value,
and an index on its own says nothing six months later. The question the person
was asked carries the numbered alternatives, so writing it down is what keeps
*what was not chosen* - which is the whole value of a recorded decision, and the
same thing every document in this directory ends with.

```json
{"value":"contents"}
{"value":2,"asked":"[the design decision] how should imports …\n  0. …\n  1. …","because":"…"}
```

A line has an `asked` exactly when a person answered. The broker's own answers
have none, because nobody was asked.

**Replay ignores both.** It needs the values and nothing else, so a recorded
reason changes nothing about what re-running does.

`sic explain` then has something to print that is not a digest:

```text
  asked a person:
    [the design decision] how should imports handle capabilities?
      0. the importing program grants everything
      1. grants are unioned
      2. a library declares, the importer approves
    answered 2
    because the only one where reading a plan still tells you the truth
```

`sic resume` takes no `--because`, and says so rather than accepting one. It
works from a checkpoint file, which is a run's state and not a run's record;
there is nowhere in it for a reason to live. Recording one is what the run store
is for, and `sic attach` is the way in.

---

## 7. Not here

- **Choosing more than one**, ranking, or editing an option before choosing it.
- **Options that are not strings.** A person reads the alternatives, so they are
  text. Choosing between records would need a way to render one for a human,
  which is a different feature.
- **Generating the options.** A model producing them is `agent` plus this, and
  needs nothing new.
