# Driving an agent CLI

`llm.invoke` defers. The run suspends, a person reads the prompt, pastes an
answer back, and the run continues. That was the honest thing to do while the
only way to reach a model was HTTPS, which this project will not write by hand.

A local agent CLI is not HTTPS. It is a program that already holds its own
credentials and can be driven in a terminal. Non-interactive mode is not
available on every plan, so the interactive one is what there is - which means a
pty, which means a multiplexer.

```console
$ sic run triage.sic --llm tmux:claude
```

Nothing about the program changes. `agent`, `LLM<T>`, `budget`, `from_json`,
`retry` and the journal keep working, because the prompt and the answer still
cross the capability boundary exactly where they did.

---

## 1. The broker owns the pane

The alternative was to let a program drive the multiplexer itself, which
`process.exec` and `process.capture` can already almost do:

| | `process.exec "/usr/bin/tmux"` from a program | the broker owns the pane |
|---|---|---|
| what the program is granted | drive every pane on the machine | `llm.invoke "claude"` |
| what the journal records | `EXEC tmux -> 0` | the prompt and the answer, as digests |
| `sic replay` | replays nothing that matters | replays the conversation |
| what the value is typed as | `String` | `LLM<T>` |

The first row is the one that decides. A multiplexer is a deputy that can reach
every pane on the machine, including the one the person running `sic` is typing
in, and `args [...]` scopes that deputy without disarming it. Keeping the
multiplexer inside the broker means no program is granted it at all.

No TLS, and no pty of our own: tmux provides the pty, and the agent holds its
own credentials.

---

## 2. Nothing answers unless it was asked for

`--llm tmux:claude` is required. Without it `llm.invoke` defers exactly as it
does today, and that is not a fallback - it is what the capability means when
nobody named a driver.

A driver that starts answering model calls because it happened to be installed
is the shape of failure this project is arranged against. Detecting `claude` on
the machine and using it would make what a run did depend on what was lying
around, which is the same argument as "no PATH-based executable resolution".

The spec is `<multiplexer>:<agent>`. `tmux` is the only multiplexer, and the
agent is either

- a **bare name**, resolved against a compiled-in list of absolute paths, the
  way `sic upgrade` resolves `curl` - never against `PATH`; or
- an **absolute path**, used exactly as written.

### The grant has to name the same agent

```sic
allow {
    llm.invoke "claude";
}
```

The constraint must equal the agent's name - the bare name as written, or the
file name of the absolute path. A program that says `llm.invoke "gpt-5"` run
under `--llm tmux:claude` fails the call rather than being answered by something
it did not ask for. The manifest would otherwise record a claim that was not
true, which is worse than the run not happening.

---

## 3. One call, one pane

For each call the driver

1. opens a detached window running the agent, in the run's session (§9),
2. pastes the prompt and presses Enter,
3. polls the pane until the answer is complete,
4. kills the window.

A person can watch it happen (`tmux -L sic attach -t <session>`), which is the
seed of the run-level session in §9 rather than a feature of this part.

The pane is closed when the call returns, and the reason is what the journal
holds: for a call with no memory the prompt and the answer are the whole story,
so the pane keeps nothing the record does not. A pane worth keeping alive is one
carrying accumulated context, and that is `memory: task` - §9.

### The tmux server is sic's, not the person's

`-L sic` puts the driver on its own socket, `-f /dev/null` ignores the person's
`tmux.conf`, and `-u` says the pane is UTF-8. Three reasons:

- **The environment.** `new-session` in an existing server runs in *that
  server's* environment, not in the one `sic` was started with. A driver that
  cannot say what environment the agent got cannot say what the agent could
  reach.
- **The configuration.** A status line, a different default shell, or
  `set -g mouse` change what `capture-pane` returns. Reading a TUI is fragile
  enough without also reading somebody's dotfiles.
- **The encoding.** tmux otherwise decides whether a pane is UTF-8 from the
  locale it was started with, and an answer that depends on whether `LANG` was
  set is an answer that arrives as mojibake on the machine that did not set it.

