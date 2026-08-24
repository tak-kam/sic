# Who decides what the agent may do

`docs/design/driving.md` §8 states the problem and refuses to paper over it:

```sic
allow {
    llm.invoke "claude";
}
```

The program may do one thing. The agent behind that one grant may edit any file,
run any command and reach the network, and `sic plan` prints a warning saying
so. This is the design that removes the warning by making it untrue.

It is the largest claim in the project standing on the smallest amount of
enforcement. Everything else here is arranged so that what a program may do is
readable before it runs - a manifest complete by construction, a verifier that
does not trust its input, a broker that re-checks what the compiler already
checked. Then a capability call starts a program that holds its own credentials
and does what it likes.

---

## 1. The rule

**The agent's authority is the program's manifest. Nothing more.**

One sentence, and everything below is how it is enforced and what it costs.

The alternative was a second manifest - grants written about the agent, nested
inside the grant that summons it. It reads well and it is wrong for now, because
it answers a question nobody has asked yet at the price of two kinds of
authority in one block. The one-manifest rule has the property that matters
more: it can be checked by reading the `allow` block that is already there, and
`sic plan` can print the agent's authority as a *view* of the manifest rather
than as a second thing to keep in step.

What it costs is real and is stated here rather than discovered later:

| the program is granted | so the agent may | and if that is wrong |
|---|---|---|
| `fs.read "./docs"` | read `./docs` | the program is granted more than it needs, or the agent less |
| nothing about the network | reach no network | the agent cannot fetch a dependency, and for some workflows that is the point of running one |

The second row is §6, and it is the row this whole document turns on.

### Why not least privilege per side

Because the agent is a deputy. It is answering a question the program asked, on
the program's behalf, inside the program's run; a deputy that can reach further
than whoever sent it is exactly the confused deputy that capability systems
exist to prevent. Giving the agent authority the program does not have would
mean the manifest no longer bounds the run, which is the property the whole
project is arranged to have.

Where a program genuinely needs an agent to do something the program itself must
not - and there will be such a case - that is a second design, and §11 says so.

---

## 2. Two layers, and only one of them is a boundary

Every agent runtime that has thought about this converged on two layers, and
Anthropic's own documentation is blunt about what the first one is: the
permission system is **"a permission gate, not a sandbox"**. A gate reduces
prompting. It does not contain an agent that has been talked into something by
the contents of a file it read.

| layer | what it is | what it enforces |
|---|---|---|
| the agent's permission configuration, and a hook that can deny | a gate | what a cooperating agent does |
| an OS sandbox, with egress through a proxy that allowlists | a boundary | what any process in it can reach |

sic has to build both halves, and - this is the part that matters for what
`sic plan` prints - **it must never print the first as though it were the
second**. A plan that says "the agent may not reach the network" because a
setting says so, when a subprocess could open a socket anyway, is the manifest
lying in the one place this project cannot afford it.

So: the gate is what routes and records, and the sandbox is what bounds. Where
the sandbox is not there, the plan says the gate is what it has.

### What the gate cannot hold, in this agent, today

Two of these were found by reading the agent's documentation rather than by
reasoning about gates in general, and both change what a plan is allowed to say.

**Some tools run whatever the rules say.** A fixed set of read-only shell
commands is always allowed and is not configurable. `dontAsk` - the mode that makes an allowlist mean what it says,
because it denies an unnamed tool without prompting, and the only mode a pane
with nobody watching it can run in - still permits `ls`, `cat`, `echo`, `pwd`,
`head`, `tail`, `grep`, `find`, `wc`, `which`, `diff`, `stat`, `du`, `cd` and
the read-only forms of `git`.

`cat` reads any file on the machine, so a `Read(./docs)` rule bounds the *Read
tool* and does not by itself bound *reading*. What closes it is the hook (§7):
it runs before any rule is consulted and can refuse what the rules would have
allowed. Measured rather than assumed - a hook that exits 2 blocks
`cat /etc/hostname` under `dontAsk`, and its message reaches the agent as the
reason. So the shell is refused outright, because no grant names one:
`process.exec` grants a binary at an absolute path, sometimes pinned by digest,
and the agent reaches that through the broker where it is checked.

