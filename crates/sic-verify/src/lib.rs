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
        // Everything below assumes the first five type entries are the
        // primitives, in order.
        if p.types.len() < TypeDesc::PRIMITIVES.len()
            || p.types[..TypeDesc::PRIMITIVES.len()] != TypeDesc::PRIMITIVES
        {
            self.error(
                None,
                None,
                "the type section must begin with the primitive types in tag order",
            );
        }
        for (i, desc) in p.types.iter().enumerate() {
            let referenced: Vec<u32> = match desc {
                TypeDesc::Task(inner) | TypeDesc::List(inner) => vec![*inner],
                TypeDesc::Object { fields, .. } => fields.iter().map(|f| f.ty).collect(),
                _ => Vec::new(),
            };
            for inner in referenced {
                if inner as usize >= p.types.len() {
                    self.error(
                        None,
                        None,
                        format!("type {i} refers to type {inner}, which is out of range"),
                    );
                }
            }
        }
        self.check_policies();

        for (i, c) in p.caps.iter().enumerate() {
            for t in c.params.iter().chain(std::iter::once(&c.ret_type)) {
                if *t as usize >= p.types.len() {
                    self.error(
                        None,
                        None,
                        format!("capability c{i} names type index {t}, which is out of range"),
                    );
                }
            }
        }
        self.check_manifest_is_minimal();

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

        for (pc, _, _, _) in &p.debug.lines {
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

    /// A policy names a call site, so the site has to be a capability call.
    ///
    /// And a budget has to be a bound something can actually count. The VM
    /// counts a call against the allowance its own policy entry names, so a
    /// budget with no allowance would be enforced against nothing, and two
    /// sites in one allowance that disagree about its size would be enforced
    /// as whichever of the two the run reached. Both are unwritable by the
    /// compiler and neither is unwritable by a file: this is where a file is
    /// refused rather than half-understood.
    fn check_policies(&mut self) {
        let p = self.program;
        let mut allowances: Vec<(u32, u32)> = Vec::new();
        for policy in &p.policies {
            if policy.budget > 0 && policy.budget_group == 0 {
                self.error(
                    None,
                    None,
                    format!(
                        "the policy at {} has a budget and no allowance to count it against",
                        policy.pc
                    ),
                );
            }
            if policy.budget_group != 0 {
                match allowances.iter().find(|(g, _)| *g == policy.budget_group) {
                    Some((_, calls)) if *calls != policy.budget => self.error(
                        None,
                        None,
                        format!(
                            "the policy at {} shares allowance {} and disagrees about its size, \
                             {} against {calls}",
                            policy.pc, policy.budget_group, policy.budget
                        ),
                    ),
                    Some(_) => {}
                    None => allowances.push((policy.budget_group, policy.budget)),
                }
            }
            match p.code.get(policy.pc as usize).and_then(|i| i.op()) {
                Some(Op::CallCap) => {}
                _ => self.error(
                    None,
                    None,
                    format!(
                        "a policy names instruction {}, which is not a capability call",
                        policy.pc
                    ),
                ),
            }
            if policy.attempts == 0 {
                self.error(
                    None,
                    None,
                    format!("the policy at {} allows zero attempts", policy.pc),
                );
            }
            // The VM parses against this type before it lets go of the pending
            // call, so an index out of range would be a panic at the moment an
            // answer arrives rather than at load.
            if policy.validates > 0 && p.types.get(policy.validates as usize - 1).is_none() {
                self.error(
                    None,
                    None,
                    format!(
                        "the policy at {} validates against type {}, which does not exist",
                        policy.pc,
                        policy.validates - 1
                    ),
                );
            }
        }
    }

    /// A capability that is granted but never called is authority the module
    /// does not need. That is a warning rather than an error, because removing
    /// it is the author's decision, but it should never pass unnoticed.
    fn check_manifest_is_minimal(&mut self) {
        let p = self.program;
        let mut used = vec![false; p.caps.len()];
        for inst in &p.code {
            if inst.op() == Some(Op::CallCap) {
                if let Some(slot) = used.get_mut(inst.b() as usize) {
                    *slot = true;
                }
            }
        }
        for (i, granted) in used.iter().enumerate() {
            if !granted {
                let name = p.caps[i].name.clone();
                self.warn(None, None, format!("`{name}` is granted but never called"));
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
                Op::Move | Op::Approve | Op::Not => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "source");
                }
                Op::AddI64
                | Op::SubI64
                | Op::MulI64
                | Op::DivI64
                | Op::RemI64
                | Op::Concat
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
                    ok &= self.window_fits(func, pc, inst.c(), callee.param_count(), "arguments");
                }
                Op::CallCap => {
                    ok &= check_reg(self, inst.a(), "destination");
                    let Some(cap) = p.caps.get(inst.b() as usize) else {
                        // A capability the manifest does not declare cannot be
                        // called: the manifest is the contract with the broker.
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!("capability index c{} is not in the manifest", inst.b()),
                        );
                        ok = false;
                        continue;
                    };
                    ok &= self.window_fits(func, pc, inst.c(), cap.params.len(), "arguments");
                }
                Op::Spawn => {
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
                    ok &= self.window_fits(func, pc, inst.c(), callee.param_count(), "arguments");
                }
                Op::Await | Op::Len => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "operand");
                }
                Op::MakeObject => {
                    ok &= check_reg(self, inst.a(), "destination");
                    let Some(fields) = p.types.get(inst.b() as usize).and_then(|t| t.fields())
                    else {
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!("type t{} is not a record", inst.b()),
                        );
                        ok = false;
                        continue;
                    };
                    ok &= self.window_fits(func, pc, inst.c(), fields.len(), "fields");
                }
                Op::GetField | Op::GetOpt | Op::HasOpt => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "record");
                }
                Op::FromJson | Op::ToJson => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.c(), "document");
                    if inst.b() as usize >= p.types.len() {
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!("type index t{} is out of range", inst.b()),
                        );
                        ok = false;
                    }
                }
                Op::MakeList => {
                    ok &= check_reg(self, inst.a(), "destination");
                    // The elements start at `b`, because `c` is how many of
                    // them there are.
                    ok &= self.window_fits(func, pc, inst.b(), inst.c() as usize, "elements");
                    if inst.c() == 0 {
                        // An empty list is a constant, because it has no
                        // element to take a type from.
                        self.error(
                            Some(&name),
                            Some(pc),
                            "MAKE_LIST needs at least one element",
                        );
                        ok = false;
                    }
                }
                Op::GetIndex => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "list");
                    ok &= check_reg(self, inst.c(), "index");
                }
                // Three registers and nothing else: no type index, no window,
                // so the structure pass has only their numbers to check.
                Op::Contains | Op::StartsWith => {
                    ok &= check_reg(self, inst.a(), "destination");
                    ok &= check_reg(self, inst.b(), "string");
                    ok &= check_reg(self, inst.c(), "sought");
                }
                Op::Return | Op::Fail => ok &= check_reg(self, inst.a(), "operand"),
                // `a` is the level, which is a number rather than a register,
                // so only `b` is one.
                Op::Log => ok &= check_reg(self, inst.b(), "message"),
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

    /// Whether the `count` registers starting at `first` fit inside `func`'s
    /// frame, which is `first + count <= reg_count`.
    ///
    /// The bound is exclusive at the top: `first + count` is one past the last
    /// register the instruction touches, so a window ending exactly at
    /// `reg_count` is inside the frame and an empty one is inside it wherever
    /// it starts. The range is written here once because five opcodes pass a
    /// window - `CALL`, `CALL_CAP`, `SPAWN`, `MAKE_OBJECT`, `MAKE_LIST` - and
    /// five hand-written copies of one rule are how they come to disagree.
    ///
    /// Which operand `first` comes from stays with the caller, because it is
    /// not the same operand for all five: `c` for the arguments of `CALL`,
    /// `CALL_CAP` and `SPAWN` and for the fields of `MAKE_OBJECT`, but `b` for
    /// the elements of `MAKE_LIST`, whose `c` is how many there are.
    fn window_fits(
        &mut self,
        func: &FuncDef,
        pc: u32,
        first: u8,
        count: usize,
        what: &str,
    ) -> bool {
        let last = first as usize + count;
        if last <= func.reg_count as usize {
            return true;
        }
        let name = func.name.clone();
        self.error(
            Some(&name),
            Some(pc),
            format!(
                "{what} r{first}..r{last} do not fit in reg_count {}",
                func.reg_count
            ),
        );
        false
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
            entry[i] = Abst::Val(*t);
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
                    let konst = &p.consts[inst.bx() as usize];
                    let ty = match konst.list_type() {
                        // An empty list carries the type it is empty of.
                        Some(index) => {
                            if index as usize >= p.types.len() {
                                self.error(
                                    Some(&name),
                                    Some(pc),
                                    format!(
                                        "constant k{} names type {index}, which is out of range",
                                        inst.bx()
                                    ),
                                );
                                UNIT
                            } else {
                                index
                            }
                        }
                        None => konst
                            .type_desc()
                            .primitive_index()
                            .expect("every other constant is primitive"),
                    };
                    next[inst.a() as usize] = Abst::Val(ty);
                    successors.push(index + 1);
                }
                // The same rule as `MOVE`, because it is the same copy. What
                // it adds is a fact for a reader of the file, not a constraint
                // on the value: whether a person may agree to a thing is the
                // checker's question, and the answer is not in the bytecode.
                Op::Move | Op::Approve => {
                    let src = self.read(&name, pc, &state, inst.b(), None);
                    next[inst.a() as usize] = src;
                    successors.push(index + 1);
                }
                Op::AddI64 | Op::SubI64 | Op::MulI64 | Op::DivI64 | Op::RemI64 => {
                    self.read(&name, pc, &state, inst.b(), Some(INT));
                    self.read(&name, pc, &state, inst.c(), Some(INT));
                    next[inst.a() as usize] = Abst::Val(INT);
                    successors.push(index + 1);
                }
                // Both operands are strings and so is the result. That is what
                // lets the VM's arm read two handles out of the arena without
                // asking what they hold, and it is the whole of what stops a
                // hand-written `CONCAT r0, r1, r2` over two integers.
                Op::Concat => {
                    self.read(&name, pc, &state, inst.b(), Some(STR));
                    self.read(&name, pc, &state, inst.c(), Some(STR));
                    next[inst.a() as usize] = Abst::Val(STR);
                    successors.push(index + 1);
                }
                // One instruction for each of the two types the VM knows an
                // order for, so this is `EQ`'s rule over a smaller set: the
                // operands must agree, and they must be a type an order
                // exists on. A `Bool` and a `String` are equal or not and
                // neither is less than the other, so `LT` over them is not an
                // instruction the VM could execute and is refused here rather
                // than discovered there.
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    let l = self.read(&name, pc, &state, inst.b(), None);
                    let r = self.read(&name, pc, &state, inst.c(), None);
                    if let (Abst::Val(a), Abst::Val(b)) = (l, r) {
                        if a != b {
                            let (a, b) = (p.type_name(a), p.type_name(b));
                            self.error(
                                Some(&name),
                                Some(pc),
                                format!("cannot order {a} against {b}"),
                            );
                        } else if a != INT && a != FLOAT {
                            let a = p.type_name(a);
                            self.error(Some(&name), Some(pc), format!("{a} has no order"));
                        }
                    }
                    next[inst.a() as usize] = Abst::Val(BOOL);
                    successors.push(index + 1);
                }
                Op::Eq | Op::Ne => {
                    let l = self.read(&name, pc, &state, inst.b(), None);
                    let r = self.read(&name, pc, &state, inst.c(), None);
                    // Equality is one instruction for every type, so the VM can
                    // only be allowed to compare two values of the same one.
                    if let (Abst::Val(a), Abst::Val(b)) = (l, r) {
                        if a != b {
                            let (a, b) = (p.type_name(a), p.type_name(b));
                            self.error(
                                Some(&name),
                                Some(pc),
                                format!("cannot compare {a} with {b}"),
                            );
                        }
                    }
                    next[inst.a() as usize] = Abst::Val(BOOL);
                    successors.push(index + 1);
                }
                Op::Not => {
                    self.read(&name, pc, &state, inst.b(), Some(BOOL));
                    next[inst.a() as usize] = Abst::Val(BOOL);
                    successors.push(index + 1);
                }
                Op::Jump => successors.push(jump_index(func, pc, inst)),
                Op::JumpIf | Op::JumpIfNot => {
                    self.read(&name, pc, &state, inst.a(), Some(BOOL));
                    successors.push(index + 1);
                    successors.push(jump_index(func, pc, inst));
                }
                Op::Call => {
                    let callee = &p.funcs[inst.b() as usize];
                    for (i, want) in callee.params.iter().enumerate() {
                        let reg = inst.c() + i as u8;
                        self.read(&name, pc, &state, reg, Some(*want));
                    }
                    next[inst.a() as usize] = Abst::Val(callee.ret_type);
                    successors.push(index + 1);
                }
                Op::Spawn => {
                    let callee = &p.funcs[inst.b() as usize];
                    for (i, want) in callee.params.iter().enumerate() {
                        let reg = inst.c() + i as u8;
                        self.read(&name, pc, &state, reg, Some(*want));
                    }
                    // The task type has to be in the section, or the verifier
                    // could not say what awaiting it produces.
                    let wanted = TypeDesc::Task(callee.ret_type);
                    match p.types.iter().position(|d| *d == wanted) {
                        Some(i) => next[inst.a() as usize] = Abst::Val(i as u32),
                        None => {
                            self.error(
                                Some(&name),
                                Some(pc),
                                format!(
                                    "the type section has no `Task<{}>`",
                                    p.type_name(callee.ret_type)
                                ),
                            );
                            next[inst.a() as usize] = Abst::Top;
                        }
                    }
                    successors.push(index + 1);
                }
                Op::Await => {
                    let task = self.read(&name, pc, &state, inst.b(), None);
                    let produced = match task {
                        Abst::Val(index) => match p.types.get(index as usize) {
                            Some(TypeDesc::Task(inner)) => Some(*inner),
                            _ => {
                                self.error(
                                    Some(&name),
                                    Some(pc),
                                    format!(
                                        "r{} holds {}, which is not a task",
                                        inst.b(),
                                        p.type_name(index)
                                    ),
                                );
                                None
                            }
                        },
                        _ => None,
                    };
                    next[inst.a() as usize] = match produced {
                        Some(inner) => Abst::Val(inner),
                        None => Abst::Top,
                    };
                    successors.push(index + 1);
                }
                Op::CallCap => {
                    let cap = &p.caps[inst.b() as usize];
                    for (i, want) in cap.params.iter().enumerate() {
                        let reg = inst.c() + i as u8;
                        self.read(&name, pc, &state, reg, Some(*want));
                    }
                    next[inst.a() as usize] = Abst::Val(cap.ret_type);
                    successors.push(index + 1);
                }
                Op::MakeObject => {
                    let fields = p.types[inst.b() as usize]
                        .fields()
                        .expect("checked in the structure pass");
                    for (i, field) in fields.iter().enumerate() {
                        let reg = inst.c() + i as u8;
                        if !field.optional {
                            self.read(&name, pc, &state, reg, Some(field.ty));
                            continue;
                        }
                        // An optional field's slot holds the field's own type
                        // or `null`, and those can never be the same, because
                        // a `Unit` field cannot be optional (E0355).
                        let found = self.read(&name, pc, &state, reg, None);
                        if let Abst::Val(ty) = found {
                            if ty != field.ty && ty != UNIT {
                                let (found, want) = (p.type_name(ty), p.type_name(field.ty));
                                self.error(
                                    Some(&name),
                                    Some(pc),
                                    format!(
                                        "r{reg} holds {found} where {want} or null is required"
                                    ),
                                );
                            }
                        }
                    }
                    next[inst.a() as usize] = Abst::Val(inst.b() as u32);
                    successors.push(index + 1);
                }
                // Three instructions read a field, and what separates them is
                // which fields they may name. `GET_FIELD` may not name an
                // optional one, because it cannot fail and that one can;
                // the other two may name nothing else, because there is
                // nothing to ask about a field that is always there.
                Op::GetField | Op::GetOpt | Op::HasOpt => {
                    let want_optional = op != Op::GetField;
                    let record = self.read(&name, pc, &state, inst.b(), None);
                    let produced = match record {
                        Abst::Val(ty) => match p.types.get(ty as usize).and_then(|t| t.fields()) {
                            Some(fields) => match fields.get(inst.c() as usize) {
                                Some(field) if field.optional != want_optional => {
                                    let how = if want_optional {
                                        "is not optional"
                                    } else {
                                        "is optional"
                                    };
                                    self.error(
                                        Some(&name),
                                        Some(pc),
                                        format!("field {} of {} {how}", inst.c(), p.type_name(ty)),
                                    );
                                    None
                                }
                                Some(field) => Some(if op == Op::HasOpt { BOOL } else { field.ty }),
                                None => {
                                    self.error(
                                        Some(&name),
                                        Some(pc),
                                        format!("{} has no field {}", p.type_name(ty), inst.c()),
                                    );
                                    None
                                }
                            },
                            None => {
                                self.error(
                                    Some(&name),
                                    Some(pc),
                                    format!(
                                        "r{} holds {}, which has no fields",
                                        inst.b(),
                                        p.type_name(ty)
                                    ),
                                );
                                None
                            }
                        },
                        _ => None,
                    };
                    next[inst.a() as usize] = match produced {
                        Some(ty) => Abst::Val(ty),
                        None => Abst::Top,
                    };
                    successors.push(index + 1);
                }
                Op::MakeList => {
                    // Every element has the same type, and the section has to
                    // hold the list type so `GET_INDEX` can be checked.
                    let first = self.read(&name, pc, &state, inst.b(), None);
                    let element = match first {
                        Abst::Val(ty) => Some(ty),
                        _ => None,
                    };
                    for i in 1..inst.c() {
                        self.read(&name, pc, &state, inst.b() + i, element);
                    }
                    next[inst.a() as usize] = match element
                        .and_then(|el| p.types.iter().position(|d| *d == TypeDesc::List(el)))
                    {
                        Some(i) => Abst::Val(i as u32),
                        None => {
                            if let Some(el) = element {
                                self.error(
                                    Some(&name),
                                    Some(pc),
                                    format!("the type section has no `List<{}>`", p.type_name(el)),
                                );
                            }
                            Abst::Top
                        }
                    };
                    successors.push(index + 1);
                }
                Op::GetIndex => {
                    let list = self.read(&name, pc, &state, inst.b(), None);
                    self.read(&name, pc, &state, inst.c(), Some(INT));
                    let produced = match list {
                        Abst::Val(ty) => match p.types.get(ty as usize) {
                            Some(TypeDesc::List(element)) => Some(*element),
                            _ => {
                                self.error(
                                    Some(&name),
                                    Some(pc),
                                    format!(
                                        "r{} holds {}, which cannot be indexed",
                                        inst.b(),
                                        p.type_name(ty)
                                    ),
                                );
                                None
                            }
                        },
                        _ => None,
                    };
                    next[inst.a() as usize] = match produced {
                        Some(ty) => Abst::Val(ty),
                        None => Abst::Top,
                    };
                    successors.push(index + 1);
                }
                Op::Len => {
                    let operand = self.read(&name, pc, &state, inst.b(), None);
                    if let Abst::Val(ty) = operand {
                        let ok = ty == STR
                            || matches!(p.types.get(ty as usize), Some(TypeDesc::List(_)));
                        if !ok {
                            self.error(
                                Some(&name),
                                Some(pc),
                                format!("`len` cannot be applied to {}", p.type_name(ty)),
                            );
                        }
                    }
                    next[inst.a() as usize] = Abst::Val(INT);
                    successors.push(index + 1);
                }
                // Both operands are strings, so the VM reads two arena
                // handles and does not check what they hold. The result is a
                // `Bool` whatever they were.
                Op::Contains | Op::StartsWith => {
                    self.read(&name, pc, &state, inst.b(), Some(STR));
                    self.read(&name, pc, &state, inst.c(), Some(STR));
                    next[inst.a() as usize] = Abst::Val(BOOL);
                    successors.push(index + 1);
                }
                Op::FromJson => {
                    self.read(&name, pc, &state, inst.c(), Some(STR));
                    next[inst.a() as usize] = Abst::Val(inst.b() as u32);
                    successors.push(index + 1);
                }
                // The inverse, and checked as one: the register read holds the
                // type the instruction names, and what comes out is text.
                Op::ToJson => {
                    self.read(&name, pc, &state, inst.c(), Some(inst.b() as u32));
                    next[inst.a() as usize] = Abst::Val(STR);
                    successors.push(index + 1);
                }
                Op::Return => {
                    let want = func.ret_type;
                    self.read(&name, pc, &state, inst.a(), Some(want));
                }
                Op::Fail => {
                    self.read(&name, pc, &state, inst.a(), None);
                }
                // A message is a string and the level is one of four. Both are
                // checked here rather than trusted, because a file that
                // decodes says nothing about what is in it.
                Op::Log => {
                    self.read(&name, pc, &state, inst.b(), Some(STR));
                    if inst.a() > 3 {
                        self.error(
                            Some(&name),
                            Some(pc),
                            format!("LOG names level {}, and there are four", inst.a()),
                        );
                    }
                    successors.push(index + 1);
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
    fn read(&mut self, func: &str, pc: u32, state: &State, reg: u8, want: Option<u32>) -> Abst {
        let value = state[reg as usize];
        match (value, want) {
            (Abst::Uninit, _) => {
                self.error(
                    Some(func),
                    Some(pc),
                    format!("r{reg} is read before it is written"),
                );
                Abst::Val(want.unwrap_or(UNIT))
            }
            (Abst::Top, _) => {
                self.error(
                    Some(func),
                    Some(pc),
                    format!("r{reg} holds different types depending on the path taken"),
                );
                Abst::Val(want.unwrap_or(UNIT))
            }
            (Abst::Val(found), Some(want)) if found != want => {
                let (found, want_name) =
                    (self.program.type_name(found), self.program.type_name(want));
                self.error(
                    Some(func),
                    Some(pc),
                    format!("r{reg} holds {found} where {want_name} is required"),
                );
                Abst::Val(want)
            }
            (value, _) => value,
        }
    }
}

/// What the verifier knows about one register at one point.
///
/// A type is an index into the program's type section, so comparing two types
/// is comparing two numbers. That works because the compiler deduplicates the
/// section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abst {
    Uninit,
    Val(u32),
    /// Reachable with different types, so nothing may be assumed about it.
    Top,
}

/// The primitives occupy the first five entries of the type section, in tag
/// order. `check_module` refuses a program where that does not hold, which is
/// what lets these be constants.
const UNIT: u32 = 0;
const BOOL: u32 = 1;
const INT: u32 = 2;
const FLOAT: u32 = 3;
const STR: u32 = 4;

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
