//! The register VM.
//!
//! The VM knows nothing about the outside world: no files, no clock, no network,
//! no processes. Everything it can do is decide what the next instruction is and
//! what it does to registers. That is what makes a run reproducible, and it is
//! what lets external effects arrive as capabilities without the VM ever holding
//! a credential.
//!
//! It expects verified bytecode. Where the verifier has already established a
//! property, the VM does not re-check it; where an index could still be out of
//! range because a caller skipped verification, it fails the run rather than
//! panicking.
//!
//! A run is a set of tasks scheduled cooperatively. A task yields at exactly two
//! instructions - `CALL_CAP` and `AWAIT` - and nowhere else, because those are
//! the only places it is already waiting. Preemption would make every point
//! between two instructions something a checkpoint has to represent.

pub mod checkpoint;
pub mod value;

use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::{Const, Program};
use sic_core::{CapError, CapRequest, CapValue, Digest};
use sic_journal::{EventKind, Journal, RunId, SpanId, TaskId, digest_values};

pub use checkpoint::{Checkpoint, CheckpointError};
pub use value::{Arena, Handle, Value};

/// Where a run ended up.
#[derive(Debug, Clone)]
pub enum Status {
    Finished(Value),
    Failed(FailInfo),
    /// No task can proceed until an effect answers. The driver asks the broker
    /// and calls `resume`.
    ///
    /// Suspending, rather than calling out through a trait, is what keeps this
    /// crate unable to reach the outside world at all. It is also exactly the
    /// point a checkpoint captures: everything needed to continue is in `Vm`.
    Suspended(CapRequest),
}

#[derive(Debug, Clone)]
pub struct FailInfo {
    pub kind: FailKind,
    pub func: String,
    pub pc: u32,
    /// The value passed to `FAIL`, when that is what ended the run.
    pub value: Option<Value>,
    /// Extra text, such as why a capability call failed.
    pub detail: Option<String>,
}

impl FailInfo {
    /// The line an error report and the journal both use.
    pub fn describe(&self) -> String {
        match (&self.detail, self.kind) {
            // A restored failure already is the text it reported.
            (Some(detail), FailKind::Restored) => detail.clone(),
            (Some(detail), _) => format!("{}: {detail}", self.kind.message()),
            (None, _) => self.kind.message().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// Integer arithmetic left the range of i64.
    Overflow,
    DivisionByZero,
    /// The program executed `FAIL`.
    Explicit,
    /// A capability call did not succeed. The reason is in `FailInfo::detail`.
    Capability,
    /// The instruction budget ran out.
    OutOfFuel,
    CallStackTooDeep,
    /// The task table is full. A run may have `MAX_TASKS` tasks at once, and
    /// this is a separate thing from a call stack that is too deep: the frames
    /// are fine, there is nowhere left to put another task.
    TooManyTasks,
    /// A list index was outside the list.
    IndexOutOfRange,
    /// A document did not parse, or did not fit the type it was checked
    /// against. The detail says which and where.
    Schema,
    /// A call site ran out of the budget its policy gave it.
    OutOfBudget,
    /// A task was awaited whose result had already been taken.
    TaskAlreadyAwaited,
    /// The task being awaited failed.
    AwaitedTaskFailed,
    /// Every task is waiting for another task, so none of them can proceed.
    Deadlock,
    /// A failure that happened before a checkpoint. Its text is carried in
    /// `detail`, because the kind it originally had is not what a resumed run
    /// needs to report - the message is.
    Restored,
    /// Something the verifier should have ruled out. Reaching this means the
    /// bytecode was run without verifying it, or the verifier has a hole.
    Internal(&'static str),
}

impl FailKind {
    pub fn message(self) -> &'static str {
        match self {
            FailKind::Overflow => "integer overflow",
            FailKind::DivisionByZero => "division by zero",
            FailKind::Explicit => "the program failed",
            FailKind::Capability => "a capability call failed",
            FailKind::OutOfFuel => "ran out of fuel",
            FailKind::CallStackTooDeep => "call stack too deep",
            FailKind::TooManyTasks => "too many tasks",
            FailKind::IndexOutOfRange => "the index is outside the list",
            FailKind::Schema => "the document does not fit the type",
            FailKind::OutOfBudget => "the call site is out of budget",
            FailKind::TaskAlreadyAwaited => "this task has already been awaited",
            FailKind::AwaitedTaskFailed => "the awaited task failed",
            FailKind::Deadlock => "every task is waiting for another task",
            FailKind::Restored => "a failure recorded before the checkpoint",
            FailKind::Internal(what) => what,
        }
    }
}

/// One activation record. Registers live in the task's stack, so a frame only
/// records where its window starts.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub func: u32,
    pub pc: u32,
    pub reg_base: usize,
    /// Register in the task that receives the return value.
    pub ret_reg: usize,
    /// The span this activation is, and the span it happened inside.
    pub span: SpanId,
    pub parent: Option<SpanId>,
}

/// A capability call a task is waiting on.
#[derive(Debug, Clone)]
pub(crate) struct PendingCap {
    /// Register in the task the answer goes into.
    pub reg: usize,
    pub index: u32,
    pub name: String,
    pub args: Vec<CapValue>,
    /// Which attempt is outstanding, counting from 1.
    pub attempt: u32,
    /// How many attempts the policy allows in total.
    pub attempts: u32,
    pub timeout_ms: u32,
    /// Which conversation the call belongs to, from the policy table. It
    /// travels in the pending call because a retry re-issues the request, and
    /// a retry that started a new conversation would not be one.
    pub conversation: u32,
    /// The site's tool allowance and answer deadline, from the policy table.
    /// They travel in the pending call because a retry re-issues the request.
    pub tools: u32,
    pub deadline_ms: u32,
    /// Which call site this is, so that what the agent used can be counted
    /// against the allowance of the site that allowed it.
    pub pc: u32,
    pub span: SpanId,
    pub parent: Option<SpanId>,
}

#[derive(Debug, Clone)]
pub(crate) enum TaskState {
    Ready,
    WaitingCap(PendingCap),
    /// Waiting for another task to finish.
    WaitingTask(u32),
    Finished(Value),
    /// The result was taken by an `await`; awaiting again is an error.
    Taken,
    Failed(FailInfo),
    /// The failure was reported to an awaiting task.
    FailureTaken,
}

impl TaskState {
    fn is_over(&self) -> bool {
        matches!(
            self,
            TaskState::Finished(_)
                | TaskState::Taken
                | TaskState::Failed(_)
                | TaskState::FailureTaken
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub regs: Vec<Value>,
    pub frames: Vec<Frame>,
    pub state: TaskState,
    /// The task's own span, which its function activations sit inside.
    pub span: SpanId,
    pub func_name: String,
}

/// Limits that keep a runaway program from exhausting the host.
const MAX_FRAMES: usize = 1024;
const MAX_REGS: usize = 1 << 16;
const MAX_TASKS: usize = 1024;
/// The default instruction budget, high enough for real work and low enough
/// that a non-terminating program stops on its own.
pub const DEFAULT_FUEL: u64 = 10_000_000;

pub struct Vm<'a> {
    program: &'a Program,
    pub(crate) tasks: Vec<Task>,
    /// Where round-robin scheduling resumes looking.
    cursor: usize,
    /// The task the last `Suspended` belongs to, and so the one `resume`
    /// answers.
    answering: Option<usize>,
    pub(crate) arena: Arena,
    /// Handles for the string constants, allocated once at startup rather than
    /// on every load.
    pub(crate) str_consts: Vec<Option<Handle>>,
    /// Every run produces events, whether or not anything is listening: the
    /// journal is the runtime's own account of what happened, not
    /// instrumentation a program has to add.
    pub(crate) journal: Journal,
    /// The span of the run itself, which every task sits inside.
    pub(crate) root_span: SpanId,
    pub(crate) fuel: u64,
    /// How many times each capability call site has run, for the budgets in
    /// the policy table. Keyed by pc, because that is what a budget is attached
    /// to; the VM does not know that some of those sites are agents.
    spent: std::collections::HashMap<u32, u32>,
    /// How many of the agent's own tools each call site has used. Counted here
    /// rather than in the broker for the same reason the budget is: it has to
    /// survive a checkpoint, or resuming would hand the run a fresh allowance.
    used_tools: std::collections::HashMap<u32, u32>,
}

impl std::fmt::Debug for Vm<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vm")
            .field("tasks", &self.tasks.len())
            .field("fuel", &self.fuel)
            .finish()
    }
}