It is not only the shell. A run showed the agent using `ToolSearch`, which the
allowlist had never named and which ran anyway - so "the rules are an allowlist"
is not true of the tool surface. The hook therefore decides the surface by name
and the rules are left doing what they are good at, which is holding a path
scope. A list of bad tools would have missed `ToolSearch` and would miss the
next one.

**Allow rules merge across settings scopes.** A rule in the machine's
`~/.claude/settings.json` or in the project's `.claude/settings.json` is added
to the ones sic passes, so the manifest is not by itself the whole story. Deny
rules are the exception - they apply across every scope and no allow rule
anywhere overrides them - which is why the network denial (§6) rests on a deny
and not on the absence of an allow.

The flag that would close this is `--setting-sources`, which takes some of
`user`, `project`, `local`; whether it accepts an empty list is not something
this design has verified, and guessing would be the silent widening this
document exists to prevent. Until it is verified, a plan describing path scopes
is describing what sic asked for rather than what is in force.

---

## 3. Translate what can be translated

The `allow` block is already a policy. It is compiled into the agent's own
permission configuration before the session starts: allowed tools, denied tools,
and their path scopes.

The dividing line is not taste. A grant carries a constraint, and the question
is whether **the agent's permission system can enforce that constraint**:

| grant | can it? | so |
|---|---|---|
| `fs.read "./docs"` | yes - path-scoped read | translate |
| `fs.write "./src"` | yes - path-scoped edit | translate |
| `process.exec "/usr/bin/cargo"` | partly - a command allowlist, no digest | route |
| `process.exec ... sha256 "..."` | no - a digest pin has no equivalent | route |
| `human.approve`, `human.choose` | no equivalent tool | route |
| a tool the manifest does not cover | - | deny |

"Sensitive" then falls out of the grant rather than being decided per program,
which is what stops this from becoming a list somebody maintains.

Two things a translation must never do: widen a grant to fit the configuration's
vocabulary, and silently drop one it cannot express. Both turn into §5.

---

## 4. Route what cannot be translated

For a grant that cannot be translated, the agent's native tool is **denied
outright** and the capability is offered to it instead as a tool that calls back
into the broker. The effect is then performed by the same code that performs it
for a sic program, checked against the same constraint, and it lands in the
journal like any other capability call.

That is the whole argument for routing rather than translating: a routed effect
is not "the same effect with a similar policy", it is *the same call*.
`process.exec "/usr/bin/cargo" sha256 "..."` hashes the file on every call
(`docs/design/capabilities.md` §7), and no permission setting will do that.

MCP is the transport. sic writes its own server, because `[dependencies]` stays
empty; the 2026-07-28 revision of the protocol went stateless and dropped
Sampling, Roots and Logging, which makes a hand-written server considerably
smaller than it would have been - and also means the target moves, which is an
argument for keeping the surface sic implements to the smallest set that carries
a capability call: `tools/list`, `tools/call`, and whichever handshake arrives.

### Whichever handshake arrives, and whichever revision

The stateless revision opens with `server/discover`; the one before it opens
with `initialize`. Answering both costs a few lines. Announcing a revision
rather than agreeing on one costs a whole session: the first attempt replied
`2026-07-28` to a client that speaks the older era, the client refused the
connection, and what a person saw was a pane where the tool simply was not
there. The server now echoes the revision the client asked for, which is both
true - three methods, and the shape of a tool has not changed between them - and
the only answer that connects.

### The socket, and where it is served

The server the agent starts performs nothing. It forwards each call to a unix
socket the run is listening on, and that socket is served **from the loop that
is already watching the pane** - no thread, no lock. The agent can only be
making a call while it is answering, and that is exactly when sic is waiting for
it, so the moment a tool use can happen is the moment something is watching for
one.

A connection carries one question and one answer. The socket is removed when the
run ends: one outliving its run would be a door into a manifest nobody is
enforcing any more.

---

## 5. A grant that can be neither stops the run before it starts

Not at the first call. Before anything runs.