### What the agent inherits

The environment is cleared down to a named list: `HOME`, `PATH`, `TERM`,
`LANG`, `LC_ALL`, `LC_CTYPE`, `USER`, `SHELL`, `TMPDIR`.

The pane starts in the directory `sic` was started in. A coding agent reads the
directory it is in, so that is what decides what it can see, and leaving it to
whatever the multiplexer picked would leave that to chance.

`HOME` is there because that is where the agent's own login lives, and `PATH`
because the agent runs tools with it. `HOME` also means the agent reads its own
configuration - its instructions, its permissions, its memory - so two machines
can answer the same prompt differently. That is a property of driving a tool
that belongs to somebody rather than an API, and it is another reason §8 matters. **No credential variable is passed** - not
`ANTHROPIC_API_KEY`, not anything else. The agent authenticates as itself, and
sic never holds the secret it uses; "no implicit credential access" means the
same thing here that it means everywhere else in this project. An agent that is
not logged in fails to start, and that is the correct failure.

---

## 4. Knowing when the answer is finished

A pane is whatever was on screen when it was looked at, so waiting for it to
stop changing is a guess. The prompt therefore carries instructions to mark the
answer:

```text
<<<SIC-BEGIN-9f2c1a4b>>>
{"cause": "disk full", "confidence": 0.9}
<<<SIC-END-9f2c1a4b>>>
```

The id is fresh per call, so a marker from an earlier answer still on screen
cannot end this one.

### The instructions never contain the marker

Whatever is typed into the pane is echoed back into it, so an instruction
containing the literal marker would put a complete-looking answer on screen
before the agent had answered anything. The instructions therefore spell the
marker in two pieces to be joined:

```text
A marker is `<<<S` and `IC-BEGIN-9f2c1a4b>>>` joined with nothing between them.
```

That is the one piece of indirection in this design and it earns its keep: it
means "the marker appeared on screen" and "the agent printed the marker" are the
same statement, and the rule that extracts the answer needs no knowledge of what
the TUI did with the echo. A test asserts that the instruction text does not
contain the marker it describes, because that property is the whole protocol.

### Extraction

The answer is the lines strictly between the last begin marker line and the
first end marker line after it.

What is searched for is less than the whole marker: `SIC-BEGIN-<id>`, without
the brackets. The split in the instructions falls between the `S` and the `IC`,
so a screen holding that as one piece holds something an agent joined, and the
echo still cannot. Looking for less costs nothing and buys the case that turned
up the first time an agent used a tool and then answered: it printed
`<<SIC-BEGIN-...>>`, having lost an angle bracket in the joining. The answer was
right, and the run waited half an hour for a marker three characters away from
the one it wanted. Each line then loses trailing spaces (`capture-pane` pads to the pane
width) and any leading run of the characters a TUI draws with -
`⏺ ⎿ │ ╭ ╰ ╮ ╯ ─ ▌ · •`.
Lines that are nothing but those characters are dropped: they are the input box,
not the answer.

Captured text is capped at the same 1 MiB as `process.capture`, for the same
reason.

### Deadlines

| | |
|---|---|
| the agent becomes ready | 60 s, a constant |
| the whole answer | `deadline` on the agent declaration, or 30 min |

Passing the deadline fails the call, which is what `retry` counts. A hung agent
must not become a hung run with no explanation.

The second one was a constant too, for the reason the first still is: `retry`
and `timeout` attach to a capability call, and an agent call is a function call,
so the language had nowhere to write one. `docs/design/authority.md` §8 gave it
somewhere - the declaration where `budget` already lives - and 30 minutes is now
what a program gets for not asking rather than what every program gets.

---

## 5. An answer has to be told what shape it must have

The first run that reached a real agent came back as three paragraphs of prose
about the repository, and `from_json` refused it - correctly, and for the wrong
reason. Nothing had told the agent what an answer looked like.

An `agent` declaration is the only place the shape is written down:

```sic
agent triage {
    input: String,
    output: Ticket,     // <- here, and nowhere else
    budget: 1,
}
```

