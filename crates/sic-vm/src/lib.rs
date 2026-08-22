//! The register VM.
//!
//! The VM knows nothing about the outside world: no files, no clock, no network,
//! no processes. Everything it can do is decide what the next instruction is and
//! what it does to registers. That is what makes a run reproducible, and it is
//! what will let external effects arrive as capabilities without the VM ever
//! holding a credential.
//!
//! It expects verified bytecode. Where the verifier has already established a
//! property, the VM does not re-check it; where an index could still be out of
//! range because a caller skipped verification, it fails the run rather than
//! panicking.

pub mod checkpoint;
pub mod value;

use sic_bytecode::inst::Op;
use sic_bytecode::program::{Const, Program};
use sic_core::{CapError, CapRequest, CapValue, Digest};
use sic_journal::{EventKind, Journal, RunId, SpanId, digest_values};

pub use checkpoint::{Checkpoint, CheckpointError};
pub use value::{Arena, Handle, Value};

/// Where a run ended up.
#[derive(Debug, Clone)]
pub enum Status {
    Finished(Value),
    Failed(FailInfo),
    /// The VM stopped because it needs an effect it cannot perform. The driver
    /// asks the broker and calls `resume`.
    ///
    /// Suspending, rather than calling out through a trait, is what keeps this
    /// crate unable to reach the outside world at all. It is also exactly the
    /// point phase 5 has to checkpoint: everything needed to continue is in
    /// `Vm`.
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
            FailKind::Internal(what) => what,
        }
    }
}

/// One activation record. Registers live in the shared stack, so a frame only
/// records where its window starts.
#[derive(Debug, Clone)]
struct Frame {
    func: u32,
    pc: u32,
    reg_base: usize,
    /// Absolute register in the caller that receives the return value.
    ret_reg: usize,
    /// The span this activation is, and the span it happened inside. Recording
    /// both as the frame is pushed is what gives the journal a trace shape
    /// without reconstructing one afterwards.
    span: SpanId,
    parent: Option<SpanId>,
}

/// A capability call the VM is waiting on.
#[derive(Debug, Clone)]
struct PendingCap {
    /// Absolute register the result goes into.
    reg: usize,
    name: String,
    span: SpanId,
    parent: Option<SpanId>,
}

/// Limits that keep a runaway program from exhausting the host.
const MAX_FRAMES: usize = 1024;
const MAX_REGS: usize = 1 << 16;
/// The default instruction budget, high enough for real work and low enough
/// that a non-terminating program stops on its own.
pub const DEFAULT_FUEL: u64 = 10_000_000;

pub struct Vm<'a> {
    program: &'a Program,
    regs: Vec<Value>,
    frames: Vec<Frame>,
    arena: Arena,
    /// Handles for the string constants, allocated once at startup rather than
    /// on every load.
    str_consts: Vec<Option<Handle>>,
    /// The capability call the VM is waiting on. `Some` exactly while the VM
    /// is suspended.
    pending: Option<PendingCap>,
    /// Every run produces events, whether or not anything is listening: the
    /// journal is the runtime's own account of what happened, not
    /// instrumentation a program has to add.
    journal: Journal,
    /// The span of the run itself, which every function span sits inside.
    root_span: SpanId,
    fuel: u64,
}

