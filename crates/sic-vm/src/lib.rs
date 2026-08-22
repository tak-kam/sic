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

pub mod value;

use sic_bytecode::inst::Op;
use sic_bytecode::program::{Const, Program};
use sic_core::{CapError, CapRequest, CapValue};

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
    /// Where the result of a suspended capability call goes. `Some` exactly
    /// while the VM is suspended.
    pending: Option<usize>,
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
    pub fn new(program: &'a Program, fuel: u64) -> Self {
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
            fuel,
        }
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
        let Some(reg) = self.pending.take() else {
            return self.fail_now(FailKind::Internal("resumed while not suspended"));
        };
        let value = self.intern_cap_value(value);
        self.set(reg, value);
        self.execute()
    }

    /// Ends a suspended run because the capability call did not succeed.
    ///
    /// Retrying is a workflow decision, and the IR already has the slot for it,
    /// so nothing here tries to be clever about recovery.
    pub fn resume_failed(&mut self, error: &CapError) -> Status {
        self.pending = None;
        self.fail_with(FailKind::Capability, None, Some(error.message.clone()))
    }

    /// Whether the VM is waiting for a capability result.
    pub fn is_suspended(&self) -> bool {
        self.pending.is_some()
    }

    /// Calls a function and runs until it returns, fails, or runs out of fuel.
    pub fn run(&mut self, func: u32, args: &[Value]) -> Status {
        let Some(def) = self.program.funcs.get(func as usize) else {
            return self.fail_now(FailKind::Internal("no such function"));
        };
        if args.len() != def.param_count() {
            return self.fail_now(FailKind::Internal("wrong number of arguments"));
        }

        // Register 0 of the outermost frame receives the final result.
        self.regs = vec![Value::Unit; def.reg_count as usize + 1];
        for (i, arg) in args.iter().enumerate() {
            self.regs[1 + i] = arg.clone();
        }
        self.frames.push(Frame {
            func,
            pc: def.code_off,
            reg_base: 1,
            ret_reg: 0,
        });

        self.execute()
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
                    // The result lands here once the driver comes back.
                    self.pending = Some(base + a);
                    return Status::Suspended(CapRequest {
                        index: b as u32,
                        name,
                        args,
                    });
                }
                Op::Return => {
                    let value = self.get(base + a);
                    let frame = self.frames.pop().expect("a frame was running");
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
        let (argc, reg_count, code_off) = (def.param_count(), def.reg_count as usize, def.code_off);

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
        self.frames.push(Frame {
            func,
            pc: code_off,
            reg_base: new_base,
            ret_reg,
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
