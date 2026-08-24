//! Checkpoints: the state of a suspended run, written out and read back.
//!
//! A run stops at a capability that cannot answer yet, and everything needed to
//! continue is already in the VM - the tasks, their registers and call stacks,
//! the arena, the outstanding call, and where the journal had got to. Durable
//! execution is therefore writing out state that exists, not a second mechanism
//! beside a synchronous call. That is why the VM suspends instead of calling the
//! broker.
//!
//! **A checkpoint holds values, and the journal does not.** They are different
//! things: the journal is an account of a run that leaves the process, so it
//! records digests; a checkpoint is the run itself, so it has to hold the
//! registers and the arena as they are. Protecting a checkpoint at rest is a
//! separate problem, and not one that recording less would solve.
//!
//! A checkpoint is read back with the same suspicion as bytecode. It comes from
//! a file, so it can be truncated, corrupt or hostile, and a VM restored from
//! one must not start out with its invariants already broken.

use sic_core::bin::{Reader, Writer};
use sic_core::{CapValue, Digest, Sha256};

use crate::value::{Handle, Value};

pub const MAGIC: [u8; 4] = *b"SICC";
pub const VERSION_MAJOR: u16 = 0;
/// Bumped from 1 for tasks: a phase 5 checkpoint holds one implicit task and
/// cannot be read as this. Refusing it is better than half-understanding it.
/// Bumped from 2 for argument vectors, which a suspended call may carry.
/// Bumped from 3 for conversations: a suspended call says which one it belongs
/// to, and a reader that stopped after the timeout would take that for a span.
/// Bumped from 4 for an agent's tool allowance and answer deadline, and for the
/// call site they belong to.
pub const VERSION_MINOR: u16 = 5;

pub type CheckpointError = sic_core::BinError;

type Result<T> = std::result::Result<T, CheckpointError>;

/// The state of a suspended run.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    /// The digest of the bytecode this run belongs to. Resuming against
    /// anything else would continue one program inside another.
    pub program_digest: Digest,
    pub run: u128,
    /// Where the journal had got to, so that a resumed run continues one
    /// sequence rather than starting a second.
    pub seq: u64,
    pub next_span: u64,
    pub root_span: u64,
    pub fuel: u64,
    /// Where round-robin scheduling resumes, so a resumed run schedules the
    /// same way an uninterrupted one would.
    pub cursor: u32,
    /// The task whose capability call is outstanding.
    pub answering: u32,
    /// What is being waited for, for whoever has to answer it. This is a value,
    /// which is why it belongs in a checkpoint and not in the journal.
    pub question: String,
    pub tasks: Vec<TaskSnapshot>,
    /// Handles of the string constants, which are allocated before the run and
    /// so cannot be rebuilt without duplicating them in the arena.
    pub str_consts: Vec<Option<u32>>,
    pub strings: Vec<String>,
    pub lists: Vec<Vec<Value>>,
    pub objects: Vec<Vec<Value>>,
    /// How many times each capability call site has run, for the budgets in the
    /// policy table.
    pub spent: Vec<(u32, u32)>,
    /// How many of the agent's own tools each call site has used. Travels for
    /// the same reason `spent` does: a resumed run must not get a fresh
    /// allowance.
    pub used_tools: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSnapshot {
    pub state: TaskStateSnapshot,
    pub span: u64,
    pub func_name: String,
    pub regs: Vec<Value>,
    pub frames: Vec<Frame>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStateSnapshot {
    Ready,
    WaitingCap(Pending),
    WaitingTask(u32),
    Finished(Value),
    Taken,
    /// A failure is stored as the text it reports. The kind matters to the
    /// program only through that text, and a restored run reports the same
    /// thing it would have.
    Failed {
        message: String,
        func: String,
        pc: u32,
    },
    FailureTaken,
}

/// The capability call a task is waiting on.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// Register in the task the answer goes into.
    pub reg: u32,
    pub index: u32,
    pub cap: String,
    pub args: Vec<CapValue>,
    pub attempt: u32,
    pub attempts: u32,
    pub timeout_ms: u32,
    /// Which conversation the call belongs to. A resumed run that retried this
    /// call would otherwise start a new one, which is the opposite of what
    /// remembering means.
    pub conversation: u32,
    /// The site's tool allowance, its answer deadline, and which site it is -
    /// so that what the agent used is charged to the site that allowed it after
    /// a resume as well as before one.
    pub tools: u32,
    pub deadline_ms: u32,
    pub pc: u32,
    pub span: u64,
    pub parent: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub func: u32,
    pub pc: u32,
    pub reg_base: u32,
    pub ret_reg: u32,
    pub span: u64,
    pub parent: Option<u64>,
}