So the shape travels with the prompt. `llm.invoke` takes a second argument for
it, optional exactly as `process.exec`'s argument vector is:

```text
llm.invoke(prompt: String, shape: String) -> String
```

An `agent` fills it in; a direct call that wants prose leaves it off. The
compiler renders it from the declared type:

```text
the deploy job has been queued for an hour

Reply with JSON of this shape, and nothing else:
{"title": string, "severity": integer}
```

### Why an argument rather than a longer prompt

The compiler cannot build the text, because the prompt is a runtime value and
the language has no string concatenation. Adding one for this would be adding an
instruction to the VM to make a capability more convenient.

Passing it separately is also better than the string would have been: the
journal digests the two arguments as they are, so two runs that asked for
different shapes do not look alike afterwards, and the broker composes the
question - which means **a person answering a deferred call is told exactly what
a model would have been told**. They are answering the same question, and before
this they were not.

### The sketch, not a schema

`{"title": string, "severity": integer}`, with `[...]` for a list and the
record's own name where a type contains a list of itself. Not JSON Schema: that
document would be larger than the prompt it decorates, and the thing reading it
is not a validator. The validator is `FROM_JSON`, which already exists, runs in
the VM, and does not have to trust anything the agent was told.

---

## 6. Reading a TUI is version-dependent, so the version is recorded

Everything in §4 is a bet on what a particular version of a particular agent CLI
prints. A recorded run therefore keeps what answered it, beside the responses
that are already there:

```json
{"driver":"tmux:claude","command":"/home/x/.local/bin/claude",
 "agent":"claude 1.2.3","multiplexer":"tmux 3.4",
 "instructions":[{"path":"./CLAUDE.md","sha256":"..."},
                 {"path":"./AGENTS.md","absent":true}]}
```

`sic explain` prints it. Without it, a recorded run's answers came from
"claude", which is not a fact anyone can check later.

### And what it was told

The same sentence applies word for word to the instruction files. The pane
starts in the directory `sic` was started in (§3) and the agent reads `HOME`,
so a file in the repository saying how this project works is one of the three
things that decided the answer - beside the prompt, which the journal digests,
and the output type, which the program declares. It was the one with no trace at
all, and the run that made this obvious answered in Japanese because of a file
under `HOME` that no part of the record mentioned.

So they are digested, not stored. They are source in a repository and
recoverable from it; what the record needs is enough to say whether they were
the same ones. A file that was **not** there is recorded as not there, because
a list with nothing in it cannot tell "looked and found nothing" from "did not
look".

**The list is what sic looks at, not what the agent reads**, and saying so is
part of recording it honestly. An agent may read nested files further down a
tree, files whose names are not on this list, and configuration in formats
nobody here has heard of. A short list that is true supports the question a
person six weeks later actually has - were these the same? - and a longer one
that implied completeness would not.

The user-level files are on it for the reason that argues hardest for them:
they are outside the repository, so a digest is the only evidence of them that
will ever exist.

It is not a journal event. The journal has a fixed vocabulary of events about
what the *program* did, and records digests rather than values; "which build of
which tool was on this machine" is neither. `docs/design/runs.md` already draws
that line for what the broker answered.

---

## 7. `sic replay` never reaches the agent

Replay re-runs a recorded run against its recorded answers. If it can reach a
live session it is not a replay, so `--llm` is not accepted there at all.

---

## 8. The manifest reaches the agent

```sic
allow {
    llm.invoke "claude";
}
```

This section used to say that the manifest under-reported, and `sic plan`
printed a warning to that effect: the program may do one thing, and the agent
behind that one grant may edit any file, run any command and reach the network.

That is no longer true, and the warning is gone. `docs/design/authority.md` is
the design that removed it - the agent's authority is the program's manifest,
translated into the agent's own permissions where those can hold a constraint,
routed back through the broker where they cannot, and everything else refused by
a hook that fails closed. So the plan reports it as a view of the manifest that
is already there:

```text
Capabilities:
  llm.invoke      [invoke]  "claude"  (not pinned)
    the agent's Read   "./docs"                 (its own permissions)
    the agent may use  "/usr/bin/cargo"         (through the broker)
    the agent may not  reach the network        (no tool it has can)
    the agent may not  run a shell of its own   (refused by the hook)
    the agent may not  use any other tool       (refused by the hook)
```

The `cargo` line is there because that grant says `delegable`. Without the word
it reads `the agent may not use "/usr/bin/cargo"`, and the reason is
`authority.md` §4a: for the `process` family the constraint does not bound the
authority, so the manifest has to say whether the agent gets it. "of its own" on
the shell line is the same change - a delegated shell is one the agent reaches
through the broker rather than one it runs itself.

Every line names where it is enforced, because a gate and a boundary are
different things - and `authority.md` §6 states plainly which of the two this
is.

---

## 9. Memory: one conversation for as long as a task

One-shot is the default, which means it is not written at all. The only thing a
program spells is the case that keeps something:

```sic
agent triage {          // one-shot: a fresh conversation every call
    input: String,
    output: Ticket,
    budget: 1,
}

agent refactorer {      // one conversation, for as long as the task
    input: String,
    output: Patch,
    budget: 20,
    memory: task,
}
```

There is deliberately no `memory: call` to write. A value whose only use is to
say "the default" is vocabulary that earns nothing, and the absence of the field
already reads as what it means. `task` is the only scope for the same reason at
the other end: a conversation lasting a whole run is what a program that never
spawns already gets, and one lasting a call is what not writing this means.

| | one-shot (the default) | `memory: task` |
|---|---|---|
| pane | one per call, closed after | one per task, kept |
| conversation | fresh every time | continues |
| what `retry` means | ask again | ask again *in a conversation that remembers the first answer* |
| the journal | holds everything that shaped the answer | holds the prompts and the answers; the accumulated context lives in the agent |

The last row is why the choice belongs in the declaration, where whoever reads
the program will see it, and why `sic plan` prints it:

```text
1. INVOKE   llm.invoke      "claude"  in one conversation per task  at most 4 in a run
```

Two calls that continue one conversation are not two independent calls, and a
plan that did not say so would describe a program that does not exist.

### It travels in the policy table, not in the manifest

`budget` is attached to a call site in the policy table and reaches the VM from
there; this takes the same path, for the same reason. The manifest was the
alternative and it is the wrong granularity: a grant names the model, and two
agents may share one grant while only one of them remembers.

What travels is a **number**, not a flag. A conversation is identified by the
pair `(conversation, task)`: the number says which caller keeps it, the task
says which of that caller's conversations this is. A flag would leave two agents
that both remember talking into the same pane, and one agent running in two
tasks doing the same.

### The run's session

Every pane a run opens lives in one tmux session named after the run's id. A
one-shot call gets a window that is killed when the call returns; a
`memory: task` call gets a window named for its conversation and task, and keeps
it.

Naming the session after the run is what makes it findable again without
anything being written down: a run continued in another process derives the same
name and its panes are there. The run's own directory does keep a list of which
conversations were opened, and that file exists for one purpose - telling a pane
that was closed apart from one that was never made. Without it, a resumed run
that reached a remembering agent for the first time would be indistinguishable
from one whose conversation was lost.

### When it is over

| | |
|---|---|
| the run finished, or failed | the session is killed |
| the run stopped to wait | the session is kept |

A run that stopped to be continued will come back, and it should not come back
to a stranger. A run that is over keeps nothing: the journal already holds the
prompts and the answers, and what a pane has beyond that is context nobody can
ask for any more.

A pane also outlives the task that opened it, until the run ends. A task
finishing is not something the broker is told about - the VM suspends at an
effect and nothing announces the rest - and inventing a channel to say so would
be paying for tidiness with a hole in the boundary.

### Coming back to it

```console
$ sic attach <RUN-ID> --value V --llm tmux:claude
```

