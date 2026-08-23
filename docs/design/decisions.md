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

## 6. Not here

- **The reason.** "The third one" is the decision; *why* the third one is what
  anybody reading it later actually needs. Carrying free text alongside an
  answer - `sic attach <id> --value 2 --because "..."` - is an addition to the
  run store and to `sic explain`, not to a capability, and it is the next piece
  of this rather than part of it.
- **Choosing more than one**, ranking, or editing an option before choosing it.
- **Options that are not strings.** A person reads the alternatives, so they are
  text. Choosing between records would need a way to render one for a human,
  which is a different feature.
- **Generating the options.** A model producing them is `agent` plus this, and
  needs nothing new.