impl Checkpoint {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&MAGIC);
        w.u16(VERSION_MAJOR);
        w.u16(VERSION_MINOR);
        w.bytes(self.program_digest.bytes());
        w.u128(self.run);
        w.u64(self.seq);
        w.u64(self.next_span);
        w.u64(self.root_span);
        w.u64(self.fuel);
        w.u32(self.cursor);
        w.u32(self.answering);
        w.str(&self.question);

        w.u32(self.tasks.len() as u32);
        for task in &self.tasks {
            write_state(&mut w, &task.state);
            w.u64(task.span);
            w.str(&task.func_name);
            w.u32(task.regs.len() as u32);
            for value in &task.regs {
                write_value(&mut w, value);
            }
            w.u32(task.frames.len() as u32);
            for frame in &task.frames {
                w.u32(frame.func);
                w.u32(frame.pc);
                w.u32(frame.reg_base);
                w.u32(frame.ret_reg);
                w.u64(frame.span);
                write_option_u64(&mut w, frame.parent);
            }
        }

        w.u32(self.str_consts.len() as u32);
        for handle in &self.str_consts {
            match handle {
                Some(h) => {
                    w.bool(true);
                    w.u32(*h);
                }
                None => w.bool(false),
            }
        }

        w.u32(self.strings.len() as u32);
        for s in &self.strings {
            w.str(s);
        }
        write_value_lists(&mut w, &self.lists);
        write_value_lists(&mut w, &self.objects);
        w.u32(self.used_tools.len() as u32);
        for (pc, count) in &self.used_tools {
            w.u32(*pc);
            w.u32(*count);
        }
        w.u32(self.spent.len() as u32);
        for (pc, count) in &self.spent {
            w.u32(*pc);
            w.u32(*count);
        }
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Checkpoint> {
        let mut r = Reader::new(bytes);
        if r.take(4)? != MAGIC {
            return Err(CheckpointError::new("not a sic checkpoint (bad magic)"));
        }
        let (major, minor) = (r.u16()?, r.u16()?);
        if (major, minor) != (VERSION_MAJOR, VERSION_MINOR) {
            return Err(CheckpointError::new(format!(
                "unsupported checkpoint version {major}.{minor}, expected {VERSION_MAJOR}.{VERSION_MINOR}"
            )));
        }

        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(r.take(32)?);
        let program_digest = Digest::from_bytes(digest_bytes);

        let run = r.u128()?;
        let seq = r.u64()?;
        let next_span = r.u64()?;
        let root_span = r.u64()?;
        let fuel = r.u64()?;
        let cursor = r.u32()?;
        let answering = r.u32()?;
        let question = r.str()?;

        let task_count = r.count(16)?;
        let mut tasks = Vec::with_capacity(task_count);
        for _ in 0..task_count {
            let state = read_state(&mut r)?;
            let span = r.u64()?;
            let func_name = r.str()?;

            let reg_count = r.count(1)?;
            let mut regs = Vec::with_capacity(reg_count);
            for _ in 0..reg_count {
                regs.push(read_value(&mut r)?);
            }

            let frame_count = r.count(28)?;
            let mut frames = Vec::with_capacity(frame_count);
            for _ in 0..frame_count {
                frames.push(Frame {
                    func: r.u32()?,
                    pc: r.u32()?,
                    reg_base: r.u32()?,
                    ret_reg: r.u32()?,
                    span: r.u64()?,
                    parent: read_option_u64(&mut r)?,
                });
            }
            tasks.push(TaskSnapshot {
                state,
                span,
                func_name,
                regs,
                frames,
            });
        }

        let const_count = r.count(1)?;
        let mut str_consts = Vec::with_capacity(const_count);
        for _ in 0..const_count {
            str_consts.push(if r.bool()? { Some(r.u32()?) } else { None });
        }

        let string_count = r.count(4)?;
        let mut strings = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            strings.push(r.str()?);
        }
        let lists = read_value_lists(&mut r)?;
        let objects = read_value_lists(&mut r)?;
        let used_count = r.count(8)?;
        let mut used_tools = Vec::with_capacity(used_count);
        for _ in 0..used_count {
            used_tools.push((r.u32()?, r.u32()?));
        }
        let spent_count = r.count(8)?;
        let mut spent = Vec::with_capacity(spent_count);
        for _ in 0..spent_count {
            spent.push((r.u32()?, r.u32()?));
        }

        r.expect_end("the checkpoint")?;

        let checkpoint = Checkpoint {
            program_digest,
            run,
            seq,
            next_span,
            root_span,
            fuel,
            cursor,
            answering,
            question,
            tasks,
            str_consts,
            strings,
            lists,
            objects,
            spent,
            used_tools,
        };
        checkpoint.check_consistency()?;
        Ok(checkpoint)
    }

    /// Checks what the VM would otherwise have to assume.
    ///
    /// This is the same contract the bytecode verifier has: a restored VM skips
    /// checks only because they happened here. Everything a hostile file could
    /// point somewhere wrong is checked against the state it points into.
    fn check_consistency(&self) -> Result<()> {
        if self.tasks.is_empty() {
            return Err(CheckpointError::new("a run has at least one task"));
        }
        let task_count = self.tasks.len() as u32;
        if self.answering >= task_count {
            return Err(CheckpointError::new(
                "the outstanding call belongs to a task that does not exist",
            ));
        }
        if !matches!(
            self.tasks[self.answering as usize].state,
            TaskStateSnapshot::WaitingCap(_)
        ) {
            return Err(CheckpointError::new(
                "the task with the outstanding call is not waiting for one",
            ));
        }
        if self.cursor >= task_count {
            return Err(CheckpointError::new(
                "the scheduler cursor is outside the task list",
            ));
        }

        // A handle only means anything against its own store, so each kind is
        // checked against the store it points into.
        let stores = Stores {
            strings: self.strings.len() as u32,
            lists: self.lists.len() as u32,
            objects: self.objects.len() as u32,
        };
        for values in self.lists.iter().chain(self.objects.iter()) {
            for value in values {
                check_value(value, stores, task_count)?;
            }
        }
        for (index, task) in self.tasks.iter().enumerate() {
            let regs = task.regs.len() as u32;
            if let TaskStateSnapshot::WaitingCap(pending) = &task.state {
                if pending.reg >= regs {
                    return Err(CheckpointError::new(format!(
                        "task {index} answers into a register that does not exist"
                    )));
                }
            }
            if let TaskStateSnapshot::WaitingTask(waited) = task.state {
                if waited >= task_count {
                    return Err(CheckpointError::new(format!(
                        "task {index} waits for task {waited}, which does not exist"
                    )));
                }
            }
            // A running task has frames; a finished one does not.
            let running = matches!(
                task.state,
                TaskStateSnapshot::Ready
                    | TaskStateSnapshot::WaitingCap(_)
                    | TaskStateSnapshot::WaitingTask(_)
            );
            if running && task.frames.is_empty() {
                return Err(CheckpointError::new(format!(
                    "task {index} is runnable but has no frames"
                )));
            }
            for (i, frame) in task.frames.iter().enumerate() {
                if frame.reg_base > regs || frame.ret_reg >= regs.max(1) {
                    return Err(CheckpointError::new(format!(
                        "frame {i} of task {index} refers to registers outside the saved stack"
                    )));
                }
            }
            // Frames sit one above another in a single register stack, and the
            // arithmetic that finds a register assumes that ordering.
            for pair in task.frames.windows(2) {
                if pair[1].reg_base < pair[0].reg_base {
                    return Err(CheckpointError::new(format!(
                        "the frames of task {index} are not in order of their register windows"
                    )));
                }
            }
            for value in &task.regs {
                check_value(value, stores, task_count)?;
            }
            if let TaskStateSnapshot::Finished(value) = &task.state {
                check_value(value, stores, task_count)?;
            }
        }

        for handle in self.str_consts.iter().flatten() {
            // A constant handle is a string or an empty list; both stores have
            // to be able to hold it.
            if *handle >= stores.strings.max(stores.lists) {
                return Err(CheckpointError::new(
                    "a string constant points outside the saved arena",
                ));
            }
        }
        Ok(())
    }

    /// The digest of the encoded checkpoint, which names it in the journal.
    pub fn digest(bytes: &[u8]) -> Digest {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finish()
    }
}