impl<'a> Vm<'a> {
    /// A VM that records nothing.
    pub fn new(program: &'a Program, fuel: u64) -> Self {
        Self::with_journal(program, fuel, Journal::discard())
    }

    pub fn with_journal(program: &'a Program, fuel: u64, journal: Journal) -> Self {
        let mut arena = Arena::default();
        let str_consts = program
            .consts
            .iter()
            .map(|c| match c {
                Const::Str(s) => Some(arena.alloc_str(s.clone())),
                Const::EmptyList(_) => Some(arena.alloc_list(Vec::new())),
                _ => None,
            })
            .collect();
        Self {
            program,
            tasks: Vec::new(),
            cursor: 0,
            answering: None,
            arena,
            str_consts,
            journal,
            root_span: SpanId(0),
            fuel,
            spent: std::collections::HashMap::new(),
            used_tools: std::collections::HashMap::new(),
        }
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// How much fuel is left.
    pub fn fuel(&self) -> u64 {
        self.fuel
    }

    /// How many tasks the run has created.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Renders a value for a human.
    pub fn display(&self, value: &Value) -> String {
        value.display(&self.arena)
    }

    /// Whether the VM is waiting for a capability result.
    pub fn is_suspended(&self) -> bool {
        self.answering.is_some()
    }

    /// The capability the VM is waiting on, if it is waiting.
    pub fn pending_capability(&self) -> Option<&str> {
        let index = self.answering?;
        match &self.tasks.get(index)?.state {
            TaskState::WaitingCap(pending) => Some(pending.name.as_str()),
            _ => None,
        }
    }

    // ---- starting, answering, finishing ----

    /// Calls a function as the run's first task and schedules until the run
    /// ends or has to wait.
    pub fn run(&mut self, func: u32, args: &[Value]) -> Status {
        let Some(def) = self.program.funcs.get(func as usize) else {
            return self.fail_now(FailKind::Internal("no such function"));
        };
        if args.len() != def.param_count() {
            return self.fail_now(FailKind::Internal("wrong number of arguments"));
        }

        let name = def.name.clone();
        self.root_span = self.journal.new_span();
        let arg_digest = digest_values(
            &args
                .iter()
                .map(|a| self.to_cap_value(a).unwrap_or(CapValue::Unit))
                .collect::<Vec<_>>(),
        );
        self.journal.emit(
            self.root_span,
            None,
            EventKind::RunStarted {
                workflow: name.clone(),
                args: arg_digest,
            },
        );

        if let Err(kind) = self.spawn_task(func, args.to_vec()) {
            return self.fail_now(kind);
        }
        let status = self.schedule();
        self.record_end(&status);
        status
    }

    /// Continues the waiting task with the value the capability produced.
    /// Records what an agent did while answering the call that is outstanding.
    ///
    /// A capability the agent reached through the broker is a capability call:
    /// authorized against the same manifest, performed by the same code. So it
    /// enters the journal as one, and needs no vocabulary of its own.
    ///
    /// They arrive when the call returns rather than as they happen, because
    /// the journal belongs to this side and the agent is on the other one. What
    /// that costs is visible only to somebody tailing the file; what it buys is
    /// that the events are nested under the call they happened inside, which is
    /// where a reader looks for them.
    pub fn record_tool_uses(&mut self, actions: &[sic_core::AgentAction]) {
        let Some(index) = self.answering else {
            return;
        };
        let TaskState::WaitingCap(pending) = self.tasks[index].state.clone() else {
            return;
        };
        let task = TaskId(index as u64);
        // Charged against the site that allowed them, and kept here rather than
        // in the broker so that resuming does not hand the run a fresh
        // allowance. Every action counts, refused ones included: a refusal is
        // an attempt, and a loop of refused attempts is the runaway a tool
        // allowance is for.
        if pending.tools > 0 {
            *self.used_tools.entry(pending.pc).or_insert(0) += actions.len() as u32;
        }
        for action in actions {
            let span = self.journal.new_span();
            match action {
                // A capability the agent reached through the broker is a
                // capability call, so it enters as one.
                sic_core::AgentAction::Capability { cap, args, outcome } => {
                    self.journal.emit_for(
                        task,
                        span,
                        Some(pending.span),
                        EventKind::CapabilityRequested {
                            cap: cap.clone(),
                            args: *args,
                            attempt: 1,
                        },
                    );
                    let kind = match outcome {
                        Ok(result) => EventKind::CapabilityCompleted {
                            cap: cap.clone(),
                            result: *result,
                            attempt: 1,
                        },
                        Err(error) => EventKind::CapabilityFailed {
                            cap: cap.clone(),
                            error: error.clone(),
                            attempt: 1,
                        },
                    };
                    self.journal.emit_for(task, span, Some(pending.span), kind);
                }
                // A tool of the agent's own is not a capability, and the
                // journal says so rather than borrowing a word that means
                // something else here.
                sic_core::AgentAction::Tool {
                    tool,
                    input,
                    allowed,
                    reason,
                } => self.journal.emit_for(
                    task,
                    span,
                    Some(pending.span),
                    EventKind::ToolUsed {
                        tool: tool.clone(),
                        input: *input,
                        allowed: *allowed,
                        reason: reason.clone(),
                    },
                ),
            }
        }
    }

    pub fn resume(&mut self, value: CapValue) -> Status {
        let Some(index) = self.answering.take() else {
            return self.fail_now(FailKind::Internal("resumed while not suspended"));
        };
        let TaskState::WaitingCap(pending) = self.tasks[index].state.clone() else {
            return self.fail_now(FailKind::Internal("the answered task is not waiting"));
        };
        self.journal.emit_for(
            TaskId(index as u64),
            pending.span,
            pending.parent,
            EventKind::CapabilityCompleted {
                cap: pending.name.clone(),
                result: digest_values(std::slice::from_ref(&value)),
                attempt: pending.attempt,
            },
        );
        let value = self.intern_cap_value(value);
        self.tasks[index].regs[pending.reg] = value;
        self.tasks[index].state = TaskState::Ready;

        let status = self.schedule();
        self.record_end(&status);
        status
    }

    /// Reports that the outstanding capability call did not succeed.
    ///
    /// Retrying is the VM's decision, not the broker's: the VM knows the
    /// policy, and every attempt is an event, so an audit shows what actually
    /// happened rather than only what finally worked.
    pub fn resume_failed(&mut self, error: &CapError) -> Status {
        let Some(index) = self.answering.take() else {
            return self.fail_now(FailKind::Internal("resumed while not suspended"));
        };
        let TaskState::WaitingCap(pending) = self.tasks[index].state.clone() else {
            return self.fail_now(FailKind::Internal("the answered task is not waiting"));
        };
        self.journal.emit_for(
            TaskId(index as u64),
            pending.span,
            pending.parent,
            EventKind::CapabilityFailed {
                cap: pending.name.clone(),
                error: error.message.clone(),
                attempt: pending.attempt,
            },
        );

        if pending.attempt < pending.attempts {
            let mut next = pending.clone();
            next.attempt += 1;
            self.journal.emit_for(
                TaskId(index as u64),
                next.span,
                next.parent,
                EventKind::CapabilityRequested {
                    cap: next.name.clone(),
                    args: digest_values(&next.args),
                    attempt: next.attempt,
                },
            );
            self.tasks[index].state = TaskState::WaitingCap(next);
            let status = self.schedule();
            self.record_end(&status);
            return status;
        }

        let info = self.fail_info(
            index,
            FailKind::Capability,
            None,
            Some(error.message.clone()),
        );
        self.finish_task(index, TaskState::Failed(info));
        let status = self.schedule();
        self.record_end(&status);
        status
    }

    /// Records how a run ended. A suspension is not an ending.
    fn record_end(&mut self, status: &Status) {
        let root = self.root_span;
        match status {
            Status::Finished(value) => {
                let result = self.to_cap_value(value).unwrap_or(CapValue::Unit);
                self.journal.emit(
                    root,
                    None,
                    EventKind::RunCompleted {
                        result: digest_values(&[result]),
                    },
                );
            }
            Status::Failed(info) => {
                let error = info.describe();
                self.journal
                    .emit(root, None, EventKind::RunFailed { error });
            }
            Status::Suspended(_) => {}
        }
    }

    // ---- checkpoints ----

    /// Writes out a suspended run, so it can continue later or elsewhere.
    ///
    /// Returns `None` when the VM is not suspended: there is no such thing as
    /// checkpointing a run in the middle of an instruction, and there is no
    /// need for one, because a run that is not waiting can simply keep going.
    pub fn checkpoint(&mut self, program_digest: Digest, question: &str) -> Option<Vec<u8>> {
        let answering = self.answering?;
        let cap = match &self.tasks[answering].state {
            TaskState::WaitingCap(pending) => pending.name.clone(),
            _ => return None,
        };
        self.journal
            .emit(self.root_span, None, EventKind::RunSuspended { cap });

        // The `checkpoint_written` event below consumes the next sequence
        // number, so what is saved is the one after it. A resumed run continues
        // the same sequence, and must not reuse a number.
        let seq = self.journal.seq() + 1;

        let saved = Checkpoint {
            program_digest,
            run: self.journal.run_id().0,
            seq,
            next_span: self.journal.next_span_id(),
            root_span: self.root_span.0,
            fuel: self.fuel,
            cursor: self.cursor as u32,
            used_tools: {
                let mut used: Vec<(u32, u32)> =
                    self.used_tools.iter().map(|(k, v)| (*k, *v)).collect();
                used.sort_unstable();
                used
            },
            spent: {
                let mut spent: Vec<(u32, u32)> = self.spent.iter().map(|(k, v)| (*k, *v)).collect();
                // Sorted so that two checkpoints of the same run are the same
                // bytes.
                spent.sort_unstable();
                spent
            },
            answering: answering as u32,
            question: question.to_string(),
            tasks: self.tasks.iter().map(snapshot_task).collect(),
            str_consts: self.str_consts.iter().map(|h| h.map(|h| h.0)).collect(),
            strings: self.arena.strings().to_vec(),
            lists: self.arena.lists().to_vec(),
            objects: self.arena.objects().to_vec(),
        };

        let bytes = saved.encode();
        self.journal.emit(
            self.root_span,
            None,
            EventKind::CheckpointWritten {
                digest: Checkpoint::digest(&bytes),
                bytes: bytes.len() as u64,
            },
        );
        Some(bytes)
    }

    /// Rebuilds a suspended run from a checkpoint, returning the VM and what it
    /// is waiting for.
    ///
    /// The checkpoint is treated with the same suspicion as bytecode: it came
    /// from a file. Everything a restored VM would otherwise assume is checked
    /// here, including that the checkpoint belongs to this program - resuming
    /// against different bytecode would continue one program inside another.
    pub fn restore(
        program: &'a Program,
        bytes: &[u8],
        program_digest: Digest,
        journal_sink: Box<dyn sic_journal::Sink>,
    ) -> Result<(Self, String), CheckpointError> {
        let saved = Checkpoint::decode(bytes)?;
        if saved.program_digest != program_digest {
            return Err(CheckpointError::new(
                "this checkpoint belongs to different bytecode",
            ));
        }
        if saved.str_consts.len() != program.consts.len() {
            return Err(CheckpointError::new(
                "the checkpoint's constants do not match this program's",
            ));
        }
        for (index, task) in saved.tasks.iter().enumerate() {
            for (i, frame) in task.frames.iter().enumerate() {
                let Some(def) = program.funcs.get(frame.func as usize) else {
                    return Err(CheckpointError::new(format!(
                        "frame {i} of task {index} names function {}, which this program does not have",
                        frame.func
                    )));
                };
                if !def.contains_pc(frame.pc) {
                    return Err(CheckpointError::new(format!(
                        "frame {i} of task {index} is at instruction {} which is outside `{}`",
                        frame.pc, def.name
                    )));
                }
            }
        }

        let question = saved.question.clone();
        let cap = match &saved.tasks[saved.answering as usize].state {
            checkpoint::TaskStateSnapshot::WaitingCap(pending) => pending.cap.clone(),
            _ => return Err(CheckpointError::new("the checkpoint is not waiting")),
        };

        let journal = Journal::resumed(RunId(saved.run), saved.seq, saved.next_span, journal_sink);

        let mut vm = Self {
            program,
            tasks: saved.tasks.iter().map(restore_task).collect(),
            cursor: saved.cursor as usize,
            answering: Some(saved.answering as usize),
            arena: Arena::from_parts(saved.strings, saved.lists, saved.objects),
            str_consts: saved.str_consts.iter().map(|h| h.map(Handle)).collect(),
            journal,
            root_span: SpanId(saved.root_span),
            fuel: saved.fuel,
            spent: saved.spent.iter().copied().collect(),
            used_tools: saved.used_tools.iter().copied().collect(),
        };
        vm.journal
            .emit(vm.root_span, None, EventKind::RunResumed { cap });
        Ok((vm, question))
    }

    // ---- the scheduler ----

    /// Runs tasks until the run ends or nothing can proceed.
    fn schedule(&mut self) -> Status {
        loop {
            // The run is over when the entry task is over. Other tasks are
            // abandoned rather than waited for, so a program can choose to stop
            // early.
            match &self.tasks[0].state {
                TaskState::Finished(value) => {
                    let value = value.clone();
                    self.abandon_unfinished();
                    return Status::Finished(value);
                }
                TaskState::Taken => {
                    self.abandon_unfinished();
                    return Status::Finished(Value::Unit);
                }
                TaskState::Failed(info) => {
                    let info = info.clone();
                    self.abandon_unfinished();
                    return Status::Failed(info);
                }
                _ => {}
            }

            if let Some(index) = self.next_ready() {
                self.run_task(index);
                continue;
            }

            // Nothing can run. If a task is waiting on an effect, the driver
            // has to answer it; a request is made for one task at a time, which
            // keeps the broker interface a simple question and answer.
            if let Some(index) = self.next_waiting_on_capability() {
                let TaskState::WaitingCap(pending) = &self.tasks[index].state else {
                    unreachable!("just matched on it");
                };
                let request = CapRequest {
                    index: pending.index,
                    name: pending.name.clone(),
                    args: pending.args.clone(),
                    task: index as u32,
                    attempt: pending.attempt,
                    timeout_ms: pending.timeout_ms,
                    conversation: pending.conversation,
                    tools_left: tools_left(&self.used_tools, pending),
                    answer_ms: pending.deadline_ms,
                };
                self.answering = Some(index);
                return Status::Suspended(request);
            }

            // Every remaining task is waiting for another task, so none of them
            // will ever proceed.
            let info = self.fail_info(0, FailKind::Deadlock, None, None);
            self.tasks[0].state = TaskState::Failed(info.clone());
            return Status::Failed(info);
        }
    }

    /// The next runnable task, round-robin from the cursor so that no task can
    /// starve another.
    fn next_ready(&mut self) -> Option<usize> {
        let count = self.tasks.len();
        for offset in 0..count {
            let index = (self.cursor + offset) % count;
            if matches!(self.tasks[index].state, TaskState::Ready) {
                self.cursor = (index + 1) % count;
                return Some(index);
            }
        }
        None
    }

    fn next_waiting_on_capability(&self) -> Option<usize> {
        self.tasks
            .iter()
            .position(|t| matches!(t.state, TaskState::WaitingCap(_)))
    }

    /// Records the tasks that were still going when the run ended.
    ///
    /// A silently discarded task is how a workflow ends up claiming to have
    /// succeeded when part of it did not.
    fn abandon_unfinished(&mut self) {
        for index in 1..self.tasks.len() {
            match &self.tasks[index].state {
                state if state.is_over() => {}
                _ => {
                    let span = self.tasks[index].span;
                    self.journal.emit_for(
                        TaskId(index as u64),
                        span,
                        Some(self.root_span),
                        EventKind::TaskAbandoned,
                    );
                }
            }
        }
    }

    /// Starts a task running `func`, and returns its id.
    ///
    /// The two ways this fails are different failures, and saying so is the
    /// point of returning the kind rather than `None`: a full task table is
    /// something the program did, and a function index that is not in the
    /// program is something the bytecode is.
    fn spawn_task(&mut self, func: u32, args: Vec<Value>) -> Result<u32, FailKind> {
        if self.tasks.len() >= MAX_TASKS {
            return Err(FailKind::TooManyTasks);
        }
        let Some(def) = self.program.funcs.get(func as usize) else {
            // The verifier checks every `SPAWN` operand against the function
            // table, so reaching this means bytecode was run without being
            // verified, or the verifier has a hole. It is not a limit the
            // program hit, and it must not be reported as one.
            return Err(FailKind::Internal(
                "spawn of a function that does not exist",
            ));
        };
        let (reg_count, code_off, name) = (def.reg_count as usize, def.code_off, def.name.clone());

        let id = self.tasks.len() as u32;
        let span = self.journal.new_span();
        self.journal.emit_for(
            TaskId(id as u64),
            span,
            Some(self.root_span),
            EventKind::TaskStarted { func: name.clone() },
        );

        let mut regs = vec![Value::Unit; reg_count];
        for (i, arg) in args.into_iter().enumerate() {
            if i < regs.len() {
                regs[i] = arg;
            }
        }
        let frame_span = self.journal.new_span();
        self.journal.emit_for(
            TaskId(id as u64),
            frame_span,
            Some(span),
            EventKind::FunctionEntered { func: name.clone() },
        );
        self.tasks.push(Task {
            regs,
            frames: vec![Frame {
                func,
                pc: code_off,
                reg_base: 0,
                ret_reg: 0,
                span: frame_span,
                parent: Some(span),
            }],
            state: TaskState::Ready,
            span,
            func_name: name,
        });
        Ok(id)
    }

    /// Moves a task to a terminal state and wakes whatever was waiting on it.
    fn finish_task(&mut self, index: usize, state: TaskState) {
        let span = self.tasks[index].span;
        let task_id = TaskId(index as u64);
        match &state {
            TaskState::Finished(value) => {
                let result = self.to_cap_value(value).unwrap_or(CapValue::Unit);
                self.journal.emit_for(
                    task_id,
                    span,
                    Some(self.root_span),
                    EventKind::TaskCompleted {
                        result: digest_values(&[result]),
                    },
                );
            }
            TaskState::Failed(info) => {
                let error = info.describe();
                self.journal.emit_for(
                    task_id,
                    span,
                    Some(self.root_span),
                    EventKind::TaskFailed { error },
                );
            }
            _ => {}
        }
        self.tasks[index].state = state;

        for other in 0..self.tasks.len() {
            if let TaskState::WaitingTask(waited) = self.tasks[other].state {
                if waited as usize == index {
                    self.tasks[other].state = TaskState::Ready;
                }
            }
        }
    }

    // ---- executing one task ----

    /// Runs one task until it yields, finishes, or fails.
    fn run_task(&mut self, index: usize) {
        loop {
            if self.fuel == 0 {
                let info = self.fail_info(index, FailKind::OutOfFuel, None, None);
                self.finish_task(index, TaskState::Failed(info));
                return;
            }
            self.fuel -= 1;

            let Some(frame) = self.tasks[index].frames.last() else {
                let info = self.fail_info(index, FailKind::Internal("no frame to run"), None, None);
                self.finish_task(index, TaskState::Failed(info));
                return;
            };
            let (pc, base) = (frame.pc, frame.reg_base);
            let Some(inst) = self.program.code.get(pc as usize).copied() else {
                let info = self.fail_info(
                    index,
                    FailKind::Internal("pc is outside the code"),
                    None,
                    None,
                );
                self.finish_task(index, TaskState::Failed(info));
                return;
            };
            let Some(op) = inst.op() else {
                let info = self.fail_info(index, FailKind::Internal("unknown opcode"), None, None);
                self.finish_task(index, TaskState::Failed(info));
                return;
            };
            self.tasks[index]
                .frames
                .last_mut()
                .expect("checked above")
                .pc = pc + 1;

            let (a, b, c) = (inst.a() as usize, inst.b() as usize, inst.c() as usize);

            macro_rules! die {
                ($kind:expr, $value:expr, $detail:expr) => {{
                    let info = self.fail_info(index, $kind, $value, $detail);
                    self.finish_task(index, TaskState::Failed(info));
                    return;
                }};
            }

            match op {
                Op::LoadConst => {
                    let Some(konst) = self.program.consts.get(inst.bx() as usize) else {
                        die!(
                            FailKind::Internal("constant index out of range"),
                            None,
                            None
                        );
                    };
                    let value = match konst {
                        Const::Unit => Value::Unit,
                        Const::Bool(v) => Value::Bool(*v),
                        Const::I64(v) => Value::I64(*v),
                        Const::F64(v) => Value::F64(*v),
                        Const::Str(_) => match self.str_consts[inst.bx() as usize] {
                            Some(h) => Value::Str(h),
                            None => die!(FailKind::Internal("missing string"), None, None),
                        },
                        // A list cannot be modified, so one empty list per
                        // constant is enough for the whole run.
                        Const::EmptyList(_) => match self.str_consts[inst.bx() as usize] {
                            Some(h) => Value::List(h),
                            None => die!(FailKind::Internal("missing empty list"), None, None),
                        },
                    };
                    self.set(index, base + a, value);
                }
                Op::Move => {
                    let v = self.get(index, base + b);
                    self.set(index, base + a, v);
                }
                Op::AddI64 | Op::SubI64 | Op::MulI64 | Op::DivI64 | Op::RemI64 => {
                    let (Value::I64(l), Value::I64(r)) =
                        (self.get(index, base + b), self.get(index, base + c))
                    else {
                        die!(
                            FailKind::Internal("arithmetic on a non-integer"),
                            None,
                            None
                        );
                    };
                    let result = match op {
                        Op::AddI64 => l.checked_add(r),
                        Op::SubI64 => l.checked_sub(r),
                        Op::MulI64 => l.checked_mul(r),
                        Op::DivI64 => {
                            if r == 0 {
                                die!(FailKind::DivisionByZero, None, None);
                            }
                            l.checked_div(r)
                        }
                        _ => {
                            if r == 0 {
                                die!(FailKind::DivisionByZero, None, None);
                            }
                            l.checked_rem(r)
                        }
                    };
                    // `None` here is i64::MIN / -1 as well as plain overflow.
                    let Some(value) = result else {
                        die!(FailKind::Overflow, None, None);
                    };
                    self.set(index, base + a, Value::I64(value));
                }
                // The only instruction that allocates without a capability
                // having been asked, and the only one that costs more than one
                // fuel: a byte of the result is a byte of the arena, so a byte
                // of the result is a unit of fuel. That makes the budget a
                // bound on the arena as well as on the instruction count - a
                // run can join at most `fuel` bytes, ever - and it is charged
                // before the string is built, so the memory a program cannot
                // afford is never taken.
                Op::Concat => {
                    let (Value::Str(l), Value::Str(r)) =
                        (self.get(index, base + b), self.get(index, base + c))
                    else {
                        die!(FailKind::Internal("CONCAT of a non-string"), None, None);
                    };
                    // Bytes rather than characters, which is the opposite of
                    // what `LEN` counts: a length is a fact about the text and
                    // this is a fact about the memory.
                    let cost = (self.arena.str(l).len() + self.arena.str(r).len()) as u64;
                    if self.fuel < cost {
                        self.fuel = 0;
                        die!(FailKind::OutOfFuel, None, None);
                    }
                    self.fuel -= cost;
                    let mut joined = String::with_capacity(cost as usize);
                    joined.push_str(self.arena.str(l));
                    joined.push_str(self.arena.str(r));
                    let handle = self.arena.alloc_str(joined);
                    self.set(index, base + a, Value::Str(handle));
                }
                Op::Eq | Op::Ne => {
                    let (l, r) = (self.get(index, base + b), self.get(index, base + c));
                    let equal = self.values_equal(&l, &r);
                    let value = Value::Bool(if op == Op::Eq { equal } else { !equal });
                    self.set(index, base + a, value);
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    let (Value::I64(l), Value::I64(r)) =
                        (self.get(index, base + b), self.get(index, base + c))
                    else {
                        die!(
                            FailKind::Internal("comparison on a non-integer"),
                            None,
                            None
                        );
                    };
                    let result = match op {
                        Op::Lt => l < r,
                        Op::Le => l <= r,
                        Op::Gt => l > r,
                        _ => l >= r,
                    };
                    self.set(index, base + a, Value::Bool(result));
                }
                Op::Not => {
                    let Value::Bool(v) = self.get(index, base + b) else {
                        die!(FailKind::Internal("`not` on a non-boolean"), None, None);
                    };
                    self.set(index, base + a, Value::Bool(!v));
                }
                Op::Jump => self.jump(index, inst.sbx()),
                Op::JumpIf | Op::JumpIfNot => {
                    let Value::Bool(cond) = self.get(index, base + a) else {
                        die!(FailKind::Internal("branch on a non-boolean"), None, None);
                    };
                    if cond == (op == Op::JumpIf) {
                        self.jump(index, inst.sbx());
                    }
                }
                Op::Call => {
                    if let Err(kind) = self.call(index, b as u32, base + c, base + a) {
                        die!(kind, None, None);
                    }
                }
                Op::Spawn => {
                    let Some(def) = self.program.funcs.get(b) else {
                        die!(
                            FailKind::Internal("spawn of a function that does not exist"),
                            None,
                            None
                        );
                    };
                    let args: Vec<Value> = (0..def.param_count())
                        .map(|i| self.get(index, base + c + i))
                        .collect();
                    let id = match self.spawn_task(b as u32, args) {
                        Ok(id) => id,
                        Err(kind) => die!(kind, None, None),
                    };
                    self.set(index, base + a, Value::Task(id));
                }
                Op::Await => {
                    let Value::Task(id) = self.get(index, base + b) else {
                        die!(FailKind::Internal("await of a non-task"), None, None);
                    };
                    let Some(other) = self.tasks.get(id as usize) else {
                        die!(FailKind::Internal("await of an unknown task"), None, None);
                    };
                    match other.state.clone() {
                        TaskState::Finished(value) => {
                            // A result is moved out of the task, not copied, so
                            // that a task cannot be awaited twice by accident.
                            self.tasks[id as usize].state = TaskState::Taken;
                            self.set(index, base + a, value);
                        }
                        TaskState::Taken => {
                            die!(FailKind::TaskAlreadyAwaited, None, None);
                        }
                        TaskState::Failed(info) => {
                            self.tasks[id as usize].state = TaskState::FailureTaken;
                            die!(FailKind::AwaitedTaskFailed, None, Some(info.describe()));
                        }
                        TaskState::FailureTaken => {
                            die!(FailKind::TaskAlreadyAwaited, None, None);
                        }
                        _ => {
                            // Step back onto the AWAIT so it is retried when the
                            // task finishes; the register still holds the task.
                            self.tasks[index]
                                .frames
                                .last_mut()
                                .expect("a frame was running")
                                .pc = pc;
                            self.tasks[index].state = TaskState::WaitingTask(id);
                            return;
                        }
                    }
                }
                Op::MakeObject => {
                    let field_count = self
                        .program
                        .types
                        .get(b)
                        .and_then(|t| t.fields())
                        .map(<[(String, u32)]>::len)
                        .unwrap_or(0);
                    let fields: Vec<Value> = (0..field_count)
                        .map(|i| self.get(index, base + c + i))
                        .collect();
                    let handle = self.arena.alloc_object(fields);
                    self.set(index, base + a, Value::Object(handle));
                }
                Op::GetField => {
                    let Value::Object(handle) = self.get(index, base + b) else {
                        die!(
                            FailKind::Internal("field access on a non-object"),
                            None,
                            None
                        );
                    };
                    let Some(value) = self.arena.object(handle).get(c).cloned() else {
                        die!(FailKind::Internal("field index out of range"), None, None);
                    };
                    self.set(index, base + a, value);
                }
                Op::MakeList => {
                    let elements: Vec<Value> =
                        (0..c).map(|i| self.get(index, base + b + i)).collect();
                    let handle = self.arena.alloc_list(elements);
                    self.set(index, base + a, Value::List(handle));
                }
                Op::GetIndex => {
                    let Value::List(handle) = self.get(index, base + b) else {
                        die!(FailKind::Internal("indexing a non-list"), None, None);
                    };
                    let Value::I64(position) = self.get(index, base + c) else {
                        die!(FailKind::Internal("a non-integer index"), None, None);
                    };
                    let list = self.arena.list(handle);
                    // There is no option type to return instead, and a silent
                    // default would be worse than stopping.
                    let Some(value) = usize::try_from(position)
                        .ok()
                        .and_then(|i| list.get(i))
                        .cloned()
                    else {
                        die!(
                            FailKind::IndexOutOfRange,
                            None,
                            Some(format!("index {position} of a list of {}", list.len()))
                        );
                    };
                    self.set(index, base + a, value);
                }
                Op::Len => {
                    let length = match self.get(index, base + b) {
                        Value::List(handle) => self.arena.list(handle).len(),
                        // Characters, not bytes: a length in bytes would be
                        // about the encoding rather than the text.
                        Value::Str(handle) => self.arena.str(handle).chars().count(),
                        _ => die!(
                            FailKind::Internal("len of neither a list nor a string"),
                            None,
                            None
                        ),
                    };
                    self.set(index, base + a, Value::I64(length as i64));
                }
                Op::FromJson => {
                    let Value::Str(handle) = self.get(index, base + c) else {
                        die!(FailKind::Internal("from_json of a non-string"), None, None);
                    };
                    let text = self.arena.str(handle).to_string();
                    match self.value_from_json(&text, b as u32) {
                        Ok(value) => self.set(index, base + a, value),
                        // The run fails here, at the boundary, rather than
                        // wherever the malformed value would first be used.
                        Err(detail) => die!(FailKind::Schema, None, Some(detail)),
                    }
                }
                Op::CallCap => {
                    // Three jobs, and they are a method rather than seventy
                    // lines of this `match`: see `begin_capability_call`.
                    if let Err(info) = self.begin_capability_call(index, inst, pc, base) {
                        self.finish_task(index, TaskState::Failed(info));
                    }
                    return;
                }
                Op::Return => {
                    let value = self.get(index, base + a);
                    let frame = self.tasks[index].frames.pop().expect("a frame was running");
                    let func_name = self
                        .program
                        .funcs
                        .get(frame.func as usize)
                        .map(|f| f.name.clone())
                        .unwrap_or_default();
                    self.journal.emit_for(
                        TaskId(index as u64),
                        frame.span,
                        frame.parent,
                        EventKind::FunctionExited { func: func_name },
                    );
                    self.tasks[index].regs.truncate(frame.reg_base);
                    if self.tasks[index].frames.is_empty() {
                        self.finish_task(index, TaskState::Finished(value));
                        return;
                    }
                    self.set(index, frame.ret_reg, value);
                }
                Op::Fail => {
                    let value = self.get(index, base + a);
                    die!(FailKind::Explicit, Some(value), None);
                }
                Op::Halt => {
                    self.finish_task(index, TaskState::Finished(Value::Unit));
                    return;
                }
                // The only instruction whose whole effect is a journal entry.
                // The VM does no I/O - a sink is the CLI's code and decides
                // whether a person sees this - so what happens here is that
                // the event exists.
                Op::Log => {
                    let Value::Str(handle) = self.get(index, base + b) else {
                        die!(
                            FailKind::Internal("LOG on a register that is not a string"),
                            None,
                            None
                        );
                    };
                    let message = self.arena.str(handle).to_string();
                    let level = match sic_journal::LogLevel::from_code(a as u8) {
                        Some(level) => level,
                        None => die!(FailKind::Internal("LOG names no level"), None, None),
                    };
                    let parent = self.tasks[index].frames.last().map(|f| f.span);
                    let span = self.journal.new_span();
                    self.journal.emit_for(
                        TaskId(index as u64),
                        span,
                        parent,
                        EventKind::Logged { level, message },
                    );
                }
            }
        }
    }

    /// Pushes a frame for `func` inside a task, copying the arguments into it.
    fn call(
        &mut self,
        index: usize,
        func: u32,
        arg_base: usize,
        ret_reg: usize,
    ) -> Result<(), FailKind> {
        let Some(def) = self.program.funcs.get(func as usize) else {
            return Err(FailKind::Internal("call to a function that does not exist"));
        };
        let (argc, reg_count, code_off, name) = (
            def.param_count(),
            def.reg_count as usize,
            def.code_off,
            def.name.clone(),
        );

        if self.tasks[index].frames.len() >= MAX_FRAMES {
            return Err(FailKind::CallStackTooDeep);
        }
        let new_base = self.tasks[index].regs.len();
        if new_base + reg_count > MAX_REGS {
            return Err(FailKind::CallStackTooDeep);
        }

        // The callee's window sits above every register in use, so copying the
        // arguments cannot overwrite one of them.
        self.tasks[index]
            .regs
            .resize(new_base + reg_count, Value::Unit);
        for i in 0..argc {
            let arg = self.get(index, arg_base + i);
            self.tasks[index].regs[new_base + i] = arg;
        }
        let parent = self.tasks[index].frames.last().map(|f| f.span);
        let span = self.journal.new_span();
        self.journal.emit_for(
            TaskId(index as u64),
            span,
            parent,
            EventKind::FunctionEntered { func: name },
        );
        self.tasks[index].frames.push(Frame {
            func,
            pc: code_off,
            reg_base: new_base,
            ret_reg,
            span,
            parent,
        });
        Ok(())
    }

    // ---- values ----

    /// Parses a document and builds a value of the given type from it.
    ///
    /// Validation happens in the VM rather than the broker: the value goes into
    /// the VM's arena, and the broker has no business knowing about types.
    fn value_from_json(&mut self, text: &str, ty: u32) -> Result<Value, String> {
        let json = sic_json::parse(text).map_err(|e| e.to_string())?;
        let mut path = String::new();
        self.build_from_json(&json, ty, &mut path)
    }

    fn build_from_json(
        &mut self,
        json: &sic_json::Json,
        ty: u32,
        path: &mut String,
    ) -> Result<Value, String> {
        use sic_bytecode::TypeDesc;
        use sic_json::Json;

        let Some(desc) = self.program.types.get(ty as usize).cloned() else {
            return Err(at(path, "the type is not in this program"));
        };
        let expected = self.program.type_name(ty);
        match (&desc, json) {
            (TypeDesc::Unit, Json::Null) => Ok(Value::Unit),
            (TypeDesc::Bool, Json::Bool(v)) => Ok(Value::Bool(*v)),
            (TypeDesc::Int, Json::Int(v)) => Ok(Value::I64(*v)),
            // A whole number is a Float; the reverse is not true, because
            // truncating would change the value.
            (TypeDesc::Float, Json::Float(v)) => Ok(Value::F64(*v)),
            (TypeDesc::Float, Json::Int(v)) => Ok(Value::F64(*v as f64)),
            (TypeDesc::Str, Json::Str(s)) => {
                let handle = self.arena.alloc_str(s.clone());
                Ok(Value::Str(handle))
            }
            (TypeDesc::List(element), Json::Array(items)) => {
                let element = *element;
                let mut values = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    let mark = path.len();
                    path.push_str(&format!("[{i}]"));
                    values.push(self.build_from_json(item, element, path)?);
                    path.truncate(mark);
                }
                let handle = self.arena.alloc_list(values);
                Ok(Value::List(handle))
            }
            (TypeDesc::Object { name, fields }, Json::Object(members)) => {
                let declared = fields.clone();
                let type_name = name.clone();
                let field_names: Vec<String> = declared.iter().map(|(n, _)| n.clone()).collect();
                let mut values = Vec::with_capacity(declared.len());
                for (field_name, field_type) in &declared {
                    let field_name = field_name.clone();
                    let Some((_, value)) = members.iter().find(|(n, _)| *n == field_name) else {
                        // Every field is required, so a missing one is a
                        // mismatch rather than a default.
                        return Err(at(
                            path,
                            &format!("`{type_name}` needs a field `{field_name}`"),
                        ));
                    };
                    let mark = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&field_name);
                    values.push(self.build_from_json(value, *field_type, path)?);
                    path.truncate(mark);
                }
                for (name, _) in members {
                    if !field_names.contains(name) {
                        return Err(at(path, &format!("`{type_name}` has no field `{name}`")));
                    }
                }
                let handle = self.arena.alloc_object(values);
                Ok(Value::Object(handle))
            }
            (_, found) => Err(at(
                path,
                &format!("expected {expected}, found {}", found.kind()),
            )),
        }
    }

    /// Copies a value out of the VM so it can cross the broker boundary.
    ///
    /// Handles are meaningless outside this arena, so a string is copied rather
    /// than referenced. A task is not a value the outside can be given at all.
    pub(crate) fn to_cap_value(&self, value: &Value) -> Option<CapValue> {
        Some(match value {
            Value::Unit => CapValue::Unit,
            Value::Bool(v) => CapValue::Bool(*v),
            Value::I64(v) => CapValue::I64(*v),
            Value::F64(v) => CapValue::F64(*v),
            Value::Str(h) => CapValue::Str(self.arena.str(*h).to_string()),
            // A list of strings is copied out of the arena, because an
            // argument vector has to survive leaving it. A list of anything
            // else does not cross: `docs/design/arguments.md` says why the
            // boundary carries argument vectors rather than values.
            Value::List(h) => {
                let mut items = Vec::new();
                for item in self.arena.list(*h) {
                    match item {
                        Value::Str(s) => items.push(self.arena.str(*s).to_string()),
                        _ => return None,
                    }
                }
                CapValue::List(items)
            }
            // A handle means nothing outside this arena, and a task means
            // nothing outside this run.
            Value::Task(_) | Value::Object(_) => return None,
        })
    }

    /// Brings a value in from the broker, allocating any string in the arena.
    fn intern_cap_value(&mut self, value: CapValue) -> Value {
        match value {
            CapValue::Unit => Value::Unit,
            CapValue::Bool(v) => Value::Bool(v),
            CapValue::I64(v) => Value::I64(v),
            CapValue::F64(v) => Value::F64(v),
            CapValue::Str(s) => Value::Str(self.arena.alloc_str(s)),
            CapValue::List(items) => {
                let values = items
                    .into_iter()
                    .map(|item| Value::Str(self.arena.alloc_str(item)))
                    .collect();
                Value::List(self.arena.alloc_list(values))
            }
            // The field order is the one `Exit` is declared with in
            // `sic-types`, because bytecode addresses a field by position and
            // this crate cannot see that declaration. What stops the two from
            // drifting is a test rather than this comment: `an_exit_code_is_an_operand`
            // reads field 0 as an `Int` and adds to it, and
            // `a_failing_program_gives_up_both_its_code_and_its_output` reads
            // field 1 as the text. Swapping them fails both.
            CapValue::Exit { code, output } => {
                let text = Value::Str(self.arena.alloc_str(output));
                Value::Object(self.arena.alloc_object(vec![Value::I64(code), text]))
            }
        }
    }

    fn jump(&mut self, index: usize, offset: i16) {
        let frame = self.tasks[index]
            .frames
            .last_mut()
            .expect("a frame was running");
        frame.pc = (frame.pc as i64 + offset as i64) as u32;
    }

    fn get(&self, task: usize, index: usize) -> Value {
        self.tasks[task]
            .regs
            .get(index)
            .cloned()
            .unwrap_or(Value::Unit)
    }

    fn set(&mut self, task: usize, index: usize, value: Value) {
        if let Some(slot) = self.tasks[task].regs.get_mut(index) {
            *slot = value;
        }
    }

    /// Equality on the values the VM can hold. The verifier has already ruled
    /// out comparing two different types.
    fn values_equal(&self, l: &Value, r: &Value) -> bool {
        match (l, r) {
            (Value::Str(a), Value::Str(b)) => self.arena.str(*a) == self.arena.str(*b),
            _ => l == r,
        }
    }

    // ---- failures ----

    /// Turns a `CALL_CAP` into a request the broker can be asked, and parks the
    /// task on it until the answer arrives.
    ///
    /// This is a method rather than an arm of the dispatch `match` because it
    /// is three jobs in a row - reading the arguments, charging the budget,
    /// recording and suspending - and the `match` is otherwise twenty short
    /// arms that each do one thing to two registers. A fifth of the loop being
    /// one arm is what made the ordering bug in `charge_budget` hard to see:
    /// answering "is a refused call charged?" meant reading seventy lines in
    /// the middle of a four-hundred-line function, past a macro that returns
    /// from the enclosing one.
    ///
    /// `Err` is a task that failed. The caller finishes it; nothing here can,
    /// because ending a task is the loop's business.
    fn begin_capability_call(
        &mut self,
        index: usize,
        inst: Inst,
        pc: u32,
        base: usize,
    ) -> Result<(), FailInfo> {
        let (a, b, c) = (inst.a() as usize, inst.b() as usize, inst.c() as usize);
        let fail = |vm: &Self, kind| vm.fail_info(index, kind, None, None);

        let Some(decl) = self.program.caps.get(b) else {
            return Err(fail(
                self,
                FailKind::Internal("capability index out of range"),
            ));
        };
        let (name, argc) = (decl.name.clone(), decl.params.len());

        let mut args = Vec::with_capacity(argc);
        for i in 0..argc {
            match self.to_cap_value(&self.get(index, base + c + i)) {
                Some(v) => args.push(v),
                None => {
                    return Err(fail(
                        self,
                        FailKind::Internal(
                            "a capability argument is not a value the broker can take",
                        ),
                    ));
                }
            }
        }

        let policy = self.program.policy_at(pc);
        let frame_span = self.tasks[index].frames.last().map(|f| f.span);
        // The span exists before the budget is charged so that the charge is
        // recorded against the call that spent it. It used to be recorded
        // against the enclosing function, where two budgeted sites in one
        // function wrote to one place and a reader could not tell which had
        // spent what.
        let span = self.journal.new_span();
        self.charge_budget(index, pc, &name, span, frame_span)?;

        // Recorded here, where the instruction runs, rather than where the
        // request leaves the VM. Two tasks can be waiting at once, and the
        // journal should show that.
        self.journal.emit_for(
            TaskId(index as u64),
            span,
            frame_span,
            EventKind::CapabilityRequested {
                cap: name.clone(),
                args: digest_values(&args),
                attempt: 1,
            },
        );
        self.tasks[index].state = TaskState::WaitingCap(PendingCap {
            reg: base + a,
            index: b as u32,
            name,
            args,
            attempt: 1,
            attempts: policy.map(|p| p.attempts).unwrap_or(1).max(1),
            timeout_ms: policy.map(|p| p.timeout_ms).unwrap_or(0),
            conversation: policy.map(|p| p.conversation).unwrap_or(0),
            tools: policy.map(|p| p.tools).unwrap_or(0),
            deadline_ms: policy.map(|p| p.deadline_ms).unwrap_or(0),
            pc,
            span,
            parent: frame_span,
        });
        Ok(())
    }

    /// Charges one call against a budgeted site, and refuses the call when
    /// there is nothing left.
    ///
    /// The order matters and used to be the other way round: the site was
    /// charged and a `BudgetConsumed` emitted before anything decided whether
    /// the call could happen, so a site with `budget: 1` reached twice wrote
    /// two of them. The second described a call that never left the VM - no
    /// `CapabilityRequested` followed it and the broker was never asked -
    /// against an event whose own doc comment says "a budgeted call site was
    /// used once". The over-count travelled into checkpoints through `spent`
    /// as well, so it survived a resume.
    ///
    /// A refused call is therefore neither charged nor recorded. The journal is
    /// this runtime's account of itself, and an account should not bill for
    /// something it declined to do.
    fn charge_budget(
        &mut self,
        index: usize,
        pc: u32,
        name: &str,
        span: SpanId,
        parent: Option<SpanId>,
    ) -> Result<(), FailInfo> {
        let Some(budget) = self
            .program
            .policy_at(pc)
            .map(|p| p.budget)
            .filter(|b| *b > 0)
        else {
            return Ok(());
        };
        let used = self.spent.get(&pc).copied().unwrap_or(0) + 1;
        if used > budget {
            return Err(self.fail_info(
                index,
                FailKind::OutOfBudget,
                None,
                Some(format!("`{name}` may run {budget} time(s) in a run")),
            ));
        }
        self.spent.insert(pc, used);

        self.journal.emit_for(
            TaskId(index as u64),
            span,
            parent,
            EventKind::BudgetConsumed {
                kind: "calls".into(),
                amount: 1,
                // The check above is what makes this never saturate.
                remaining: budget.saturating_sub(used) as u64,
            },
        );
        Ok(())
    }

    fn fail_info(
        &self,
        task: usize,
        kind: FailKind,
        value: Option<Value>,
        detail: Option<String>,
    ) -> FailInfo {
        let (func, pc) = match self.tasks.get(task).and_then(|t| t.frames.last()) {
            Some(frame) => (
                self.program
                    .funcs
                    .get(frame.func as usize)
                    .map(|f| f.name.clone())
                    .unwrap_or_default(),
                // The pc has already moved past the instruction that failed.
                frame.pc.saturating_sub(1),
            ),
            None => (
                self.tasks
                    .get(task)
                    .map(|t| t.func_name.clone())
                    .unwrap_or_default(),
                0,
            ),
        };
        FailInfo {
            kind,
            func,
            pc,
            value,
            detail,
        }
    }

    fn fail_now(&self, kind: FailKind) -> Status {
        Status::Failed(FailInfo {
            kind,
            func: String::new(),
            pc: 0,
            value: None,
            detail: None,
        })
    }
}