A manifest that cannot be enforced is worse than no manifest, because
`sic plan` printed it. The failure has to name the grant and say which half was
missing - no translation and no route - so that the answer is either a driver
that can do it or a program that does not ask for it.

This is the same shape as `sic run --llm` refusing a driver it cannot open
(`driving.md` §2): everything checkable before the run is checked before the
run.

---

## 6. The network

sic has no network capability. A program cannot open a socket, `sic upgrade`
shells out to `curl` at an absolute path and spends a paragraph justifying it,
and "no implicit network access" is the first of the security principles. Then
`--llm tmux:claude` starts an agent with `HOME` and `PATH` and it can reach
anything on the internet.

Under §1 the answer is already decided and needs no new syntax: **the program is
granted no network, so the agent gets none.** Egress is denied by default
because nothing in this project is permitted that was not named, and there is
nothing to name.

### What it actually rests on, which is not what this section first said

This section was written expecting an OS sandbox to be the enforcement, with the
gate unable to help. Building the rest changed the answer, and the correction is
worth more than the prediction was.

The hook (§7) refuses **every tool the manifest does not account for**, by name,
before any rule is consulted. So the agent's whole tool surface is the manifest's
- and nothing on that surface reaches the network:

| | |
|---|---|
| `WebFetch`, `WebSearch` | not named by any grant, so refused; also denied by rule, since a deny rule holds across every settings scope |
| `Bash` | refused, so there is no subprocess to reach anything |
| `Read`, `Write`, `Edit` | do not network |
| `mcp__sic__*` | performed by the broker, under the manifest |

And the sandbox that was going to enforce this covers **Bash subprocesses**:
`Read`, `Write`, `WebFetch` and `WebSearch` do not go through it. So for this
agent it isolates exactly the surface the hook already refuses, and would add
nothing to egress. Its own documentation is the source for both halves of that.

Deny by name is a stronger rule than the deny list it replaced, and it was
reached by measurement rather than by reasoning: a run showed the agent using
`ToolSearch`, a tool the allowlist had never named and which ran anyway. A rule
listing bad tools would have missed it, and would miss the next one.

### What a sandbox is still for

Not egress. Filesystem isolation - a bound on what a tool sic *does* allow can
be talked into touching - and containment if the hook is bypassed rather than
consulted. §11 keeps both as not here, and the plan says the gate is what it
has, which after this is more than it was.

### What this costs, plainly

A coding agent that cannot reach the network cannot install a dependency, fetch
a crate's documentation, or read the issue it was asked about. For some
workflows that is the point of running one. This design says no to that today
and says why: the alternative is a grant that names domains, and **a grant about
the agent rather than about the program is a change to what `allow` means**.
That change deserves its own argument, not a line slipped into this document.
When somebody needs it, the question to answer first is whether sic gets a
network capability at all - and then the agent gets it the same way it gets
everything else, by the program being granted it.

### The limit, stated

The hook is consulted by the agent. It is a gate the agent walks through, not a
wall around it: it holds because the agent asks, and an agent that did not ask -
a different build, a bypass, a bug - is not stopped by it. That is the same
sentence the permission system's own documentation uses about itself, and it
stays true of this. What it buys over that documentation's version is that the
gate is now the manifest rather than a list somebody maintains, and that
everything it sees reaches the journal.

A boundary is the sandbox, and the sandbox is not here.

---

## 7. The tool uses have to reach the journal

Translation alone leaves the journal with a hole exactly where the interesting
part is: `INVOKE llm.invoke -> "..."`, and nothing about the twelve files the
agent edited to produce it. The run's account of itself would describe the
conversation and omit the work.

### The hook is binding

`PreToolUse` runs before a tool call executes and can block it, returning
`permissionDecision: "allow"` or `"deny"`; exiting 0 with no decision falls
through to the normal flow. So the version that is worth having is the version
that exists, and the advisory fallback does not need designing.

Three consequences, and they are the reason this is not a footnote:

**The broker becomes an authorization server, synchronously, per tool use.**
Today it answers the VM and returns. Here it answers a process it started, on a
hot path, where a slow answer stalls the agent and a crashed one either fails
open or hangs. **It fails closed**, and exit 2 is how: a hook that returns a
`deny` decision does not override an allow rule, while exiting 2 blocks before
any rule is consulted. So both a refusal and an unreachable run block, and what
tells them apart is the message - which reaches the agent, and a person reading
a denial has to be able to tell them apart or they will debug the wrong thing.

    sic refused it: no grant names a shell...
    sic could not be reached, so nothing authorized this (...)

**The payload is a journal entry, and the journal has a fixed vocabulary.**
`tool_name` plus `tool_input` is what an audit needs, and `docs/design/v0.1.md`
§10 has no event for it. Whatever event is added follows the existing rule -
digests, never values - and for an `Edit` the input is a path and a diff. The
path is the interesting half and it is also the half that is not secret. That
split is decided when the event is designed, not after.

**More than `PreToolUse` exists.** A budget that counts tool uses wants
`PostToolUse` as much as `PreToolUse`, and `SubagentStart` is the event that
says the count is about to stop being one loop. Which of these sic subscribes to
is decided once, here, rather than discovered one at a time.

---

## 8. Budget: three numbers, because there are three enforcement points

`budget: N` counts capability calls, is enforced by the VM against a pc in the
policy table, and travels in checkpoints so that resuming does not hand a run a
fresh allowance. Compared with the field that is the good half: it is *hard*.
`task_budget` in Anthropic's API is advisory - the model sees a countdown and
paces itself - and the client-library limits elsewhere are enforced by the
client. sic enforces its budget inside the machine that is executing, which
nobody outside the runtime is in a position to do.

The unit stops meaning anything the day an agent has tools: the driver counts 1
where the machine did 200 tool uses. And across every framework surveyed, the
budget that actually stops a runaway is not the token budget - it is a step
count or a wall clock. Tokens are the unit people care about because tokens are
what they are billed for, and also the unit that only ever arrives as a hint.

A single field whose enforcement point depends on its unit cannot be explained
in one sentence, so there are three fields and each has one:

```sic
agent refactorer {
    input: String,
    output: Patch,
    budget: 20,             // model calls  - the VM, against a pc
    tools: 200,             // tool uses    - the broker, through the hook
    deadline: 1800000,      // wall clock   - the broker, which has the clock
}
```

`deadline` is milliseconds, which reads badly at this magnitude and is still the
right answer: `timeout N` is milliseconds, and one unit for every duration in the
language is worth more than a readable number. Two units in one file is a bug
nobody sees. A duration literal - `30m` - is what would fix the reading, and it
belongs to the lexer and to `timeout` as much as to this, so it is not smuggled
in here.

`deadline` also replaces two numbers nobody declared. `driving.md` §4 hard-codes
60 seconds for the agent to become ready and 30 minutes for a whole answer,
`sic plan` does not print them, and §11 there lists "a deadline in the source"
as not-here with the note that it needs somewhere to write one. This is that
somewhere, and it is the declaration the budget is already written in.

### What survives a checkpoint

| | travels | why |
|---|---|---|
| `budget` | yes | already does; otherwise resuming hands the run a fresh allowance |
| `tools` | yes | same argument, same reason |
| `deadline` | **no** | it bounds one answer, not the run |
| the site it belongs to | yes | a count charged to the wrong site is not a bound |

The last row is the one with a real answer rather than a preference. A deadline
that travelled would mean a run that waited two days for a person had spent its
whole deadline waiting, which is wrong; one that reset on every resume would
never bound anything, which is also wrong. Both readings dissolve once the
deadline is understood as what `driving.md` §4 already used it for: how long the
agent may take to produce **this answer**. It starts when the prompt is sent and
ends when the answer arrives, and a run that is suspended is not producing one.

---

## 9. Skills

A skill is closer to "what the agent knows how to do" than to authority, but a
skill can carry scripts, so its authority question reduces to the tools those
use - which is §3 and §4. What is left is identity, and this project already has
an answer for identity:

```sic
allow {
    llm.invoke "claude" skill sha256 "...";
}
```

What runs is decided by what the file **is**, the same way `process.exec` pins a
binary. Which artifact the digest covers - one `SKILL.md`, a directory, a
manifest of them - is the part still to design, and it is the part where a
directory digest has to be defined rather than assumed.