/// How big each of the arena's stores is.
#[derive(Debug, Clone, Copy)]
struct Stores {
    strings: u32,
    lists: u32,
    objects: u32,
}

fn check_value(value: &Value, stores: Stores, tasks: u32) -> Result<()> {
    let out_of_range = match value {
        Value::Str(h) => h.0 >= stores.strings,
        Value::List(h) => h.0 >= stores.lists,
        Value::Object(h) => h.0 >= stores.objects,
        Value::Task(id) if *id >= tasks => {
            return Err(CheckpointError::new(
                "a saved value names a task that does not exist",
            ));
        }
        _ => false,
    };
    if out_of_range {
        return Err(CheckpointError::new(
            "a saved value points outside the saved arena",
        ));
    }
    Ok(())
}

fn write_value_lists(w: &mut Writer, values: &[Vec<Value>]) {
    w.u32(values.len() as u32);
    for group in values {
        w.u32(group.len() as u32);
        for value in group {
            write_value(w, value);
        }
    }
}

fn read_value_lists(r: &mut Reader<'_>) -> Result<Vec<Vec<Value>>> {
    let count = r.count(4)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.count(1)?;
        let mut group = Vec::with_capacity(len);
        for _ in 0..len {
            group.push(read_value(r)?);
        }
        out.push(group);
    }
    Ok(out)
}