/// Prefixes a message with the path in the document it is about.
fn at(path: &str, message: &str) -> String {
    if path.is_empty() {
        message.to_string()
    } else {
        format!("{path}: {message}")
    }
}

/// Converting between a live task and its saved form.
fn snapshot_task(task: &Task) -> checkpoint::TaskSnapshot {
    checkpoint::TaskSnapshot {
        state: match &task.state {
            TaskState::Ready => checkpoint::TaskStateSnapshot::Ready,
            TaskState::WaitingCap(pending) => {
                checkpoint::TaskStateSnapshot::WaitingCap(checkpoint::Pending {
                    reg: pending.reg as u32,
                    index: pending.index,
                    cap: pending.name.clone(),
                    args: pending.args.clone(),
                    attempt: pending.attempt,
                    attempts: pending.attempts,
                    timeout_ms: pending.timeout_ms,
                    conversation: pending.conversation,
                    tools: pending.tools,
                    deadline_ms: pending.deadline_ms,
                    pc: pending.pc,
                    span: pending.span.0,
                    parent: pending.parent.map(|s| s.0),
                })
            }
            TaskState::WaitingTask(id) => checkpoint::TaskStateSnapshot::WaitingTask(*id),
            TaskState::Finished(value) => checkpoint::TaskStateSnapshot::Finished(value.clone()),
            TaskState::Taken => checkpoint::TaskStateSnapshot::Taken,
            TaskState::Failed(info) => checkpoint::TaskStateSnapshot::Failed {
                message: info.describe(),
                func: info.func.clone(),
                pc: info.pc,
            },
            TaskState::FailureTaken => checkpoint::TaskStateSnapshot::FailureTaken,
        },
        span: task.span.0,
        func_name: task.func_name.clone(),
        regs: task.regs.clone(),
        frames: task
            .frames
            .iter()
            .map(|f| checkpoint::Frame {
                func: f.func,
                pc: f.pc,
                reg_base: f.reg_base as u32,
                ret_reg: f.ret_reg as u32,
                span: f.span.0,
                parent: f.parent.map(|s| s.0),
            })
            .collect(),
    }
}