impl std::fmt::Debug for Vm<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vm")
            .field("frames", &self.frames.len())
            .field("regs", &self.regs.len())
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
                _ => None,
            })
            .collect();
        Self {
            program,
            regs: Vec::new(),
            frames: Vec::new(),
            arena,
            str_consts,
            pending: None,
            journal,
            root_span: SpanId(0),
            fuel,
        }
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// How much fuel is left, which is also how many instructions have run.
    pub fn fuel(&self) -> u64 {
        self.fuel
    }

    /// Renders a value for a human.
    pub fn display(&self, value: &Value) -> String {
        value.display(&self.arena)
    }

    /// Continues a suspended run with the value the capability produced.
    pub fn resume(&mut self, value: CapValue) -> Status {
        let Some(pending) = self.pending.take() else {
            return self.fail_now(FailKind::Internal("resumed while not suspended"));
        };
        self.journal.emit(
            pending.span,
            pending.parent,
            EventKind::CapabilityCompleted {
                cap: pending.name,
                result: digest_values(std::slice::from_ref(&value)),
            },
        );
        let value = self.intern_cap_value(value);
        self.set(pending.reg, value);
        let status = self.execute();
        self.record_end(&status);
        status
    }

    /// Ends a suspended run because the capability call did not succeed.
    ///
    /// Retrying is a workflow decision, and the IR already has the slot for it,
    /// so nothing here tries to be clever about recovery.
    pub fn resume_failed(&mut self, error: &CapError) -> Status {
        if let Some(pending) = self.pending.take() {
            self.journal.emit(
                pending.span,
                pending.parent,
                EventKind::CapabilityFailed {
                    cap: pending.name,
                    error: error.message.clone(),
                },
            );
        }
        let status = self.fail_with(FailKind::Capability, None, Some(error.message.clone()));
        self.record_end(&status);
        status
    }

    /// Whether the VM is waiting for a capability result.
    pub fn is_suspended(&self) -> bool {
        self.pending.is_some()
    }

    /// The capability the VM is waiting on, if it is waiting.
    pub fn pending_capability(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.name.as_str())
    }

    /// Writes out a suspended run, so it can continue later or elsewhere.
    ///
    /// Returns `None` when the VM is not suspended: there is no such thing as
    /// checkpointing a run in the middle of an instruction, and there is no
    /// need for one, because a run that is not waiting can simply keep going.
    pub fn checkpoint(&mut self, program_digest: Digest, question: &str) -> Option<Vec<u8>> {
        let pending = self.pending.clone()?;
        self.journal.emit(
            self.root_span,
            None,
            EventKind::RunSuspended {
                cap: pending.name.clone(),
            },
        );

        // The `checkpoint_written` event below consumes the next sequence
        // number, so what is saved is the one after it. A resumed run continues
        // the same sequence, and must not reuse a number.
        let seq = self.journal.seq() + 1;

        let saved = Checkpoint {
            program_digest,
            run: self.journal.run_id().0,
            seq,
            next_span: self.journal.next_span_id(),
            fuel: self.fuel,
            pending: checkpoint::Pending {
                reg: pending.reg as u32,
                cap: pending.name.clone(),
                span: pending.span.0,
                parent: pending.parent.map(|s| s.0),
                question: question.to_string(),
            },
            frames: self
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
            regs: self.regs.clone(),
            str_consts: self.str_consts.iter().map(|h| h.map(|h| h.0)).collect(),
            strings: self.arena.strings().to_vec(),
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

        for (i, frame) in saved.frames.iter().enumerate() {
            let Some(def) = program.funcs.get(frame.func as usize) else {
                return Err(CheckpointError::new(format!(
                    "frame {i} names function {}, which this program does not have",
                    frame.func
                )));
            };
            if !def.contains_pc(frame.pc) {
                return Err(CheckpointError::new(format!(
                    "frame {i} is at instruction {} which is outside `{}`",
                    frame.pc, def.name
                )));
            }
        }
        if saved.str_consts.len() != program.consts.len() {
            return Err(CheckpointError::new(
                "the checkpoint's constants do not match this program's",
            ));
        }

        let question = saved.pending.question.clone();
        let journal = Journal::resumed(RunId(saved.run), saved.seq, saved.next_span, journal_sink);

        let mut vm = Self {
            program,
            regs: saved.regs,
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
            arena: Arena::from_strings(saved.strings),
            str_consts: saved.str_consts.iter().map(|h| h.map(Handle)).collect(),
            pending: Some(PendingCap {
                reg: saved.pending.reg as usize,
                name: saved.pending.cap.clone(),
                span: SpanId(saved.pending.span),
                parent: saved.pending.parent.map(SpanId),
            }),
            journal,
            // The run span is the one the entry frame sits inside.
            root_span: saved
                .frames
                .first()
                .and_then(|f| f.parent)
                .map(SpanId)
                .unwrap_or(SpanId(0)),
            fuel: saved.fuel,
        };
        vm.journal.emit(
            vm.root_span,
            None,
            EventKind::RunResumed {
                cap: saved.pending.cap,
            },
        );
        Ok((vm, question))
    }

    /// Calls a function and runs until it returns, fails, or runs out of fuel.
    pub fn run(&mut self, func: u32, args: &[Value]) -> Status {
        let Some(def) = self.program.funcs.get(func as usize) else {
            return self.fail_now(FailKind::Internal("no such function"));
        };
        if args.len() != def.param_count() {
            return self.fail_now(FailKind::Internal("wrong number of arguments"));
        }

        let name = def.name.clone();
        let (reg_count, code_off) = (def.reg_count as usize, def.code_off);

        // Register 0 of the outermost frame receives the final result.
        self.regs = vec![Value::Unit; reg_count + 1];
        for (i, arg) in args.iter().enumerate() {
            self.regs[1 + i] = arg.clone();
        }

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

        let span = self.journal.new_span();
        self.journal.emit(
            span,
            Some(self.root_span),
            EventKind::FunctionEntered { func: name },
        );
        self.frames.push(Frame {
            func,
            pc: code_off,
            reg_base: 1,
            ret_reg: 0,
            span,
            parent: Some(self.root_span),
        });

        let status = self.execute();
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
                let mut error = info.kind.message().to_string();
                if let Some(detail) = &info.detail {
                    error.push_str(": ");
                    error.push_str(detail);
                }
                self.journal
                    .emit(root, None, EventKind::RunFailed { error });
            }
            Status::Suspended(_) => {}
        }
    }

    fn execute(&mut self) -> Status {
        loop {
            if self.fuel == 0 {
                return self.fail(FailKind::OutOfFuel, None);
            }
            self.fuel -= 1;

            let Some(frame) = self.frames.last() else {
                return self.fail_now(FailKind::Internal("no frame to run"));
            };
            let (pc, base) = (frame.pc, frame.reg_base);
            let Some(inst) = self.program.code.get(pc as usize).copied() else {
                return self.fail(FailKind::Internal("pc is outside the code"), None);
            };
            let Some(op) = inst.op() else {
                return self.fail(FailKind::Internal("unknown opcode"), None);
            };
            self.frames.last_mut().expect("checked above").pc = pc + 1;

            let (a, b, c) = (inst.a() as usize, inst.b() as usize, inst.c() as usize);

            match op {
                Op::LoadConst => {
                    let Some(konst) = self.program.consts.get(inst.bx() as usize) else {
                        return self.fail(FailKind::Internal("constant index out of range"), None);
                    };
                    let value = match konst {
                        Const::Unit => Value::Unit,
                        Const::Bool(v) => Value::Bool(*v),
                        Const::I64(v) => Value::I64(*v),
                        Const::F64(v) => Value::F64(*v),
                        Const::Str(_) => match self.str_consts[inst.bx() as usize] {
                            Some(h) => Value::Str(h),
                            None => return self.fail(FailKind::Internal("missing string"), None),
                        },
                    };
                    self.set(base + a, value);
                }
                Op::Move => {
                    let v = self.get(base + b);
                    self.set(base + a, v);
                }
                Op::AddI64 | Op::SubI64 | Op::MulI64 | Op::DivI64 | Op::RemI64 => {
                    let (Value::I64(l), Value::I64(r)) = (self.get(base + b), self.get(base + c))
                    else {
                        return self.fail(FailKind::Internal("arithmetic on a non-integer"), None);
                    };
                    let result = match op {
                        Op::AddI64 => l.checked_add(r),
                        Op::SubI64 => l.checked_sub(r),
                        Op::MulI64 => l.checked_mul(r),
                        Op::DivI64 => {
                            if r == 0 {
                                return self.fail(FailKind::DivisionByZero, None);
                            }
                            l.checked_div(r)
                        }
                        _ => {
                            if r == 0 {
                                return self.fail(FailKind::DivisionByZero, None);
                            }
                            l.checked_rem(r)
                        }
                    };
                    // `None` here is i64::MIN / -1 as well as plain overflow.
                    let Some(value) = result else {
                        return self.fail(FailKind::Overflow, None);
                    };
                    self.set(base + a, Value::I64(value));
                }
                Op::Eq | Op::Ne => {
                    let (l, r) = (self.get(base + b), self.get(base + c));
                    let equal = self.values_equal(&l, &r);
                    self.set(
                        base + a,
                        Value::Bool(if op == Op::Eq { equal } else { !equal }),
                    );
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    let (Value::I64(l), Value::I64(r)) = (self.get(base + b), self.get(base + c))
                    else {
                        return self.fail(FailKind::Internal("comparison on a non-integer"), None);
                    };
                    let result = match op {
                        Op::Lt => l < r,
                        Op::Le => l <= r,
                        Op::Gt => l > r,
                        _ => l >= r,
                    };
                    self.set(base + a, Value::Bool(result));
                }
                Op::Not => {
                    let Value::Bool(v) = self.get(base + b) else {
                        return self.fail(FailKind::Internal("`not` on a non-boolean"), None);
                    };
                    self.set(base + a, Value::Bool(!v));
                }
                Op::Jump => self.jump(inst.sbx()),
                Op::JumpIf | Op::JumpIfNot => {
                    let Value::Bool(cond) = self.get(base + a) else {
                        return self.fail(FailKind::Internal("branch on a non-boolean"), None);
                    };
                    if cond == (op == Op::JumpIf) {
                        self.jump(inst.sbx());
                    }
                }
                Op::Call => {
                    if let Some(status) = self.call(b as u32, base + c, base + a) {
                        return status;
                    }
                }
                Op::CallCap => {
                    let Some(decl) = self.program.caps.get(b) else {
                        return self
                            .fail(FailKind::Internal("capability index out of range"), None);
                    };
                    let (name, argc) = (decl.name.clone(), decl.params.len());
                    let mut args = Vec::with_capacity(argc);
                    for i in 0..argc {
                        match self.to_cap_value(&self.get(base + c + i)) {
                            Some(v) => args.push(v),
                            None => {
                                return self.fail(
                                    FailKind::Internal(
                                        "a capability argument is not a value the broker can take",
                                    ),
                                    None,
                                );
                            }
                        }
                    }
                    let frame_span = self.frames.last().map(|f| f.span);
                    let span = self.journal.new_span();
                    self.journal.emit(
                        span,
                        frame_span,
                        EventKind::CapabilityRequested {
                            cap: name.clone(),
                            args: digest_values(&args),
                        },
                    );
                    // The result lands here once the driver comes back.
                    self.pending = Some(PendingCap {
                        reg: base + a,
                        name: name.clone(),
                        span,
                        parent: frame_span,
                    });
                    return Status::Suspended(CapRequest {
                        index: b as u32,
                        name,
                        args,
                    });
                }
                Op::Return => {
                    let value = self.get(base + a);
                    let frame = self.frames.pop().expect("a frame was running");
                    let func_name = self
                        .program
                        .funcs
                        .get(frame.func as usize)
                        .map(|f| f.name.clone())
                        .unwrap_or_default();
                    self.journal.emit(
                        frame.span,
                        frame.parent,
                        EventKind::FunctionExited { func: func_name },
                    );
                    self.regs.truncate(frame.reg_base);
                    if self.frames.is_empty() {
                        return Status::Finished(value);
                    }
                    self.set(frame.ret_reg, value);
                }
                Op::Fail => {
                    let value = self.get(base + a);
                    return self.fail(FailKind::Explicit, Some(value));
                }
                Op::Halt => {
                    return Status::Finished(Value::Unit);
                }
            }
        }
    }

    /// Pushes a frame for `func`, copying the arguments into it. Returns a
    /// status only when the call could not be made.
    fn call(&mut self, func: u32, arg_base: usize, ret_reg: usize) -> Option<Status> {
        let Some(def) = self.program.funcs.get(func as usize) else {
            return Some(self.fail(
                FailKind::Internal("call to a function that does not exist"),
                None,
            ));
        };
        let (argc, reg_count, code_off, name) = (
            def.param_count(),
            def.reg_count as usize,
            def.code_off,
            def.name.clone(),
        );

        if self.frames.len() >= MAX_FRAMES {
            return Some(self.fail(FailKind::CallStackTooDeep, None));
        }
        let new_base = self.regs.len();
        if new_base + reg_count > MAX_REGS {
            return Some(self.fail(FailKind::CallStackTooDeep, None));
        }

        // The callee's window sits above every register in use, so copying the
        // arguments cannot overwrite one of them.
        self.regs.resize(new_base + reg_count, Value::Unit);
        for i in 0..argc {
            let arg = self.get(arg_base + i);
            self.regs[new_base + i] = arg;
        }
        let parent = self.frames.last().map(|f| f.span);
        let span = self.journal.new_span();
        self.journal
            .emit(span, parent, EventKind::FunctionEntered { func: name });
        self.frames.push(Frame {
            func,
            pc: code_off,
            reg_base: new_base,
            ret_reg,
            span,
            parent,
        });
        None
    }

    /// Copies a value out of the VM so it can cross the broker boundary.
    ///
    /// Handles are meaningless outside this arena, so a string is copied rather
    /// than referenced. Lists and objects have no representation yet.
    fn to_cap_value(&self, value: &Value) -> Option<CapValue> {
        Some(match value {
            Value::Unit => CapValue::Unit,
            Value::Bool(v) => CapValue::Bool(*v),
            Value::I64(v) => CapValue::I64(*v),
            Value::F64(v) => CapValue::F64(*v),
            Value::Str(h) => CapValue::Str(self.arena.str(*h).to_string()),
            Value::List(_) | Value::Object(_) => return None,
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
        }
    }

    fn jump(&mut self, offset: i16) {
        let frame = self.frames.last_mut().expect("a frame was running");
        frame.pc = (frame.pc as i64 + offset as i64) as u32;
    }

    fn get(&self, index: usize) -> Value {
        self.regs.get(index).cloned().unwrap_or(Value::Unit)
    }

    fn set(&mut self, index: usize, value: Value) {
        if let Some(slot) = self.regs.get_mut(index) {
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

    fn fail(&self, kind: FailKind, value: Option<Value>) -> Status {
        self.fail_with(kind, value, None)
    }

    fn fail_with(&self, kind: FailKind, value: Option<Value>, detail: Option<String>) -> Status {
        let (func, pc) = match self.frames.last() {
            Some(frame) => (
                self.program
                    .funcs
                    .get(frame.func as usize)
                    .map(|f| f.name.clone())
                    .unwrap_or_default(),
                // The pc has already moved past the instruction that failed.
                frame.pc.saturating_sub(1),
            ),
            None => (String::new(), 0),
        };
        Status::Failed(FailInfo {
            kind,
            func,
            pc,
            value,
            detail,
        })
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

#[cfg(test)]
mod tests;