fn write_state(w: &mut Writer, state: &TaskStateSnapshot) {
    match state {
        TaskStateSnapshot::Ready => w.u8(0),
        TaskStateSnapshot::WaitingCap(pending) => {
            w.u8(1);
            w.u32(pending.reg);
            w.u32(pending.index);
            w.str(&pending.cap);
            w.u32(pending.args.len() as u32);
            for arg in &pending.args {
                arg.write(w);
            }
            w.u32(pending.attempt);
            w.u32(pending.attempts);
            w.u32(pending.timeout_ms);
            w.u32(pending.conversation);
            w.u32(pending.tools);
            w.u32(pending.deadline_ms);
            w.u32(pending.pc);
            w.u64(pending.span);
            write_option_u64(w, pending.parent);
        }
        TaskStateSnapshot::WaitingTask(id) => {
            w.u8(2);
            w.u32(*id);
        }
        TaskStateSnapshot::Finished(value) => {
            w.u8(3);
            write_value(w, value);
        }
        TaskStateSnapshot::Taken => w.u8(4),
        TaskStateSnapshot::Failed { message, func, pc } => {
            w.u8(5);
            w.str(message);
            w.str(func);
            w.u32(*pc);
        }
        TaskStateSnapshot::FailureTaken => w.u8(6),
    }
}

