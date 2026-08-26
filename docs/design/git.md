# `git`, and when a program deserves a capability

`process.run "/usr/bin/git" args ["status"]` already works. So this document
has to argue for more than convenience, and the argument is not about git: it
is about which programs deserve a capability of their own. Git is the first one
that passes the test.

---

## 1. The test

**A capability earns its place when the broker can enforce something the
manifest cannot say.**

A `process` grant fixes an absolute path, a leading argv, a working directory
and an environment. For most programs that is everything there is to say, and
adding `cargo.*` or `npm.*` on top would buy spelling and start a catalogue of
the world's command-line tools inside the one crate that touches the world -
each with an output format to track and a maintenance cost to carry.

Git is different, and the difference is that **granting git is closer to
granting arbitrary execution than it looks.**

| what | reaches |
|------|---------|
| `core.pager`, `core.editor`, `diff.external` | a command line, named in a config file |
| `.git/hooks` | executables that arrived with the repository |
| `credential.helper` | a program, named in configuration |
| `protocol.ext` | a remote URL turned into a command |
| aliases | one subcommand standing for another, or for `!sh -c ...` |

A manifest cannot say "git, but with none of that". It can pin the binary and
clear the environment, and neither of those reaches `.git/config`,
`~/.gitconfig` or `/etc/gitconfig`. A broker can, because the broker builds the
command line.

That is `env_clear` one level up. A capability call gets no environment unless
the grant writes it out, because what a child inherits should not depend on
what the shell happened to have. **What git reads should not depend on what the
checkout happened to contain**, for the same reason and with the same fix: the
decision is taken here, once, and it is in the source rather than in the
machine.

---

## 2. What every call is told

```text
-c core.hooksPath=/dev/null    a repository is data; its hooks are not
-c core.pager=cat              a command line in a config file
-c core.editor=false           the same
-c diff.external=              the same
-c protocol.ext.allow=never    a URL that becomes a command
-c credential.helper=          a program, named in configuration
--no-pager                     whatever the config said
```

and an environment of exactly three variables:

```text
GIT_CONFIG_NOSYSTEM=1          not /etc/gitconfig
GIT_CONFIG_GLOBAL=/dev/null    not ~/.gitconfig
GIT_TERMINAL_PROMPT=0          nobody is there to answer git
```

`env_clear` alone would not do the last two. Git finds `/etc/gitconfig` by a
path of its own, and with no `HOME` it falls back to the passwd entry rather
than to nothing - so the guarantee has to be stated in git's own words rather
than by taking things away.

`core.hooksPath` is set although nothing here writes, and that is deliberate: a
list that is right for the *reason* rather than for today's two calls is the
one that stays right when a third is added.

---

## 3. What is in it: two calls

| call | answers | returns |
|------|---------|---------|
| `git.status()` | what is modified, staged or untracked | `Observed<List<String>>`, one entry per path |
| `git.rev_parse(rev)` | what a revision resolves to | `Observed<String>` |

Two rather than the four an inventory would suggest. `git.log` and `git.diff`
are things somebody might want; `status` and `rev_parse` are what this
repository's own `workflows/ci.sic` would ask - is the tree dirty, and what
commit is this. Nothing has asked for the other two, and a capability nothing
calls is surface area with a maintenance cost and no reader.

`--porcelain=v1` is what `status` reads, because git documents it as stable for
scripts. That is what makes parsing it a reasonable thing for a broker to do
and an unreasonable thing for a workflow to do: `sic`'s string handling is thin
on purpose and should stay that way rather than grow to meet `sed`.

`rev_parse` is the only place a `git` call takes a name from the program, so it
is the only place an argument could become an option. A revision that is empty,
holds whitespace, or starts with `-` is refused before git sees it - `-` is how
an argument becomes an option, and an option is how a read becomes something
else.

### The grant

```sic
allow {
    git.status "/usr/bin/git" in "/abs/path/to/repo";
}
```

The constraint names git, at an absolute path, the way every other grant that
starts a program does - and `sha256` pins it, for the same reason. `in` names
the repository, which is the mechanism #51 built for exactly this question:
what directory does this call get.

Nothing new was needed for either. That is the point: a capability that had to
invent its own way of saying "which binary" and "which directory" would be
describing a different kind of thing than the manifest is about.

**`env` is refused** (E0336). A variable there would decide what git reads,
which is the decision this capability exists to take; handing it back would
leave `git.status` as `process.run` with extra steps. `sic plan` says so
positively - *reading no configuration but this repository's* - rather than
printing "with no environment", which would read as something this grant chose
rather than something it cannot change.

---

## 4. What comes back, and what it is allowed to decide

Both return `Observed`, which is what `process.capture` returns and for the
same reason: it is what a program printed.

That matters more than it sounds. A path out of `git.status` or a hash out of
`git.rev_parse` could otherwise flow into the tail of a `process.run` and
decide what a child is told - which is the thing `Observed<T>` exists to stop.
It does not stop the useful cases: `len(git.status(...)) > 0` is "the tree is
dirty", and a comparison is not a decision about what runs.

`Observed<List<String>>` is a new interned type, and it is the second one after
`Observed<String>`. It is not a record and not a list of records, because
`CapValue` is flat on purpose - the same objection is written out twice already
in `sic-core`, once against a general list and once against a general record:
nesting buys a depth limit, a recursive encoder, and a decoder that has to
refuse a hostile depth, and nothing that exists needs any of it.

So this document does not deliver "a record per changed file". It delivers the
line git documents as stable, one per entry, in a list a program can measure
and index. If a caller ever needs the fields apart, that is an argument about
`CapValue`, not about git.

---

## 5. What the agent may do with it

`Routed`, and only when the grant says `delegable` - the same terms as the
`process` family.

Not `Translated`. The safe command line is the broker's: `-c
core.hooksPath=/dev/null` and the six lines beside it are the whole difference
between this and `process.run "/usr/bin/git"`, and none of them survives being
rewritten as a rule about a shell command. Translating would widen the grant to
fit the configuration's vocabulary, which is the one thing a translation must
never do - `docs/design/authority.md` argues it for `process.exec` and the
argument is the same one.

---

## 6. Deliberately not here

- **Writing.** `commit`, `add`, `checkout`, `merge`. Every one runs hooks by
  default, and turning hooks off for a *write* is a different decision from
  turning them off for a read: a repository whose hooks are meant to run is a
  normal repository. That needs its own argument and its own issue.
- **Anything that reaches the network.** `push`, `fetch`, `pull`, `clone`. The
  runtime has no network capability at all, on purpose - `sic upgrade` runs
  curl only when that command is the one that was typed, and a program a user
  runs reaches nothing. `git.push` would be a network capability wearing
  another name, arriving without the argument a network capability is owed.
- **`git.log` and `git.diff`.** §3. When something asks, they are two more arms
  and this document is the precedent.
- **Reimplementing git.** Pack files, the index format and zlib, by hand, with
  no dependencies. Enormous, and not what `sic` is for. The broker runs the git
  that is on the machine, at an absolute path the grant names, like every other
  program.
- **`cargo.*`, `docker.*`, and the rest.** None passes §1. If one does later,
  the argument goes in its own document and this is the precedent to answer.
- **A working directory, or `cd`.** #51 decided the other way: a grant says
  `in "/abs"` so that each call names where it runs, rather than inheriting one
  from whatever ran before it. A `cd` would put that state back - mutable,
  ambient, and readable only by replaying the program in your head - and `sic
  plan` could no longer print where a call runs, because it would depend on
  control flow.
