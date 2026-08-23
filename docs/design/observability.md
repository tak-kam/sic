# OpenTelemetry (phase 8)

The execution journal is the canonical record of a run. OpenTelemetry is a view
of it - an external interface, not the internal model.

```text
Internal:                    External:
Execution Journal   ────>    OTLP traces
(the source of truth)        OTLP metrics
```

The arrow only points one way. Nothing in the VM, the journal, or the language
depends on OTel, and none of the OTel vocabulary leaks back into the event
model. If the standard changes, this crate changes.

---

## 1. Converting, not sending

`sic-otel` turns journal events into OTLP/JSON. It does not open a socket.

That is not squeamishness about the work; it is where the boundary belongs.
Sending telemetry is an external effect, and an external effect is a capability.
A VM that could quietly post spans somewhere would be exactly the exfiltration
path phase 4 was careful not to build. So the exporter produces a document, and
getting it to a collector is somebody else's job - a capability, a sidecar, or
`curl`.

```console
$ sic run app.sic --journal run.jsonl
$ sic export run.jsonl --traces traces.json --metrics metrics.json
```

This also means the exporter is a pure function of the journal, and can run long
after the run finished, on a machine that never saw it.

---

## 2. Reading the journal back

Phase 4 said a journal reader belonged with replay, and did not write one.
Exporting needs one, so it arrives here: `sic-journal` gains a reader that turns
a line of JSONL back into an `Event`, using `sic-json`.

An unparseable line is skipped with a count, not a hard failure. A journal is
append-only and a run can be killed mid-write, so the last line may be a
fragment; refusing to export anything because of it would be worse than
exporting the rest and saying how many lines were dropped.

---

## 3. Spans

Journal events already carry a span and a parent, recorded as they happened, so
a trace is a matter of pairing starts with ends rather than reconstructing a
tree.

| Span | Opened by | Closed by |
|---|---|---|
| the run | `RunStarted` | `RunCompleted` / `RunFailed` |
| a task | `TaskStarted` | `TaskCompleted` / `TaskFailed` / `TaskAbandoned` |
| a function | `FunctionEntered` | `FunctionExited` |
| a capability call | `CapabilityRequested` | `CapabilityCompleted` / `CapabilityFailed` |

- `traceId` is the run id, which is already 128 bits.
- `spanId` is the journal's span id plus one, because OTLP reserves all-zero.
- A span that never closed - the run was killed, or a task was abandoned mid
  flight - is exported with the end time of the last event in the journal and a
  status saying it did not finish. Dropping it would hide exactly the runs
  worth looking at.
- Timestamps come from the `ts` field a sink added when it wrote the line. A
  journal without timestamps exports spans with zero duration rather than
  nothing at all: `seq` is the order, and the order is what the trace shape
  needs.

### Attributes

Language-specific attributes use the `sic.` namespace, per section 24 of the
specification:

```text
sic.run.id, sic.task.id, sic.capability.name, sic.capability.attempt,
sic.checkpoint.digest, sic.budget.remaining
```

For a model call the GenAI conventions apply, so a capability span for
`llm.invoke` also carries:

```text
gen_ai.system, gen_ai.operation.name, gen_ai.request.model
```

The model name is the grant's constraint, which the journal does not record.
Where it is unknown the attribute is omitted rather than guessed.

**Digests stay digests.** The journal records the digest of an argument, and the
export does the same. Telemetry is an exfiltration path, and converting to
another format is not a reason to start including values.

---

## 4. Metrics

Counters and durations that the runtime produces on its own, per section 25:

```text
sic.workflow.runs        sic.workflow.failures     sic.workflow.duration
sic.capability.calls     sic.capability.failures   sic.capability.duration
sic.task.started         sic.task.failed
sic.agent.invocations
sic.checkpoints.written
```

Each is a sum or a histogram over one journal, with the attributes that make it
worth splitting: the capability name, the workflow name. Exporting a single run
gives a single data point; a collector aggregates across runs, which is what a
collector is for.

`sic.agent.invocations` counts `llm.invoke` calls, because that is what an agent
is at this level - the exporter does not know what an agent is either.

---

## 5. What is not here

- **No logs.** There is no log event to export yet; `log` as a statement arrives
  with the phase that adds it. Failures appear as span statuses, which is where
  a trace backend looks for them.
- **No sending, no OTLP/gRPC, no protobuf.** JSON is what OTLP defines for the
  HTTP transport, and a document is what this produces.
- **No sampling and no batching.** Both are decisions about a stream of runs,
  and this converts one journal at a time.
- **No trust or secret attributes.** Section 19's types do not exist yet, so
  there is nothing to label. When they do, `sic.trust.level` is the attribute
  and the rule is that a `Secret<T>` never reaches an attribute at all.

---

## 6. Units of work

| # | Unit | Done when |
|---|------|-----------|
| 8-1 | A journal reader in `sic-journal` | a truncated last line is skipped and counted |
| 8-2 | `sic-otel`: spans from paired events | an unclosed span is exported with a status, not dropped |
| 8-3 | Attributes, including the GenAI ones | digests are still digests |
| 8-4 | Metrics | a failed run counts once in runs and once in failures |
| 8-5 | `sic export` | a journal from a resumed run exports as one trace |
