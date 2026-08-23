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

1. starts a detached tmux session running the agent,
2. pastes the prompt and presses Enter,
3. polls the pane until the answer is complete,
4. kills the session.

A person can watch it happen (`tmux -L sic attach -t <session>`), which is the
seed of the run-level session in §8 rather than a feature of this part.

The pane is closed when the call returns, and the reason is what the journal
holds: for a call with no memory the prompt and the answer are the whole story,
so the pane keeps nothing the record does not. A pane worth keeping alive is one
carrying accumulated context, and that is `memory: task` - §8.

### The tmux server is sic's, not the person's

`-L sic` puts the driver on its own socket and `-f /dev/null` ignores the
person's `tmux.conf`. Two reasons:

- **The environment.** `new-session` in an existing server runs in *that
  server's* environment, not in the one `sic` was started with. A driver that
  cannot say what environment the agent got cannot say what the agent could
  reach.
- **The configuration.** A status line, a different default shell, or
  `set -g mouse` change what `capture-pane` returns. Reading a TUI is fragile
  enough without also reading somebody's dotfiles.

### What the agent inherits

The environment is cleared down to a named list: `HOME`, `PATH`, `TERM`,
`LANG`, `LC_ALL`, `USER`, `SHELL`, `TMPDIR`.

`HOME` is there because that is where the agent's own login lives, and `PATH`
because the agent runs tools with it. **No credential variable is passed** - not
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

The answer is the text between the last begin marker and the first end marker
after it. Each line then loses trailing spaces (`capture-pane` pads to the pane
width) and any leading run of the characters a TUI draws with - `⏺ ⎿ │ ╭ ╰ ─ ▌`.
Lines that are nothing but those characters are dropped: they are the input box,
not the answer.

Captured text is capped at the same 1 MiB as `process.capture`, for the same
reason.

### Deadlines

Two, both constants, because the language has nowhere to write one: `retry` and
`timeout` attach to a capability call, and an agent call is a function call.

| | |
|---|---|
| the agent becomes ready | 60 s |
| the whole answer | 30 min |

Passing the deadline fails the call, which is what `retry` counts. A hung agent
must not become a hung run with no explanation.

---

## 5. Reading a TUI is version-dependent, so the version is recorded

Everything in §4 is a bet on what a particular version of a particular agent CLI
prints. A recorded run therefore keeps what answered it, beside the responses
that are already there:

```json
{"driver":"tmux:claude","command":"/home/x/.local/bin/claude",
 "agent":"claude 1.2.3","multiplexer":"tmux 3.4"}
```

`sic explain` prints it. Without it, a recorded run's answers came from
"claude", which is not a fact anyone can check later.

It is not a journal event. The journal has a fixed vocabulary of events about
what the *program* did, and records digests rather than values; "which build of
which tool was on this machine" is neither. `docs/design/runs.md` already draws
that line for what the broker answered.

---

## 6. `sic replay` never reaches the agent

Replay re-runs a recorded run against its recorded answers. If it can reach a
live session it is not a replay, so `--llm` is not accepted there at all.

---

## 7. The manifest under-reports, and the plan says so

```sic
allow {
    llm.invoke "claude";
}
```

The program may do one thing. The agent behind that one grant may edit any file,
run any command and reach the network - and `sic plan` exists to say what may
happen. So it says what it does not know:

```text
Capabilities:
  llm.invoke      [invoke]  "claude"  (not pinned)
    warning: this grant says what the program may ask for, not what the
             agent may do while answering
```

This is not a placeholder for work that finishes the sentence; it is the true
statement available today. Making the grant reach the agent - translating it
into the agent's own permission configuration, routing what cannot be translated
back through the broker, and putting the agent's tool uses in the journal - is
its own design, tracked separately. Until that exists, a plan that printed one
confident line would be the manifest lying about the most important thing in it.

The warning is printed whether or not a driver is chosen, because it was already
true: an answer pasted in by a person also came from outside the manifest.

---

## 8. Part two: memory, sessions, resume

Split off so that this part is one mechanism rather than four.

### A session is sic's session with a person

**One session per run, and the session is where the human is** - not a
conversation with a model. `sic attach <RUN-ID>` already means "come to this
run"; it grows into attaching to that run's session, `spawn` becomes a layout
with a pane per task, and `human.approve` can be asked in the pane instead of
suspending the run. The deferring path stays, because a headless run still
needs it.

### Two kinds of agent call, and the declaration says which

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

There is deliberately no `memory: call` to write: a value whose only use is to
say "the default" is vocabulary that earns nothing.

| | one-shot (the default) | `memory: task` |
|---|---|---|
| pane | one per call, closed after | one per task, alive as long as the task |
| conversation | fresh every time | continues |
| what `retry` means | ask again | ask again *in a conversation that remembers the first answer* |
| resume needs | nothing | the conversation id, per task |
| the journal | holds everything that shaped the answer | holds the prompts and the answers; the accumulated context lives in the agent |

Scoping memory to the **task** rather than the run means a program that never
spawns gets one conversation for the whole run without that having to be
declared. The last row is why the choice belongs in the declaration, where
whoever reads the program will see it.

### Resume should resume the conversation too

A run that comes back in another process should not come back to a stranger.
`claude --resume <id>` exists, so a checkpoint and a conversation id line up -
and that raises where the id lives (the run store, most likely, because the
broker has been stateless between calls until now), what happens when the
session is gone (fail loudly; silently starting fresh changes what the run means
without saying so), and whether a conversation id belongs in a journal that
records digests.

---

## 9. Not here

- **Everything in §8**, which is the second half of this work.
- **Making the grant reach the agent** (§7): permission translation, capabilities
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
- **A deadline in the source** (§4). It needs somewhere to write one, and the
  agent declaration is where it would go.

---

## 10. Units of work

| # | Unit | Done when |
|---|------|-----------|
| 1 | The driver interface, and the broker asking it | a broker with no driver still defers |
| 2 | The marker protocol and extraction | the instructions do not contain the marker they describe |
| 3 | The tmux driver | a call opens a pane, asks, reads the answer back, and closes it |
| 4 | `--llm` on `run`, refused on `replay` | an unknown spec is a usage error |
| 5 | What answered, recorded and explained | `sic explain` names the version of the agent |
| 6 | The plan's warning | `sic plan` says what a grant of `llm.invoke` does not cover |
| 7 | §8 | a second commit |