fn read_state(r: &mut Reader<'_>) -> Result<TaskStateSnapshot> {
    Ok(match r.u8()? {
        0 => TaskStateSnapshot::Ready,
        1 => {
            let reg = r.u32()?;
            let index = r.u32()?;
            let cap = r.str()?;
            let arg_count = r.count(1)?;
            let mut args = Vec::with_capacity(arg_count);
            for _ in 0..arg_count {
                args.push(CapValue::read(r)?);
            }
            TaskStateSnapshot::WaitingCap(Pending {
                reg,
                index,
                cap,
                args,
                attempt: r.u32()?,
                attempts: r.u32()?,
                timeout_ms: r.u32()?,
                conversation: r.u32()?,
                tools: r.u32()?,
                deadline_ms: r.u32()?,
                pc: r.u32()?,
                span: r.u64()?,
                parent: read_option_u64(r)?,
            })
        }
        2 => TaskStateSnapshot::WaitingTask(r.u32()?),
        3 => TaskStateSnapshot::Finished(read_value(r)?),
        4 => TaskStateSnapshot::Taken,
        5 => TaskStateSnapshot::Failed {
            message: r.str()?,
            func: r.str()?,
            pc: r.u32()?,
        },
        6 => TaskStateSnapshot::FailureTaken,
        other => {
            return Err(CheckpointError::new(format!(
                "unknown task state {other} in a checkpoint"
            )));
        }
    })
}

fn write_option_u64(w: &mut Writer, value: Option<u64>) {
    match value {
        Some(v) => {
            w.bool(true);
            w.u64(v);
        }
        None => w.bool(false),
    }
}

fn read_option_u64(r: &mut Reader<'_>) -> Result<Option<u64>> {
    Ok(if r.bool()? { Some(r.u64()?) } else { None })
}

fn write_value(w: &mut Writer, value: &Value) {
    match value {
        Value::Unit => w.u8(0),
        Value::Bool(v) => {
            w.u8(1);
            w.bool(*v);
        }
        Value::I64(v) => {
            w.u8(2);
            w.i64(*v);
        }
        Value::F64(v) => {
            w.u8(3);
            w.f64(*v);
        }
        Value::Str(h) => {
            w.u8(4);
            w.u32(h.0);
        }
        Value::List(h) => {
            w.u8(5);
            w.u32(h.0);
        }
        Value::Object(h) => {
            w.u8(6);
            w.u32(h.0);
        }
        Value::Task(id) => {
            w.u8(7);
            w.u32(*id);
        }
    }
}

fn read_value(r: &mut Reader<'_>) -> Result<Value> {
    Ok(match r.u8()? {
        0 => Value::Unit,
        1 => Value::Bool(r.bool()?),
        2 => Value::I64(r.i64()?),
        3 => Value::F64(r.f64()?),
        4 => Value::Str(Handle(r.u32()?)),
        5 => Value::List(Handle(r.u32()?)),
        6 => Value::Object(Handle(r.u32()?)),
        7 => Value::Task(r.u32()?),
        other => {
            return Err(CheckpointError::new(format!(
                "unknown value tag {other} in a checkpoint"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing that suspends takes an argument vector yet, so this pair is not
    /// reachable through a run. The format still has to hold every `CapValue`,
    /// and a format is checked rather than assumed.
    #[test]
    fn an_argument_vector_survives_the_format() {
        for value in [
            CapValue::List(Vec::new()),
            CapValue::List(vec!["send-keys".into(), "-t".into(), "sic:0".into()]),
            CapValue::List(vec![String::new(), "  ".into(), "\u{3053}".into()]),
        ] {
            let mut w = Writer::new();
            value.write(&mut w);
            let bytes = w.finish();
            let mut r = Reader::new(&bytes);
            assert_eq!(CapValue::read(&mut r).unwrap(), value);
        }
    }
}