`attach` knows which run it is answering, so it derives the session name and
finds the panes. `resume` does not: a checkpoint is a run's state and does not
say which run it came from. Rather than opening a fresh conversation and
continuing as though it were the old one, `resume` refuses a driver for a
program that keeps a conversation, and says which command does it instead.

A pane that should be there and is not - the machine restarted, or somebody
closed it - fails the call:

```text
error: the conversation this run was holding for task 0 is gone: its pane was
       closed, or the machine it was on restarted. It cannot be continued
```

Failing loudly is the whole point. Silently starting a fresh conversation would
change what the run means without saying so, and the record would show a call
that looked exactly like the one before it.

---

## 10. What the interface does to a long answer

A terminal user interface draws an answer at the width it has, and a line too
long for that comes back with a break in the middle of whatever it was drawing.
This is not the terminal wrapping - `capture-pane -J` puts that back together -
but the agent's own renderer, which emits the break itself. Nothing on the
screen says which breaks are the answer's and which are the interface's.

For JSON there is no need to tell them apart. The grammar requires whitespace
nowhere, and a newline inside a string is not legal, so **joining every line
with nothing between them** repairs a wrap exactly and leaves a document that
was already whole unchanged. The instructions also ask for the JSON on one line,
so that there is less to break in the first place.

Prose has no such property. An answer meant to be read by a person comes back
with the interface's line breaks in it, and that is stated rather than papered
over: `llm.invoke` without a shape is the one call this cannot serve faithfully.
It is also the reason the broker tells the driver whether a shape was asked for
(§5) rather than the driver guessing.

---

## 11. Not here

- **The run's session as the person's session.** `sic attach <RUN-ID>` still
  answers a waiting run rather than attaching to its panes, `spawn` is not a
  layout with a pane per task, and `human.approve` is not asked in the pane. The
  session outliving the process is what those need and is now there; they are a
  change to the CLI and to the scheduler rather than to this mechanism.
- **Resuming a conversation from a loose checkpoint.** `claude --resume <id>`
  exists, but the id is written where the agent keeps its own state, and reading
  that is a coupling to one agent's internals rather than to its interface. The
  pane is the conversation here, and tmux is what keeps it.
- **Making the grant reach the agent** (§8): permission translation, capabilities
  offered back to the agent through the broker, a hook that puts tool uses in the
  journal, and a `budget` that counts something an agent with tools can exceed.
- **A general terminal capability** - `term.open`, `term.send`, `term.read`.
  That is the deputy of §1 with a nicer name.
- **A pty of our own.** tmux has one.
- **Any multiplexer but tmux**, and any agent that is not a coding agent CLI.
- **Streaming a partial answer into a running program.** A value arrives when it
  is complete, as everywhere else.
- **Agent-to-agent protocols.** In this model the orchestrator holds the values,
  so passing one agent's output to another is a variable, and where it came from
  is already in its type.
- **A duration literal.** `deadline: 1800000` is milliseconds because `timeout`
  is, and one unit for every duration in the language is worth more than a
  readable number - two units in one file is a bug nobody sees. `30m` is what
  would fix the reading, and it belongs to the lexer and to `timeout` as much as
  to this.

---

## 12. Units of work

| # | Unit | Done when |
|---|------|-----------|
| 1 | The driver interface, and the broker asking it | a broker with no driver still defers |
| 2 | The marker protocol and extraction | the instructions do not contain the marker they describe |
| 3 | The tmux driver | a call opens a pane, asks, reads the answer back, and closes it |
| 4 | `--llm` on `run`, refused on `replay` | an unknown spec is a usage error |
| 5 | The shape of the answer, carried with the prompt | an `agent` call asks for JSON of its `output` type |
| 6 | What answered, recorded and explained | `sic explain` names the version of the agent |
| 7 | The plan's warning | `sic plan` says what a grant of `llm.invoke` does not cover |
| 8 | `memory: task`, from the declaration to the pane | a second call is answered in a conversation that remembers the first |
| 9 | The run's session, and coming back to it | `sic attach --llm` continues; `resume` says it cannot |