---

## 10. What `sic plan` has to say afterwards

The test for whether any of this worked is the plan. It has to print something
like:

```text
Capabilities:
  llm.invoke      [invoke]  "claude"  (not pinned)
    the agent's Read   "./docs"                 (its own permissions)
    the agent's Write  "./out.txt"              (its own permissions)
    the agent's Edit   "./out.txt"              (its own permissions)
    the agent may use  "/usr/bin/cargo"         (through the broker)
    the agent may not  reach the network        (no tool it has can)
    the agent may not  run a shell              (refused by the hook)
    the agent may not  use any other tool       (refused by the hook)
```

The three bounds of §8 are not repeated here: they belong to a call site rather
than to a grant, and the plan prints them on the line of the call they bound.

Every line names **where** it is enforced, because §2 is the whole point: a
reader has to be able to tell a gate from a boundary. A line with nothing in
parentheses would be a claim with no mechanism behind it.

The first two lines say "the agent's Read" rather than "the agent may read" for
the reason in §2: those rules bound a tool. What stops the agent reaching a file
those rules do not name is the line below them, because the shell is where that
would otherwise happen.

And the warning in `driving.md` §8 is removed only when the plan can print this
for the manifest in front of it. Until then the warning is the true statement,
and a confident line would be the manifest lying about the most important thing
in it.

---

## 11. Not here

- **A second manifest for the agent** (§1). Grants written about the agent
  rather than about the program - `llm.invoke "claude" { net "docs.rs"; }` - are
  the answer to "the agent needs something the program must not have", and that
  question has not been asked by a real program yet. When it is, the argument
  starts from what `allow` means, not from the syntax.
- **A network capability** (§6). Whether sic gets one at all is a bigger
  decision than this document, and this document is arranged so that it does not
  have to be taken now: no grant, no egress.
- **Sandboxing beyond egress.** Filesystem and process isolation at the OS
  level, seccomp, containers. §2 names the layer; bounding what the agent may
  reach on the filesystem is done by the gate today, and saying that plainly is
  better than a half-built boundary that the plan would then have to describe.
- **More than one agent per pane**, and MCP servers other than sic's own
  capabilities.
- **A program written by a model.** Worth recording precisely because it is not
  obviously wrong: an agent could emit a `.sic` program rather than a JSON
  value, and sic is the only project in this space that already has the language,
  the type checker, a verifier that does not trust its input, a manifest complete
  by construction, and a `plan` command to read it with. The manifest question
  even has an answer already - `docs/design/modules.md` gives a library
  `requires` and reserves `allow` for the program that runs, and a model-written
  program is a stranger case of the same shape.

  It stays off the map for now for two reasons that are not about safety. The
  language is too small to write in: no loops, no string concatenation, no
  optional types, so a model asked for a sic program would mostly discover what
  it cannot write. And the current design - the orchestrator holds the values,
  provenance is in the type, the program is written by a person - is coherent,
  and `driving.md` §11 rejects agent-to-agent protocols on exactly that basis.
  Moving authorship into the run deserves its own argument before it gets an
  implementation. Recorded, not rejected.

---

## 12. Units of work

Each is a piece somebody could finish on its own. The order is the order in
which each one makes the next honest.

| # | Unit | Done when |
|---|------|-----------|
| 1 | The translation, and the refusal | a grant that can be neither translated nor routed stops the run before it starts |
| 2 | The broker's MCP server, and routing one capability through it | a pinned `process.exec` performed for the agent hashes the file, and lands in the journal |
| 3 | The `PreToolUse` hook, binding, failing closed | a denied tool use says whether the broker refused it or was unreachable |
| 4 | The tool-use event | an audit can see the twelve files the agent edited, as digests and paths |
| 5 | `tools:` and `deadline:` on an agent declaration | `driving.md` §4's two hard-coded numbers are gone |
| 6 | Egress denied | no tool the agent has reaches the network, and the hook refuses every tool the manifest does not account for |
| 7 | The plan | §10, with every line naming where it is enforced |
| 8 | `driving.md` §8's warning removed | done with 7, because 7 is what made it untrue |
