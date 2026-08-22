//! The bytecode verifier.
//!
//! The contract is one-directional: the VM may skip a runtime check only
//! because the verifier established the property. So anything the VM assumes -
//! that a register it reads is initialized, that the operands of `ADD_I64` hold
//! integers, that a jump lands inside the function - has to be proved here.
//!
//! Structure and operand ranges are checked in a single pass. Initialization
//! and types are then established by forward abstract interpretation with a
//! worklist, intersecting at merge points, which converges because the lattice
//! only ever widens towards `Top`.

use std::collections::HashMap;

use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;

/// Resource limits. They exist so that a hostile file cannot turn verification
/// itself into the attack.
pub const MAX_FUNCS: usize = 256;
pub const MAX_CONSTS: usize = u16::MAX as usize + 1;
pub const MAX_CODE_LEN: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub func: Option<String>,
    pub pc: Option<u32>,
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.func, self.pc) {
            (Some(name), Some(pc)) => write!(f, "{name}+{pc:04}: {}", self.message),
            (Some(name), None) => write!(f, "{name}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Verifies a decoded program. Every problem is reported, not just the first.
pub fn verify(program: &Program) -> VerifyReport {
    let mut v = Verifier {
        program,
        report: VerifyReport::default(),
    };
    v.check_module();
    for func in &program.funcs {
        v.check_function(func);
    }
    v.report
}

struct Verifier<'a> {
    program: &'a Program,
    report: VerifyReport,
}

impl<'a> Verifier<'a> {
    fn error(&mut self, func: Option<&str>, pc: Option<u32>, message: impl Into<String>) {
        self.report.errors.push(Finding {
            func: func.map(str::to_string),
            pc,
            message: message.into(),
        });
    }

    fn warn(&mut self, func: Option<&str>, pc: Option<u32>, message: impl Into<String>) {
        self.report.warnings.push(Finding {
            func: func.map(str::to_string),
            pc,
            message: message.into(),
        });
    }

    // ---- module level ----

    fn check_module(&mut self) {
        let p = self.program;
        if p.funcs.len() > MAX_FUNCS {
            self.error(
                None,
                None,
                format!(
                    "{} functions exceed the limit of {MAX_FUNCS}",
                    p.funcs.len()
                ),
            );
        }
        if p.consts.len() > MAX_CONSTS {
            self.error(
                None,
                None,
                format!(
                    "{} constants exceed the limit of {MAX_CONSTS}",
                    p.consts.len()
                ),
            );
        }
        if p.code.len() > MAX_CODE_LEN {
            self.error(
                None,
                None,
                format!(
                    "{} instructions exceed the limit of {MAX_CODE_LEN}",
                    p.code.len()
                ),
            );
        }
        if !p.caps.is_empty() {
            self.error(
                None,
                None,
                "capabilities are declared but v0.1 cannot call one",
            );
        }

        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (i, f) in p.funcs.iter().enumerate() {
            if let Some(prev) = seen.insert(&f.name, i) {
                self.error(
                    Some(&f.name),
                    None,
                    format!("function name is also used by function {prev}"),
                );
            }
        }

        for (pc, _, _) in &p.debug.lines {
            if *pc as usize >= p.code.len() {
                self.error(
                    None,
                    None,
                    format!("the debug table names pc {pc}, which is past the code"),
                );
                break;
            }
        }
    }

    // ---- per function ----

    fn check_function(&mut self, func: &FuncDef) {
        let name = func.name.clone();
        let p = self.program;

        for t in func.params.iter().chain(std::iter::once(&func.ret_type)) {
            if *t as usize >= p.types.len() {
                self.error(Some(&name), None, format!("type index {t} is out of range"));
                return;
            }
        }
        if func.param_count() > func.reg_count as usize {
            self.error(
                Some(&name),
                None,
                format!(
                    "{} parameters do not fit in {} registers",
                    func.param_count(),
                    func.reg_count
                ),
            );
            return;
        }
        let end = func.code_off as u64 + func.code_len as u64;
        if end > p.code.len() as u64 {
            self.error(
                Some(&name),
                None,
                "the function's code runs past the code section",
            );
            return;
        }
        if func.code_len == 0 {
            self.error(Some(&name), None, "the function has no instructions");
            return;
        }

        // Structure first: the data flow pass may only run on instructions whose
        // operands are known to be in range.
        if !self.check_structure(func) {
            return;
        }
        self.check_data_flow(func);
    }

    /// Opcodes, operand ranges, jump targets, and how the function ends.
    fn check_structure(&mut self, func: &FuncDef) -> bool {
        let name = func.name.clone();
        let p = self.program;
        let mut ok = true;

        for i in 0..func.code_len {
            let pc = func.code_off + i;
            let inst = p.code[pc as usize];
            let Some(op) = inst.op() else {
                self.error(
                    Some(&name),
                    Some(pc),
                    format!("unknown opcode {}", inst.raw_op()),
                );
                ok = false;
                continue;
            };

            let regs = func.reg_count;
            let check_reg = |v: &mut Self, r: u8, what: &str| {
                if r >= regs {
                    v.error(
                        Some(&name),
                        Some(pc),
                        format!("{what} register r{r} is beyond reg_count {regs}"),
                    );
                    false
                } else {
                    true
                }
            };

            match op {
                Op::LoadConst => {
                    ok &= check_reg(self, inst.a(), "destination");
                    if inst.bx() as usize >= p.consts.len() {
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!("constant index k{} is out of range", inst.bx()),
                        );
                        ok = false;
                    }
                }
                Op::Move | Op::Not => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "source");
                }
                Op::AddI64
                | Op::SubI64
                | Op::MulI64
                | Op::DivI64
                | Op::RemI64
                | Op::Eq
                | Op::Ne
                | Op::Lt
                | Op::Le
                | Op::Gt
                | Op::Ge => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "left operand");
                    ok &= check_reg(self, inst.c(), "right operand");
                }
                Op::Jump => ok &= self.check_jump(func, pc, inst),
                Op::JumpIf | Op::JumpIfNot => {
                    ok &= check_reg(self, inst.a(), "condition");
                    ok &= self.check_jump(func, pc, inst);
                }
                Op::Call => {
                    ok &= check_reg(self, inst.a(), "destination");
                    let Some(callee) = p.funcs.get(inst.b() as usize) else {
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!("function index f{} is out of range", inst.b()),
                        );
                        ok = false;
                        continue;
                    };
                    let argc = callee.param_count();
                    let last = inst.c() as usize + argc;
                    if last > func.reg_count as usize {
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!(
                                "arguments r{}..r{} do not fit in reg_count {}",
                                inst.c(),
                                last,
                                func.reg_count
                            ),
                        );
                        ok = false;
                    }
                }
                Op::Return | Op::Fail => ok &= check_reg(self, inst.a(), "operand"),
                Op::Halt => {}
            }
        }

        // Control must not walk off the end of the function into whatever
        // instruction happens to be stored next.
        let last_pc = func.code_off + func.code_len - 1;
        match p.code[last_pc as usize].op() {
            Some(Op::Return | Op::Fail | Op::Jump | Op::Halt) => {}
            _ => {
                self.error(Some(&name), Some(last_pc), "control can fall out of the function; it must end in RETURN, FAIL, JUMP or HALT");
                ok = false;
            }
        }
        ok
    }

    fn check_jump(&mut self, func: &FuncDef, pc: u32, inst: Inst) -> bool {
        let target = pc as i64 + 1 + inst.sbx() as i64;
        let lo = func.code_off as i64;
        let hi = lo + func.code_len as i64;
        if target < lo || target >= hi {
            let name = func.name.clone();
            self.error(
                Some(&name),
                Some(pc),
                format!("jump target {target} is outside the function"),
            );
            return false;
        }
        true
    }

    /// Register initialization and types, by forward abstract interpretation.
    fn check_data_flow(&mut self, func: &FuncDef) {
        let name = func.name.clone();
        let p = self.program;
        let len = func.code_len as usize;

        let mut entry: State = vec![Abst::Uninit; func.reg_count as usize];
        for (i, t) in func.params.iter().enumerate() {
            entry[i] = Abst::Val(p.types[*t as usize]);
        }

        let mut states: Vec<Option<State>> = vec![None; len];
        states[0] = Some(entry);
        let mut work: Vec<usize> = vec![0];

        while let Some(index) = work.pop() {
            let pc = func.code_off + index as u32;
            let inst = p.code[pc as usize];
            let op = inst.op().expect("structure pass accepted every opcode");
            let state = states[index]
                .clone()
                .expect("only reachable states are queued");

            let mut next = state.clone();
            let mut successors: Vec<usize> = Vec::new();

            match op {
                Op::LoadConst => {
                    next[inst.a() as usize] = Abst::Val(p.consts[inst.bx() as usize].type_tag());
                    successors.push(index + 1);
                }
                Op::Move => {
                    let src = self.read(&name, pc, &state, inst.b(), None);
                    next[inst.a() as usize] = src;
                    successors.push(index + 1);
                }
                Op::AddI64 | Op::SubI64 | Op::MulI64 | Op::DivI64 | Op::RemI64 => {
                    self.read(&name, pc, &state, inst.b(), Some(TypeTag::Int));
                    self.read(&name, pc, &state, inst.c(), Some(TypeTag::Int));
                    next[inst.a() as usize] = Abst::Val(TypeTag::Int);
                    successors.push(index + 1);
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    self.read(&name, pc, &state, inst.b(), Some(TypeTag::Int));
                    self.read(&name, pc, &state, inst.c(), Some(TypeTag::Int));
                    next[inst.a() as usize] = Abst::Val(TypeTag::Bool);
                    successors.push(index + 1);
                }
                Op::Eq | Op::Ne => {
                    let l = self.read(&name, pc, &state, inst.b(), None);
                    let r = self.read(&name, pc, &state, inst.c(), None);
                    // Equality is one instruction for every type, so the VM can
                    // only be allowed to compare two values of the same one.
                    if let (Abst::Val(a), Abst::Val(b)) = (&l, &r) {
                        if a != b {
                            self.error(
                                Some(&name),
                                Some(pc),
                                format!("cannot compare {} with {}", a.name(), b.name()),
                            );
                        }
                    }
                    next[inst.a() as usize] = Abst::Val(TypeTag::Bool);
                    successors.push(index + 1);
                }
                Op::Not => {
                    self.read(&name, pc, &state, inst.b(), Some(TypeTag::Bool));
                    next[inst.a() as usize] = Abst::Val(TypeTag::Bool);
                    successors.push(index + 1);
                }
                Op::Jump => successors.push(jump_index(func, pc, inst)),
                Op::JumpIf | Op::JumpIfNot => {
                    self.read(&name, pc, &state, inst.a(), Some(TypeTag::Bool));
                    successors.push(index + 1);
                    successors.push(jump_index(func, pc, inst));
                }
                Op::Call => {
                    let callee = &p.funcs[inst.b() as usize];
                    for (i, want) in callee.params.iter().enumerate() {
                        let reg = inst.c() + i as u8;
                        self.read(&name, pc, &state, reg, Some(p.types[*want as usize]));
                    }
                    next[inst.a() as usize] = Abst::Val(p.types[callee.ret_type as usize]);
                    successors.push(index + 1);
                }
                Op::Return => {
                    let want = p.types[func.ret_type as usize];
                    self.read(&name, pc, &state, inst.a(), Some(want));
                }
                Op::Fail => {
                    self.read(&name, pc, &state, inst.a(), None);
                }
                Op::Halt => {}
            }

            for succ in successors {
                if succ >= len {
                    self.error(
                        Some(&name),
                        Some(pc),
                        "control continues past the end of the function",
                    );
                    continue;
                }
                let changed = match &mut states[succ] {
                    Some(existing) => merge(existing, &next),
                    slot @ None => {
                        *slot = Some(next.clone());
                        true
                    }
                };
                if changed {
                    work.push(succ);
                }
            }
        }

        for (i, state) in states.iter().enumerate() {
            if state.is_none() {
                self.warn(
                    Some(&name),
                    Some(func.code_off + i as u32),
                    "unreachable instruction",
                );
            }
        }
    }

    /// Reads a register, reporting it if it is uninitialized, ambiguous, or of
    /// the wrong type.
    fn read(&mut self, func: &str, pc: u32, state: &State, reg: u8, want: Option<TypeTag>) -> Abst {
        let value = state[reg as usize];
        match (value, want) {
            (Abst::Uninit, _) => {
                self.error(
                    Some(func),
                    Some(pc),
                    format!("r{reg} is read before it is written"),
                );
                Abst::Val(want.unwrap_or(TypeTag::Unit))
            }
            (Abst::Top, _) => {
                self.error(
                    Some(func),
                    Some(pc),
                    format!("r{reg} holds different types depending on the path taken"),
                );
                Abst::Val(want.unwrap_or(TypeTag::Unit))
            }
            (Abst::Val(found), Some(want)) if found != want => {
                self.error(
                    Some(func),
                    Some(pc),
                    format!(
                        "r{reg} holds {} where {} is required",
                        found.name(),
                        want.name()
                    ),
                );
                Abst::Val(want)
            }
            (value, _) => value,
        }
    }
}

/// What the verifier knows about one register at one point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abst {
    Uninit,
    Val(TypeTag),
    /// Reachable with different types, so nothing may be assumed about it.
    Top,
}

type State = Vec<Abst>;

/// Merges `incoming` into `state`, returning whether anything changed.
///
/// Uninitialized on either path means uninitialized after the merge, and two
/// different types widen to `Top`. Both directions only lose information, which
/// is what makes the fixed point terminate.
fn merge(state: &mut State, incoming: &State) -> bool {
    let mut changed = false;
    for (slot, new) in state.iter_mut().zip(incoming) {
        let merged = match (*slot, *new) {
            (a, b) if a == b => a,
            (Abst::Uninit, _) | (_, Abst::Uninit) => Abst::Uninit,
            _ => Abst::Top,
        };
        if merged != *slot {
            *slot = merged;
            changed = true;
        }
    }
    changed
}

fn jump_index(func: &FuncDef, pc: u32, inst: Inst) -> usize {
    let target = pc as i64 + 1 + inst.sbx() as i64;
    (target - func.code_off as i64) as usize
}

#[cfg(test)]
mod tests;