fn restore_task(saved: &checkpoint::TaskSnapshot) -> Task {
    Task {
        regs: saved.regs.clone(),
        frames: saved
            .frames
            .iter()
            .map(|f| Frame {
                func: f.func,
                pc: f.pc,
                reg_base: f.reg_base as usize,
                ret_reg: f.ret_reg as usize,
                span: SpanId(f.span),
                parent: f.parent.map(SpanId),
            })
            .collect(),
        state: match &saved.state {
            checkpoint::TaskStateSnapshot::Ready => TaskState::Ready,
            checkpoint::TaskStateSnapshot::WaitingCap(pending) => {
                TaskState::WaitingCap(PendingCap {
                    reg: pending.reg as usize,
                    index: pending.index,
                    name: pending.cap.clone(),
                    args: pending.args.clone(),
                    attempt: pending.attempt,
                    attempts: pending.attempts,
                    timeout_ms: pending.timeout_ms,
                    conversation: pending.conversation,
                    tools: pending.tools,
                    deadline_ms: pending.deadline_ms,
                    pc: pending.pc,
                    span: SpanId(pending.span),
                    parent: pending.parent.map(SpanId),
                })
            }
            checkpoint::TaskStateSnapshot::WaitingTask(id) => TaskState::WaitingTask(*id),
            checkpoint::TaskStateSnapshot::Finished(value) => TaskState::Finished(value.clone()),
            checkpoint::TaskStateSnapshot::Taken => TaskState::Taken,
            checkpoint::TaskStateSnapshot::Failed { message, func, pc } => {
                TaskState::Failed(FailInfo {
                    kind: FailKind::Restored,
                    func: func.clone(),
                    pc: *pc,
                    value: None,
                    detail: Some(message.clone()),
                })
            }
            checkpoint::TaskStateSnapshot::FailureTaken => TaskState::FailureTaken,
        },
        span: SpanId(saved.span),
        func_name: saved.func_name.clone(),
    }
}

#[cfg(test)]
mod tests;

/// What is left of a call site's tool allowance.
///
/// Zero means no limit, here as everywhere else in the policy table, so a site
/// that has used its whole allowance is sent 1 rather than 0 - and the broker
/// refusing the next one is what the bound does.
fn tools_left(used: &std::collections::HashMap<u32, u32>, pending: &PendingCap) -> u32 {
    if pending.tools == 0 {
        return 0;
    }
    let spent = used.get(&pending.pc).copied().unwrap_or(0);
    pending.tools.saturating_sub(spent).max(1)
}
